#!/usr/bin/env python3
"""Build the render submission experiment from a frozen real-source host copy.

Only disposable copies of wgpu-core and wgpu-hal are modified. NOOP HAL measures
host work, never Vulkan/driver performance. The experiment's integration and
fixture modules are separate inputs and are fingerprinted with the vendor copy.
"""
from __future__ import annotations

import argparse
from datetime import datetime, timezone
import difflib
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import tempfile


HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[2]


def fingerprint(path: Path) -> dict:
    return {"path": str(path.resolve()), "sha256": hashlib.sha256(path.read_bytes()).hexdigest()}


def inventory(directory: Path) -> list[dict]:
    return [dict(relativePath=str(path.relative_to(directory)), **fingerprint(path))
            for path in sorted(directory.rglob("*")) if path.is_file()]


def source_patch(original: Path, copied: Path, crate: str) -> str:
    """Reviewable Rust-source changes; temporary Cargo pruning stays in report."""
    paths = ({path.relative_to(original) for path in original.rglob("*.rs")} |
             {path.relative_to(copied) for path in copied.rglob("*.rs")})
    chunks = []
    for relative in sorted(paths):
        before, after = original / relative, copied / relative
        before_text = before.read_text().splitlines(keepends=True) if before.exists() else []
        after_text = after.read_text().splitlines(keepends=True) if after.exists() else []
        path = "game-runtime/vendor/" + crate + "/" + str(relative)
        chunks.extend(difflib.unified_diff(before_text, after_text,
                      fromfile="a/" + path if before.exists() else "/dev/null",
                      tofile="b/" + path if after.exists() else "/dev/null"))
    return "".join(chunks)


def prune_manifest(source: str, *, core: bool) -> tuple[str, list[str]]:
    """Remove inactive dependency resolution edges only in a host NOOP copy.

    Cargo resolves optional/foreign-platform declarations even for NOOP. Keep
    every nonoptional runtime dependency and its exact existing declaration.
    Retain the active core std/noop/wgsl feature meanings. HAL NOOP is always
    compiled; no optional hardware backend feature is active in this harness.
    """
    removed = []

    def prune_section(match: re.Match) -> str:
        heading = match[1]
        if ("dev-dependencies" in heading or
                re.search(r"^optional\s*=\s*true\s*$", match[0], re.M)):
            removed.append(heading)
            return "\n"
        return match[0]

    manifest = re.sub(r"\n\[([^\n]*dependencies[^\n]*)\]\n.*?(?=\n\[|\Z)",
                      prune_section, source, flags=re.S)
    block = re.search(r"\[features\]\n(.*?)(?=\n\[)", source, re.S)
    if block is None:
        raise ValueError("manifest features section missing")
    names = re.findall(r"^([A-Za-z0-9_-]+)\s*=", block[1], re.M)
    active = {"default": '["std"]', "std": '[]', "noop": '[]',
              "wgsl": '["naga/wgsl-in"]',
              "strict_asserts": '["wgpu-types/strict_asserts"]'} if core else {}
    features = "[features]\n" + "\n".join(
        name + " = " + active.get(name, "[]") for name in names) + "\n"
    manifest = re.sub(r"\[features\]\n.*?(?=\n\[)", features, manifest, flags=re.S)
    # Published HAL examples are omitted from the vendor tree. They are never
    # built by --lib, and their declarations have no role in this experiment.
    manifest = re.sub(r"\n\[\[example\]\]\n.*?(?=\n\[|\Z)", "\n", manifest, flags=re.S)
    return manifest, removed


