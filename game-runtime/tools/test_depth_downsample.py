#!/usr/bin/env python3
"""Compile the actual pinned rewrite helper alone; validate both shader outputs.

Only creates isolated host-test/evidence artifacts. Never builds the game crate,
changes its Cargo manifest, installs anything, or contacts a device.
Requires your private WGSL at MUGUET_DEPTH_SHADER_FIXTURE, or the legacy local
resources/game/b/8b0b5899ed/shaders/23a7a3c1c0d38f46.wgsl path.
"""
import collections
import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import re
import struct
import subprocess

ROOT = Path(__file__).resolve().parents[2]
EVIDENCE = ROOT / "native-platform/evidence/depth-reduction-optimization"
OUT = EVIDENCE
HOST = ROOT / "game-runtime/logs/depth-downsample-host"
HELPER = ROOT / "game-runtime/src/depth_downsample.rs"
SOURCE = Path(os.environ.get("MUGUET_DEPTH_SHADER_FIXTURE", str(ROOT / "resources/game/b/8b0b5899ed/shaders/23a7a3c1c0d38f46.wgsl"))).resolve()
COMPILER = ROOT / "game-runtime/shader-compiler/target/aarch64-apple-darwin/release/muguet-image-bounds-review"
SANITIZER = ROOT / "game-runtime/artifacts/review_spirv_vulkan"
VALIDATOR = ROOT / ".tools/SPIRV-Tools/build/tools/spirv-val"
DIS = VALIDATOR.with_name("spirv-dis")
CAPTURED_SHA = "ba4620d7c08afb24d974d3e42e0c2f53b3363e0cb8ce0e6fe72ce6f7241c4006"

