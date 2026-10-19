#!/usr/bin/env python3
"""Actual helper env tests plus bounded source wiring checks; no shader/game build.

The private rewrite case runs only when MUGUET_DEPTH_SHADER_FIXTURE (or the
legacy local resource path) exists; its absence is reported as not run.
"""
from pathlib import Path
import hashlib
import json
import os
import re
import subprocess

ROOT = Path(__file__).resolve().parents[2]
HELPER = ROOT / "game-runtime/src/depth_downsample.rs"
GPU = ROOT / "game-runtime/src/gpu.rs"
OUT = ROOT / "native-platform/evidence/depth-reduction-optimization/v3-zero-init-review/production-selector"
HOST = ROOT / "game-runtime/logs/depth-zero-selector-host"
SOURCE = Path(os.environ.get("MUGUET_DEPTH_SHADER_FIXTURE", str(ROOT / "resources/game/b/8b0b5899ed/shaders/23a7a3c1c0d38f46.wgsl"))).resolve()

def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()

def block(text, marker, start=0):
    """Read a bounded Rust braced block, skipping strings and comments."""
    begin = text.index(marker, start)
    opening = text.index("{", begin)
    i, depth = opening, 0
    while i < len(text):
        if text.startswith("//", i):
            end = text.find("\n", i)
            i = len(text) if end < 0 else end + 1
            continue
        if text.startswith("/*", i):
            end = text.index("*/", i + 2)
            i = end + 2
            continue
        if text[i] == '"':
            i += 1
            while i < len(text):
                if text[i] == "\\":
                    i += 2
                elif text[i] == '"':
                    i += 1
                    break
                else:
                    i += 1
            continue
        if text[i] == "{":
            depth += 1
        elif text[i] == "}":
            depth -= 1
            if depth == 0:
                return text[begin:i + 1]
        i += 1
    raise AssertionError(f"unterminated block {marker}")