def append_tracker_introspection(core: Path) -> None:
    """Read-only snapshots of real tracker state and strong owner identities."""
    for name, states in [("buffer", "&self.start, &self.end"),
                         ("texture", "&self.start_set, &self.end_set")]:
        extra = '''
// TEMPORARY HOST TEST INTROSPECTION ONLY. No production tracker body changes.
#[cfg(test)]
impl ''' + name.title() + '''Tracker {
    pub(crate) fn host_fingerprint(&self) -> alloc::string::String {
        let owned: Vec<_> = self.metadata.owned_indices().map(|i| {
            let r = unsafe { self.metadata.get_resource_unchecked(i) };
            (i, Arc::as_ptr(r) as usize)
        }).collect();
        alloc::format!("{:?}|{:?}|{:?}|{:?}", ''' + states + ''', owned, self.temp)
    }
}
'''
        with (core / "src/track" / (name + ".rs")).open("a") as stream:
            stream.write(extra)
    with (core / "src/command/memory_init.rs").open("a") as stream:
        stream.write('''
// TEMPORARY HOST TEST INTROSPECTION ONLY. Read-only; no init action drain.
#[cfg(test)]
impl CommandBufferTextureMemoryActions {
    pub(crate) fn host_fingerprint(&self) -> alloc::string::String {
        let inits: Vec<_> = self.init_actions.iter().map(|action| (
            Arc::as_ptr(&action.texture) as usize,
            action.range.mip_range.clone(), action.range.layer_range.clone(), action.kind,
        )).collect();
        let discards: Vec<_> = self.discards.iter().map(|discard| (
            Arc::as_ptr(&discard.texture) as usize, discard.mip_level, discard.layer,
        )).collect();
        alloc::format!("{:?}|{:?}", inits, discards)
    }
}
''')


def prepare(output: Path, workdir: Path | None = None, *, integrate: bool = True) -> dict:
    output = output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    workdir = (workdir.resolve() if workdir else
               Path(tempfile.mkdtemp(prefix="gta-render-submit-", dir="/private/tmp")))
    if workdir == ROOT or ROOT in workdir.parents:
        raise ValueError("host source copy must be outside the repository")
    core_source = ROOT / "game-runtime/vendor/wgpu-core"
    hal_source = ROOT / "game-runtime/vendor/wgpu-hal"
    core = workdir / "wgpu-core"
    hal = workdir / "wgpu-hal"
    if core.exists() or hal.exists():
        raise ValueError("workdir must not contain prior wgpu-core/wgpu-hal source copies")
    original = {"core": inventory(core_source), "hal": inventory(hal_source)}
    for source, destination in [(core_source, core), (hal_source, hal)]:
        shutil.copytree(source, destination, ignore=shutil.ignore_patterns("target", ".git"))
    removed = {}
    for name, destination in [("core", core), ("hal", hal)]:
        manifest_path = destination / "Cargo.toml"
        manifest, removed[name] = prune_manifest(manifest_path.read_text(), core=name == "core")
        if name == "core":
            manifest += '\n[patch.crates-io]\nwgpu-hal = { path = ' + json.dumps(str(hal)) + ' }\n'
        manifest_path.write_text(manifest)
    shutil.copyfile(ROOT / "game-runtime/Cargo.lock", core / "Cargo.lock")
    (workdir / ".cargo").mkdir(exist_ok=True)
    shutil.copyfile(ROOT / "game-runtime/.cargo/config.toml", workdir / ".cargo/config.toml")
    experiments = []
    for source_name, destination_name in [("policy.rs", "render_submit_prototype.rs"),
                                          ("host_render_harness.rs", "host_render_harness.rs")]:
        source = HERE / source_name
        if not source.is_file():
            raise FileNotFoundError(source)
        shutil.copyfile(source, core / "src/command" / destination_name)
        experiments.append(fingerprint(source))
    with (core / "src/command/mod.rs").open("a") as stream:
        stream.write('\n#[cfg(all(test, feature = "noop"))]\nmod render_submit_prototype;\n')
        stream.write('#[cfg(all(test, feature = "noop"))]\nmod host_render_harness;\n')
    append_tracker_introspection(core)
    integration_command = None
    if integrate:
        integration = HERE / "integrate.py"
        spec = importlib.util.spec_from_file_location("render_submit_integrate", integration)
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        module.integrate(core, hal)
        integration_command = {"source": str(integration), "function": "integrate(core, hal)"}
        experiments.append(fingerprint(integration))
    # Include policy and orchestration sources even when integrate() installs
    # their generated output rather than prepare() copying them directly.
    experiments = [fingerprint(path) for path in sorted(HERE.iterdir())
                   if path.is_file() and path.suffix in (".py", ".rs", ".json", ".md")]
    patch = output / "host-source-changes.patch"
    patch.write_text(source_patch(core_source, core, "wgpu-core") +
                     source_patch(hal_source, hal, "wgpu-hal"))
    report = {
        "schemaVersion": 1,
        "createdAtUtc": datetime.now(timezone.utc).isoformat(),
        "workdir": str(workdir), "core": str(core), "hal": str(hal),
        "output": str(output), "workspaceVendorEdited": False,
        "sourceScope": "real dirty-tree vendor sources copied byte for byte before documented host-only changes",
        "originalSourceInventory": original,
        "experimentInputs": experiments,
        "hostOnlyRustSourcePatch": fingerprint(patch),
        "inactiveDependencyEdgesRemovedOnlyInCopy": removed,
        "integrationCommand": integration_command,
        "compiledSourceInventory": {"core": inventory(core), "hal": inventory(hal)},
        "limits": "Host NOOP only. No Vulkan validation, Maleoon driver timings, GPU output or real-device frame-time improvement is established.",
    }
    (output / "prepared-source.json").write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n")
    return report