def sha(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()

def run(command, log, env=None):
    result = subprocess.run([str(v) for v in command], cwd=ROOT, env=env,
                            text=True, capture_output=True, timeout=180)
    log.write_text(result.stdout + result.stderr)
    if result.returncode:
        raise RuntimeError(f"exit {result.returncode}; inspect {log}")
    return {"exit": result.returncode, "log": str(log.relative_to(ROOT))}

def interface(path):
    """Canonical full descriptor interface, independent of result IDs."""
    raw = Path(path).read_bytes()
    words = struct.unpack("<" + "I" * (len(raw) // 4), raw)
    types, constants, dec, mem, variables = {}, {}, {}, {}, []
    pos = 5
    while pos < len(words):
        count, opcode = words[pos] >> 16, words[pos] & 65535
        assert count
        a = words[pos + 1:pos + count]
        if opcode in [19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 32]:
            types[a[0]] = (opcode, list(a[1:]))
        elif opcode == 43:
            constants[a[1]] = list(a[2:])
        elif opcode == 59:
            variables.append(a)
        elif opcode == 71:
            dec.setdefault(a[0], {})[a[1]] = list(a[2:])
        elif opcode == 72:
            mem.setdefault((a[0], a[1]), {})[a[2]] = list(a[3:])
        pos += count

    def signature(t):
        op, a = types[t]
        if op in [19, 20, 21, 22, 26]:
            return [op, a]
        if op in [23, 24, 25, 27]:
            return [op, signature(a[0]), *a[1:]]
        if op == 28:
            return [op, signature(a[0]), constants[a[1]], dec.get(t, {}).get(6)]
        if op == 29:
            return [op, signature(a[0]), dec.get(t, {}).get(6)]
        if op == 30:
            return [op, {str(k): v for k, v in dec.get(t, {}).items() if k in [2, 3]},
                    [[signature(member), {str(k): v for k, v in mem.get((t, i), {}).items()
                                          if k in [4, 5, 7, 35]}]
                     for i, member in enumerate(a)]]
        if op == 32:
            return [op, a[0], signature(a[1])]
        raise AssertionError((op, a))

    descriptors = []
    for t, v, storage, *_ in variables:
        d = dec.get(v, {})
        if 33 in d and 34 in d:
            descriptors.append({"set": d[34], "binding": d[33], "storage": storage,
                                "type": signature(t)})
    return sorted(descriptors, key=lambda r: (r["set"], r["binding"]))

def instructions(path, out):
    run([DIS, path, "-o", out], out.with_suffix(".dis.log"))
    text = out.read_text()
    counts = collections.Counter(re.findall(r"\b(Op\w+)\b", text))
    values = {i: int(v) for i, v in re.findall(r"(%\S+) = OpConstant %\S+ (\d+)", text)}
    barriers = [[values[i] for i in row.split()]
                for row in re.findall(r"OpControlBarrier (.*)", text)]
    return {"opCounts": dict(sorted(counts.items())), "controlBarriers": barriers,
            "localSizes": re.findall(r"OpExecutionMode %\S+ LocalSize (.*)", text),
            "versionWord": struct.unpack_from("<I", Path(path).read_bytes(), 4)[0]}

def main():
    global OUT, HOST
    parser = argparse.ArgumentParser()
    parser.add_argument("--variant", choices=["v1", "v2"], default="v1")
    args = parser.parse_args()
    if not SOURCE.is_file():
        parser.error("private depth shader missing; set MUGUET_DEPTH_SHADER_FIXTURE to your pinned WGSL")
    is_v2 = args.variant == "v2"
    if is_v2:
        OUT = EVIDENCE / "v2"
        HOST = ROOT / "game-runtime/logs/depth-downsample-host-v2"
    OUT.mkdir(parents=True, exist_ok=True)
    HOST.mkdir(parents=True, exist_ok=True)
    before = sha(HELPER)
    (HOST / "Cargo.toml").write_text('''[package]
name = "depth-downsample-helper-host-test"
version = "0.1.0"
edition = "2021"
[lib]
path = "lib.rs"
[dependencies]
regex = "=1.13.1"
sha2 = "=0.10.9"
[workspace]
''')
    rust_path = lambda p: json.dumps(str(p), ensure_ascii=False)
    (HOST / "lib.rs").write_text(f'''#[path = {rust_path(HELPER)}]
pub mod actual_production;
#[test]
fn export_actual_rewrite_for_shader_validation() {{
    let original = std::fs::read_to_string({rust_path(SOURCE)}).unwrap();
    let text = actual_production::rewrite(actual_production::LABEL,
        &original, &actual_production::CONSTANTS)
        .unwrap().expect("the pinned production source must rewrite");
    std::fs::write({rust_path(OUT / 'optimized.wgsl')}, text).unwrap();
}}
''')
    env = dict(os.environ)
    env["CARGO_HOME"] = str(ROOT / "runtime-probe/.toolchain/cargo")
    env["RUSTUP_HOME"] = str(ROOT / "runtime-probe/.toolchain/rustup")
    env["PATH"] = env["CARGO_HOME"] + "/bin:" + env["PATH"]
    env["CARGO_TARGET_DIR"] = str(HOST / "target")
    env["MUGUET_DEPTH_SHADER_FIXTURE"] = str(SOURCE)
    host_test = run([Path(env["CARGO_HOME"]) / "bin/cargo", "test", "--offline",
                     "--manifest-path", HOST / "Cargo.toml", "--target", "aarch64-apple-darwin",
                     "--", "--include-ignored", "--nocapture"], HOST / "host-tests.log", env)
    assert sha(HELPER) == before, "helper changed while tests ran; repeat against final source"
    # Keep the original byte-for-byte CRLF source. Its SHA is the production gate.
    (OUT / "original.wgsl").write_bytes(SOURCE.read_bytes())
    original_wgsl = SOURCE.read_text()
    optimized_wgsl = (OUT / "optimized.wgsl").read_text()
    declarations = lambda s: re.findall(r"@group\((\d+)u?\)\s*@binding\((\d+)u?\)\s*var(?:<([^>]*)>)?\s+(\w+)\s*:\s*([^;]+);", s)
    assert declarations(original_wgsl) == declarations(optimized_wgsl)
    if is_v2:
        assert optimized_wgsl.count("workgroupBarrier();") == 9
        assert "loop {" not in optimized_wgsl
        assert "v.tint_symbol_2[" not in optimized_wgsl
        assert "bitcast<f32>(1u)" in optimized_wgsl
    variants = {}
    for texture_format in ["rgba32float", "rgba16float"]:
        variants[texture_format] = {}
        for which in ["original", "optimized"]:
            # rgba16 is the actual observed runtime storage-format variant.
            raw_source = (OUT / f"{which}.wgsl").read_bytes()
            assert raw_source.count(b"texture_storage_2d<rgba32float, write>") == 1
            source = OUT / f"{which}-{texture_format}.wgsl"
            source.write_bytes(raw_source.replace(b"texture_storage_2d<rgba32float, write>",
                               f"texture_storage_2d<{texture_format}, write>".encode()))
            variants[texture_format][which] = {}
            for image_policy in ["unchecked", "restrict"]:
                stem = f"{which}-{texture_format}-{image_policy}"
                raw_spv, final_spv = OUT / f"{stem}-naga13.spv", OUT / f"{stem}-vulkan13.spv"
                compile_result = run([COMPILER, source, raw_spv, "Compute", "1.3", "pack", image_policy],
                                     OUT / f"{stem}-compile.log")
                sanitize = run([SANITIZER, raw_spv, final_spv], OUT / f"{stem}-sanitize.log")
                validation = run([VALIDATOR, "--target-env", "vulkan1.3", final_spv],
                                 OUT / f"{stem}-validate.log")
                assert interface(raw_spv) == interface(final_spv)
                ins = instructions(final_spv, OUT / f"{stem}.spvasm")
                assert ins["versionWord"] == 0x00010300
                variants[texture_format][which][image_policy] = {
                    "compile": compile_result, "sanitize": sanitize, "validation": validation,
                    "finalSHA256": sha(final_spv), "finalBytes": final_spv.stat().st_size,
                    "descriptorInterface": interface(final_spv),
                    "legalizersPreserveAllDescriptors": True, **ins}

    comparisons = []
    for texture_format, v in variants.items():
        for policy in ["unchecked", "restrict"]:
            a, b = v["original"][policy], v["optimized"][policy]
            ao, bo = a["opCounts"], b["opCounts"]
            removed = []
            if is_v2:
                removed = [d for d in a["descriptorInterface"] if d["set"] == [0] and d["binding"] == [1]]
                assert len(removed) == 1 and removed[0]["storage"] == 2
                assert [d for d in a["descriptorInterface"] if d not in removed] == b["descriptorInterface"]
                assert a["controlBarriers"] == [[2, 2, 264]] * 2
                assert b["controlBarriers"] == [[2, 2, 264]] * 9
                assert ao.get("OpLoopMerge", 0) == 1 and bo.get("OpLoopMerge", 0) == 0
            else:
                assert a["descriptorInterface"] == b["descriptorInterface"]
                assert a["controlBarriers"] == b["controlBarriers"] == [[2, 2, 264], [2, 2, 264]]
            assert a["localSizes"] == b["localSizes"] == ["16 16 1"]
            assert sum(count for op, count in bo.items() if op.startswith("OpAtomic")) == 0
            assert sum(count for op, count in ao.items() if op.startswith("OpAtomic")) == 24
            for name in ["OpImageFetch", "OpImageWrite", "OpImageQuerySizeLod", "OpImageQueryLevels"] + ([] if is_v2 else ["OpLoopMerge"]):
                assert ao.get(name, 0) == bo.get(name, 0)
            float_ops = sorted(n for n in set(ao) | set(bo) if n.startswith("OpF") or re.match(r"OpConvert(FTo|[SU]ToF)", n))
            assert {n: ao.get(n, 0) for n in float_ops} == {n: bo.get(n, 0) for n in float_ops}
            # Ordinary destination read and write each retain a Restrict guard:
            # five more workgroup clamps, plus two unsigned reduction minima.
            # Distinguish these from the three actual unsigned reduction maxima.
            def glsl_counts(which, op):
                assembly = (OUT / f"{which}-{texture_format}-{policy}.spvasm").read_text()
                limit_ids = {i for i, value in re.findall(r"(%\S+) = OpConstant %\S+ (\d+)", assembly)
                             if int(value) == 1535}
                rows = re.findall(rf"OpExtInst %\S+ %\S+ {op}\b ([^\n]*)", assembly)
                clamps = sum(row.split()[-1] in limit_ids for row in rows)
                return {"total": len(rows), "workgroupRestrictClamps": clamps,
                        "other": len(rows) - clamps}
            min_a, min_b = glsl_counts("original", "UMin"), glsl_counts("optimized", "UMin")
            max_a, max_b = glsl_counts("original", "UMax"), glsl_counts("optimized", "UMax")
            assert min_b["other"] - min_a["other"] == (16 if is_v2 else 2)
            if not is_v2:
                assert min_b["workgroupRestrictClamps"] - min_a["workgroupRestrictClamps"] == 5
            assert min_b["workgroupRestrictClamps"] >= min_a["workgroupRestrictClamps"]
            assert max_b["total"] - max_a["total"] == (24 if is_v2 else 3)
            comparisons.append({"storageFormat": texture_format, "imagePolicy": policy,
                                "allDeclaredWGSLHostBindingsEqual": True,
                                "allReachableDescriptorInterfacesEqual": not removed,
                                "onlyRemovedReachableDescriptor": removed,
                                "allOtherDescriptorLayoutsIdentical": True,
                                "barrierOperandsEqual": True, "localSizeEqual": True,
                                "originalStaticBarriers": 2, "originalDynamicBarriers": 9,
                                "optimizedStaticBarriers": 9 if is_v2 else 2,
                                "optimizedDynamicBarriers": 9,
                                "unrolledReductionHasNoLoop": is_v2,
                                "atomicOpsBefore": 24, "atomicOpsAfter": 0,
                                "newStaticUnsignedUMin": 16 if is_v2 else 2,
                                "newStaticUnsignedUMax": 24 if is_v2 else 3,
                                "newStaticWorkgroupRestrictClamps": min_b["workgroupRestrictClamps"] - min_a["workgroupRestrictClamps"],
                                "UMinCounts": {"original": min_a, "optimized": min_b},
                                "UMaxCounts": {"original": max_a, "optimized": max_b},
                                "imageOperationCountsEqual": True,
                                "floatArithmeticAndComparisonOperationCountsEqual": {n: ao.get(n, 0) for n in float_ops},
                                "note": "Bounds policy remains Restrict for index and buffer in both variants; image policy is separately recorded."})
    actual_reproduced = variants["rgba16float"]["original"]["unchecked"]["finalSHA256"] == CAPTURED_SHA
    report = {"status": "pass", "variant": args.variant, "scope": "Actual complete production helper isolated host unit tests and offline Naga30.0.1 regeneration; no GPU execution, timing, game build, device operation, or production source change",
              "helperSHA256": before, "helperUnchangedDuringRun": sha(HELPER) == before,
              "originalWGSLRawSHA256": sha(SOURCE), "optimizedWGSLRawSHA256": sha(OUT / "optimized.wgsl"),
              "hostTests": host_test, "hostOnlyDependencies": ["regex=1.13.1", "sha2=0.10.9"],
              "compilerSHA256": sha(COMPILER), "sanitizerSHA256": sha(SANITIZER),
              "actualCapturedOriginalSPVByteExactReproduced": actual_reproduced,
              "actualCapturedOriginalSPVSHA256": CAPTURED_SHA,
              "comparisons": comparisons, "variants": variants}
    report_path = EVIDENCE / ("actual-helper-shader-validation-v2.json" if is_v2 else "actual-helper-shader-validation.json")
    report_path.write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps({"status": report["status"], "comparisons": len(comparisons),
                      "actualCapturedOriginalSPVByteExactReproduced": actual_reproduced,
                      "report": str(report_path)}))

if __name__ == "__main__":
    main()