def main():
    OUT.mkdir(parents=True, exist_ok=True)
    HOST.mkdir(parents=True, exist_ok=True)
    before = {"helperSHA256": sha(HELPER), "gpuSHA256": sha(GPU)}
    helper, gpu = HELPER.read_text(), GPU.read_text()
    selector = block(helper, "pub fn zero_initialize_workgroup_memory(")
    assert re.search(r"!verified_full_write\s*\|\|\s*!std::env::var_os\(\"MUGUET_DEPTH_REDUCE_NO_ZERO\"\)", selector)
    assert selector.count("std::env::var_os") == 1
    assert 'is_some_and(|v| v == "1")' in selector
    load = block(gpu, "    fn load_map_program(")
    assert "let mut full_write_workgroup = false;" in load
    assert "label == depth_downsample::LABEL && depth_downsample::requested()" in load
    rewrite = block(load, "match depth_downsample::rewrite(")
    success = block(rewrite, "Ok(Some(text)) =>")
    assert "full_write_workgroup = true;" in success
    assert "verified_consts = values;" in success
    assert rewrite.count("full_write_workgroup = true") == 1
    assert "full_write_workgroup" not in block(rewrite, "Ok(None) =>")
    assert "full_write_workgroup" not in block(rewrite, "Err(error) =>")
    assignments = re.findall(r"\bfull_write_workgroup\s*=\s*([^;]+);", gpu)
    assert assignments == ["false", "true"], assignments
    constructors = []
    for m in re.finditer(r"\bProgram\s*\{", gpu):
        if gpu[max(0, m.start() - 8):m.start()].strip().endswith("struct"):
            continue
        constructors.append(block(gpu, "Program", m.start()))
    assert len(constructors) == 2
    assert re.search(r",\s*full_write_workgroup,\s*luminance_full_write\s*}$", constructors[0])
    assert re.search(r"full_write_workgroup:\s*prog\.full_write_workgroup,\s*luminance_full_write:\s*prog\.luminance_full_write\s*}$", constructors[1])
    dispatch = block(gpu, "    fn do_dispatch(")
    assert constructors[1] in dispatch
    expected = "let zero_initialize_workgroup_memory = depth_downsample::zero_initialize_workgroup_memory(cprog.full_write_workgroup)\n                && luminance_init::zero_initialize_workgroup_memory(cprog.luminance_full_write);"
    assert expected in dispatch
    assert "compilation_options: PipelineCompilationOptions { zero_initialize_workgroup_memory, ..Default::default() }" in dispatch
    assert gpu.count("depth_downsample::zero_initialize_workgroup_memory(") == 1
    pipelines = [block(gpu, "ComputePipelineDescriptor", m.start())
                 for m in re.finditer(r"\bComputePipelineDescriptor\s*\{", gpu)]
    assert len(pipelines) == 2
    assert 'label: Some("bc1_astc")' in pipelines[0]
    assert "compilation_options: Default::default()" in pipelines[0]
    assert "compilation_options: PipelineCompilationOptions { zero_initialize_workgroup_memory, ..Default::default() }" in pipelines[1]

    (HOST / "Cargo.toml").write_text('''[package]
name = "depth-zero-selector-host-test"
version = "0.1.0"
edition = "2021"
[lib]
path = "lib.rs"
[dependencies]
regex = "=1.13.1"
sha2 = "=0.10.9"
[workspace]
''')
    rust_path = json.dumps(str(HELPER), ensure_ascii=False)
    (HOST / "lib.rs").write_text('''#[path = PATH]
pub mod actual_production;
#[cfg(test)]
mod selector_tests {
    use super::actual_production as actual;
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;
    const KEY: &str = "MUGUET_DEPTH_REDUCE_NO_ZERO";
    struct Restore(Option<OsString>, Option<OsString>);
    impl Drop for Restore {
        fn drop(&mut self) {
            match &self.0 {Some(v)=>std::env::set_var(KEY,v),None=>std::env::remove_var(KEY)}
            match &self.1 {Some(v)=>std::env::set_var("MUGUET_DEPTH_REDUCE_NO_ZERO_INIT",v),None=>std::env::remove_var("MUGUET_DEPTH_REDUCE_NO_ZERO_INIT")}
        }
    }
    #[test]
    fn actual_selector_matrix() {
        let _restore = Restore(std::env::var_os(KEY),std::env::var_os("MUGUET_DEPTH_REDUCE_NO_ZERO_INIT"));
        let values: Vec<Option<OsString>> = vec![None,Some("".into()),Some("0".into()),Some("1".into()),Some("true".into()),Some("01".into()),Some(" 1".into()),Some("1 ".into()),Some(OsString::from_vec(vec![255]))];
        for value in &values {
            match value {Some(v)=>std::env::set_var(KEY,v),None=>std::env::remove_var(KEY)}
            for verified in [false,true] {
                let expected = !(verified && value.as_ref().is_some_and(|v|v=="1"));
                assert_eq!(actual::zero_initialize_workgroup_memory(verified),expected,
                           "verified={verified}, value={value:?}");
            }
        }
        // The obsolete-looking suffix is deliberately not the production key.
        std::env::remove_var(KEY);
        std::env::set_var("MUGUET_DEPTH_REDUCE_NO_ZERO_INIT","1");
        assert!(actual::zero_initialize_workgroup_memory(true));
    }
    #[test]
    #[ignore = "requires your private MUGUET_DEPTH_SHADER_FIXTURE"]
    fn actual_pinned_rewrite_gate_with_private_fixture() {
        let _restore = Restore(std::env::var_os(KEY),std::env::var_os("MUGUET_DEPTH_REDUCE_NO_ZERO_INIT"));
        let source_path = std::env::var_os("MUGUET_DEPTH_SHADER_FIXTURE").expect("set private fixture path");
        let original = std::fs::read_to_string(source_path).expect("read private depth shader fixture");
        let original_source = original.as_str();
        std::env::set_var(KEY,"1");
        for (label, source, constants) in [
            (actual::LABEL,original_source,actual::CONSTANTS.to_vec()),
            ("tiled_lightingMS::DepthDownsampleCS_Default",original_source,actual::CONSTANTS.to_vec()),
            (actual::LABEL,"",actual::CONSTANTS.to_vec()),
            (actual::LABEL,original_source,vec![1,4,8,12,16,64]),
        ] {
            let verified = actual::rewrite(label,source,&constants).unwrap().is_some();
            let exact = label==actual::LABEL && source==original_source && constants==actual::CONSTANTS;
            assert_eq!(verified,exact);
            assert_eq!(actual::zero_initialize_workgroup_memory(verified),!exact);
        }
    }
}
'''.replace("PATH", rust_path))
    env = dict(os.environ)
    env["CARGO_HOME"] = str(ROOT / "runtime-probe/.toolchain/cargo")
    env["RUSTUP_HOME"] = str(ROOT / "runtime-probe/.toolchain/rustup")
    env["PATH"] = env["CARGO_HOME"] + "/bin:" + env["PATH"]
    env["CARGO_TARGET_DIR"] = str(HOST / "target")
    private_fixture_available = SOURCE.is_file()
    if private_fixture_available:
        env["MUGUET_DEPTH_SHADER_FIXTURE"] = str(SOURCE)
    result = subprocess.run([str(Path(env["CARGO_HOME"]) / "bin/cargo"), "test", "--offline",
                             "--manifest-path", str(HOST / "Cargo.toml"), "--target", "aarch64-apple-darwin",
                             "selector_tests", "--", *(["--include-ignored"] if private_fixture_available else []),
                             "--nocapture", "--test-threads=1"],
                            cwd=ROOT, env=env, text=True, capture_output=True, timeout=120)
    (HOST / "host-tests.log").write_text(result.stdout + result.stderr)
    assert result.returncode == 0, result.stdout + result.stderr
    assert before == {"helperSHA256": sha(HELPER), "gpuSHA256": sha(GPU)}, "production source changed during review"
    report = {"status": "pass", "scope": "Actual complete helper compiled in a small isolated host crate; selector-only runtime tests and explicit production wiring assertions. No game/HAP/shader build or device operations.",
              **before, "productionSourcesStableDuringReview": True,
              "envKey": "MUGUET_DEPTH_REDUCE_NO_ZERO", "exactDisableValue": "1",
              "actualRustCases": {"environmentStates": 9, "verifiedTagStates": 2,
                                   "matrixAssertions": 18, "wrongEnvSuffixAssertions": 1,
                                   "actualRewriteCases": ["exact", "wrong-label", "wrong-source", "wrong-constants"] if private_fixture_available else [],
                                   "privateRewriteFixture": "provided" if private_fixture_available else "not run: private WGSL missing"},
              "sourceWiring": {"programConstructorCount": 2,
                                "defaultFalseUntilExactRewriteSuccess": True,
                                "onlyAssignmentTrueInsideOkSomeText": True,
                                "rejectedOrErrorRewriteCannotTag": True,
                                "constantsSnapshotReused": True,
                                "formatVariantCopiesVerifiedTag": True,
                                "onlyMainDispatchPipelineUsesSelector": True,
                                "bc1AstcOtherComputeDefaultUnchanged": True,
                                "falseTagShortCircuitsBeforeEnvironmentRead": True},
              "runtimeLimit": "Compilation options are selected on pipeline creation; an already cached pipeline is not dynamically recompiled by a later environment change.",
              "hostLog": str((HOST / "host-tests.log").relative_to(ROOT))}
    (OUT / "report.json").write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps({"status":"pass",**before,"report":str(OUT / "report.json")}))

if __name__ == "__main__":
    main()
