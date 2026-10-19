#!/usr/bin/env python3
"""Offline resource WGSL coverage, distinct from actual device shader capture."""
import argparse
from collections import Counter
from concurrent.futures import ThreadPoolExecutor, as_completed
import hashlib
import json
from pathlib import Path
import re
import struct
import subprocess
import time

from compare_spirv_buffer_layouts import layouts


def run(command):
    p = subprocess.run([str(v) for v in command], text=True, capture_output=True, timeout=90)
    return {'exit': p.returncode, 'stdout': p.stdout, 'stderr': p.stderr}


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument('resource_dir', type=Path)
    ap.add_argument('--out', type=Path, required=True)
    ap.add_argument('--workers', type=int, default=6)
    ap.add_argument('--spirv-version', choices=['1.3', '1.4'], default='1.4')
    ap.add_argument('--compare-layout-root', type=Path, help='Prior real 1.4 regeneration output directory')
    ap.add_argument('--compiler', type=Path, default=Path('game-runtime/shader-compiler/target/aarch64-apple-darwin/release/muguet-shader-review'))
    ap.add_argument('--sanitizer', type=Path, default=Path('game-runtime/artifacts/review_spirv_vulkan'))
    ap.add_argument('--validator', type=Path, default=Path('.tools/SPIRV-Tools/build/tools/spirv-val'))
    args = ap.parse_args()
    started = time.monotonic()
    index = json.loads((args.resource_dir / 'index.json').read_text())
    sources = sorted(args.resource_dir.glob('*.wgsl'))
    args.out.mkdir(parents=True, exist_ok=True)
    skips = []
    jobs = []
    for source in sources:
        text = source.read_text()
        entries = re.findall(r'@(vertex|fragment|compute)\b[\s\S]{0,150}?\bfn\s+(\w+)\s*\(', text)
        meta = index.get(source.stem, {})
        job = {'file': str(source), 'resourceHash': source.stem,
               'sourceSHA256': hashlib.sha256(text.encode()).hexdigest(),
               'effect': meta.get('effect'), 'program': meta.get('program'),
               'indexedStage': meta.get('stage'), 'entryPoints': entries,
               'overrideDeclarations': len(re.findall(r'\boverride\s+', text))}
        if len(entries) != 1 or entries[0][1] != 'main':
            skips.append({**job, 'reason': 'entry point is not uniquely stage/main'})
        else:
            job['stage'] = entries[0][0].capitalize()
            jobs.append(job)
    (args.out / 'jobs.json').write_text(json.dumps(jobs, indent=2) + '\n')

    def check(job):
        result = dict(job)
        out = args.out / job['resourceHash']
        out.mkdir(exist_ok=True)
        tag = args.spirv_version.replace('.', '')
        raw, fixed = out / f'naga{tag}.spv', out / f'vulkan{tag}.spv'
        try:
            result['compile'] = run([args.compiler, job['file'], raw, job['stage'], args.spirv_version, 'pack'])
            if result['compile']['exit']:
                result['result'] = 'compile-fail'
                return result
            result['sanitize'] = run([args.sanitizer, raw, fixed])
            if result['sanitize']['exit']:
                result['result'] = 'sanitize-fail'
                return result
            result['sanitizerReport'] = json.loads(result['sanitize']['stdout'])
            result['explicitBufferLayoutsPreserved'] = layouts(raw) == layouts(fixed)
            if args.compare_layout_root:
                comparison = args.compare_layout_root / job['resourceHash'] / 'naga14.spv'
                result['explicitBufferLayoutsMatchReal14Generation'] = layouts(raw) == layouts(comparison)
            data = fixed.read_bytes()
            result['spirvSHA256'] = hashlib.sha256(data).hexdigest()
            result['spirvBytes'] = len(data)
            result['spirvVersionWord'] = struct.unpack_from('<I', data, 4)[0]
            result['validation'] = run([args.validator, '--target-env', 'vulkan1.3', fixed])
            result['result'] = ('pass' if result['validation']['exit'] == 0
                                and result['explicitBufferLayoutsPreserved']
                                and result['spirvVersionWord'] == (0x00010000 | (int(args.spirv_version.split('.')[1]) << 8)) else 'validation-fail')
        except Exception as error:
            result['result'] = 'tool-error'
            result['error'] = repr(error)
        finally:
            (out / 'result.json').write_text(json.dumps(result, indent=2) + '\n')
        return result

    results = []
    with ThreadPoolExecutor(max_workers=args.workers) as pool:
        pending = [pool.submit(check, job) for job in jobs]
        for future in as_completed(pending):
            results.append(future.result())
            if len(results) % 250 == 0 or len(results) == len(jobs):
                print(json.dumps({'completed': len(results), 'total': len(jobs),
                                  'results': dict(Counter(r['result'] for r in results)),
                                  'elapsedSeconds': round(time.monotonic() - started, 1)}), flush=True)
    results.sort(key=lambda r: r['file'])
    counts = dict(Counter(r['result'] for r in results))
    report = {
        'scope': f'Offline regeneration of actual supplied WGSL resources with Naga 30.0.1, same pack_uniforms, main entry point, real SPIR-V writer {args.spirv_version}, default override values, Native workgroup zero initialization, existing exact ArrayStride/SampleMask legalizers and Vulkan 1.3 validation. This does not cover every device pipeline variant or establish vendor execution/gameplay.',
        'resourceDirectory': str(args.resource_dir), 'resourceIndexEntries': len(index),
        'actualWGSLFiles': len(sources), 'jobs': len(jobs), 'excludedEntryPoints': skips,
        'stageCounts': dict(Counter(j['stage'] for j in jobs)),
        'resultCounts': counts, 'elapsedSeconds': round(time.monotonic() - started, 3),
        'allActualWGSLResourcesCovered': len(results) == len(sources) and not skips,
        'allCompiledAndValid': counts.get('pass', 0) == len(sources),
        'allExplicitBufferLayoutsPreserved': all(r.get('explicitBufferLayoutsPreserved', False) for r in results),
        'allExplicitBufferLayoutsMatchReal14Generation': (all(r.get('explicitBufferLayoutsMatchReal14Generation', False) for r in results) if args.compare_layout_root else None),
        'sanitizerTotals': {k: sum(r.get('sanitizerReport', {}).get(k, 0) for r in results)
                            for k in ['arrayStrideRemoved', 'arrayStrideMixedRetained', 'sampleMaskWrapped', 'sampleMaskScalarAccesses']},
        'failures': [r for r in results if r['result'] != 'pass'], 'files': results,
    }
    (args.out / 'report.json').write_text(json.dumps(report, indent=2) + '\n')
    print(json.dumps({k: v for k, v in report.items() if k not in ['files', 'failures', 'excludedEntryPoints']}, indent=2), flush=True)
    return 0 if report['allCompiledAndValid'] else 1


if __name__ == '__main__':
    raise SystemExit(main())