def run(report: dict, *, release: bool = True, test_filter: str = "host_render_harness") -> int:
    workdir = Path(report["workdir"])
    output = Path(report["output"])
    log = output / ("host-tests-release.log" if release else "host-tests-debug.log")
    command = ["cargo", "test", "--offline", "--manifest-path", str(Path(report["core"]) / "Cargo.toml"),
               "--target-dir", str(workdir / "target"), "--lib", "--no-default-features",
               "--features", "std,noop,wgsl"]
    if release:
        command.append("--release")
    command.extend([test_filter, "--", "--nocapture", "--test-threads=1"])
    # Source the repository's normal cached Rust environment. Paths/arguments
    # are positional parameters; they are never interpolated into shell code.
    env_script = ROOT / "game-runtime/scripts/env.sh"
    shell_command = ["/bin/bash", "--noprofile", "--norc", "-c",
                     'source "$1"; shift; exec "$@"', "render-submit-host", str(env_script), *command]
    env = dict(os.environ, MUGUET_RENDER_SUBMIT_OUTPUT=str(output))
    with log.open("w") as stream:
        done = subprocess.run(shell_command, cwd=workdir, env=env, stdout=stream, stderr=subprocess.STDOUT)
    log_text = log.read_text()
    counts = [int(value) for value in re.findall(r"^running (\d+) tests?$", log_text, re.M)]
    ran_tests = sum(counts)
    source_integrity = all(
        Path(row["path"]).is_file() and fingerprint(Path(row["path"]))["sha256"] == row["sha256"]
        for rows in report["originalSourceInventory"].values() for row in rows)
    exit_code = done.returncode or (0 if ran_tests > 0 and source_integrity else 1)
    resolved_lock = output / "cargo-resolved.lock"
    shutil.copyfile(Path(report["core"]) / "Cargo.lock", resolved_lock)
    result = {"exitCode": exit_code, "cargoExitCode": done.returncode,
              "status": "pass" if exit_code == 0 else "failed", "executedTestCount": ran_tests,
              "originalVendorInputsUnchanged": source_integrity,
              "command": command, "resolvedLockfile": fingerprint(resolved_lock),
              "log": fingerprint(log), "preparedSource": fingerprint(output / "prepared-source.json"),
              "limits": report["limits"]}
    (output / "host-run.json").write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps({"exitCode": exit_code, "workdir": str(workdir), "log": str(log)}))
    return exit_code


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--workdir", type=Path)
    parser.add_argument("--no-run", action="store_true")
    parser.add_argument("--debug", action="store_true")
    parser.add_argument("--test-filter", default="host_render_harness")
    args = parser.parse_args()
    report = prepare(args.output, args.workdir)
    if args.no_run:
        print(json.dumps({key: report[key] for key in ("workdir", "core", "hal", "output")}, indent=2))
    else:
        raise SystemExit(run(report, release=not args.debug, test_filter=args.test_filter))


if __name__ == "__main__":
    main()
