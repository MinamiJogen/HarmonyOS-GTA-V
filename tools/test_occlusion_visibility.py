#!/usr/bin/env python3
"""Compile the actual occlusion aggregation loops and real bounds helpers.

The only substituted leaves are memory_violation (panic only, no ffi/file I/O)
and query_store (records publications rather than writing Wasmtime atomics).
No GPU, device, package, benchmark tool or production source is modified.
"""
import argparse
from datetime import datetime, timezone
import hashlib
import json
from pathlib import Path
import shutil
import subprocess
import tempfile


def sha(data):
    return hashlib.sha256(data).hexdigest()


def block(source, marker):
    if source.count(marker) != 1:
        raise ValueError(f'expected one source marker: {marker}')
    start = source.index(marker)
    brace = source.index('{', start)
    depth = 1
    at = brace + 1
    # These narrowly extracted production blocks contain no braces in their
    # string literals. Fail rather than silently substituting a mirrored loop.
    while depth:
        depth += (source[at] == '{') - (source[at] == '}')
        at += 1
    return source[start:at]


TESTS = r'''
#[cfg(test)]
mod occlusion_visibility_tests {
    use super::*;
    const OBSERVED: [u64; 2] = [13853757725453407890, 4681940450981080896];
    fn bytes(values: &[u64], consecutive: bool, prefix: usize) -> Vec<u8> {
        let stride = if consecutive { 8 } else { 256 };
        let len = if values.is_empty() { prefix } else { prefix + (values.len()-1)*stride+8 };
        let mut data = vec![0xab; len];
        for (i, value) in values.iter().enumerate() {
            data[prefix+i*stride..prefix+i*stride+8].copy_from_slice(&value.to_le_bytes());
        }
        data
    }
    fn result(values: &[u64], consecutive: bool) -> Vec<(u64,u64,u32)> {
        let data = bytes(values, consecutive, 16);
        let mut sink = Sink::default();
        sink.current(&data, &[(16, values.len() as u32, 0x400, consecutive, 0x1234)]);
        sink.publications
    }
    fn panic_text(f: impl FnOnce()) -> String {
        let error = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).unwrap_err();
        if let Some(text) = error.downcast_ref::<String>() { text.clone() }
        else { error.downcast_ref::<&str>().unwrap().to_string() }
    }
    #[test]
    fn zero_and_empty_publish_zero_for_both_layouts() {
        for consecutive in [true, false] {
            for values in [&[][..], &[0][..], &[0,0,0][..]] {
                assert_eq!(result(values, consecutive), [(0x400,0,0x1234)]);
            }
        }
    }
    #[test]
    fn any_nonzero_in_every_position_is_visible_for_both_layouts() {
        for consecutive in [true, false] {
            for value in [1, 65536, 1 << 63, u64::MAX, OBSERVED[0], OBSERVED[1]] {
                for position in 0..3 {
                    let mut values = [0;3]; values[position] = value;
                    assert_eq!(result(&values, consecutive), [(0x400,65536,0x1234)]);
                }
            }
        }
    }
    #[test]
    fn observed_operands_reproduce_old_checked_sum_failure_and_new_success() {
        assert!(OBSERVED[0].checked_add(OBSERVED[1]).is_none());
        for consecutive in [true, false] {
            let data = bytes(&OBSERVED, consecutive, 16);
            let mut old = Sink::default();
            let message = panic_text(|| old.previous(&data, &[(16,2,0x400,consecutive,0x1234)]));
            assert!(message.contains("occlusion result sum: addition overflow 13853757725453407890+4681940450981080896"), "{message}");
            assert!(old.publications.is_empty());
            assert_eq!(result(&OBSERVED, consecutive), [(0x400,65536,0x1234)]);
        }
    }
    #[test]
    fn max_u64_values_do_not_wrap_or_clamp_and_still_publish_boolean() {
        for consecutive in [true, false] {
            assert_eq!(result(&[u64::MAX,u64::MAX,0,u64::MAX], consecutive), [(0x400,65536,0x1234)]);
        }
    }
    #[test]
    fn visible_first_slot_does_not_skip_later_consecutive_bounds() {
        let data = bytes(&[1], true, 0);
        let mut sink = Sink::default();
        let message = panic_text(|| sink.current(&data, &[(0,2,0x400,true,7)]));
        assert!(message.contains("occlusion mapped result: out of bounds"), "{message}");
        assert!(sink.publications.is_empty());
    }
    #[test]
    fn visible_first_slot_does_not_skip_later_sparse_bounds() {
        let mut data = vec![0;256]; data[..8].copy_from_slice(&u64::MAX.to_le_bytes());
        let mut sink = Sink::default();
        let message = panic_text(|| sink.current(&data, &[(0,2,0x400,false,7)]));
        assert!(message.contains("occlusion mapped result: out of bounds address=0x100"), "{message}");
        assert!(sink.publications.is_empty());
    }
    #[test]
    fn alignment_and_address_span_fail_before_any_guest_publication() {
        for byte in [1, u64::MAX-7] {
            let mut sink = Sink::default();
            let message = panic_text(|| sink.current(&[0;16], &[(byte,1,0x400,true,7)]));
            assert!(message.contains("occlusion mapped result:"), "{message}");
            assert!(sink.publications.is_empty());
        }
    }
    #[test]
    fn multiple_queries_preserve_publication_order_addresses_and_seq_cast() {
        let data = bytes(&[0, u64::MAX, 0], true, 0);
        let mut sink = Sink::default();
        sink.current(&data, &[(0,1,0x400,true,1),(8,2,0x600,true,u32::MAX as usize+1)]);
        assert_eq!(sink.publications, [(0x400,0,1),(0x600,65536,0)]);
        // A malformed later query cannot publish its result after an earlier
        // valid result. This records, rather than strengthens, existing order.
        let message = panic_text(|| sink.current(&data, &[(0,1,0x800,true,2),(8,3,0xa00,true,3)]));
        assert!(message.contains("occlusion mapped result: out of bounds"), "{message}");
        assert_eq!(sink.publications.last(), Some(&(0x800,0,2)));
        assert_eq!(sink.publications.len(), 3);
    }
    #[test]
    fn nonoverflowing_old_results_match_new_boolean_for_1024_cases() {
        let mut seed = 0x01916d3ab5u64;
        for n in 0..1024 {
            let mut values = [0;4];
            for value in &mut values {
                seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
                *value = if n % 7 == 0 { 0 } else { seed & 0xffffffff };
            }
            let consecutive = n % 2 == 0;
            let data = bytes(&values, consecutive, 16);
            let list = [(16,4,0x400,consecutive,n)];
            let mut old = Sink::default(); let mut new = Sink::default();
            old.previous(&data, &list); new.current(&data, &list);
            assert_eq!(old.publications,new.publications);
        }
    }
}
'''


def main():
    root = Path(__file__).resolve().parents[1]
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, default=root / 'evidence/native-30fps-800p-20261021/occlusion-visibility-fix')
    args = parser.parse_args()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    gpu = root / 'game-runtime/src/gpu.rs'
    before = root / 'evidence/native-30fps-800p-20261021/occlusion-visibility-fix/gpu.rs.before'
    memory = root / 'game-runtime/src/gpu_memory.rs'
    inputs = {str(p.relative_to(root)): sha(p.read_bytes()) for p in (gpu,before,memory)}
    marker = 'for &(byte, cnt, addr, consecutive, seq) in list {'
    current = block(gpu.read_text(), marker)
    previous = block(before.read_text(), marker)
    checked = block(gpu.read_text(), 'fn checked_value<T>(')
    if 'visible |=' not in current or 'occlusion result sum' not in previous:
        raise ValueError('aggregation source does not match visibility fix and old failure')
    executable = '\n'.join(line.split('//',1)[0] for line in current.splitlines())
    if 'break;' in executable or 'return ' in executable or '||' in executable:
        raise ValueError('possible short circuit added to checked production loop')
    common_checks = [line.strip() for line in previous.splitlines() if 'let offset =' in line or 'let at =' in line or 'let source =' in line]
    if common_checks != [line.strip() for line in current.splitlines() if 'let offset =' in line or 'let at =' in line or 'let source =' in line]:
        raise ValueError('production slot address/bounds checks changed')
    harness = '\n'.join([
        '#[path = ' + json.dumps(str(memory),ensure_ascii=False) + '] mod memory_checks;',
        'fn memory_violation(context: &str, detail: impl std::fmt::Display) -> ! { panic!("{context}: {detail}") }',
        checked,
        'const OCCLUSION_VISIBLE_COUNT: u64 = 65536;',
        '#[derive(Default)] struct Sink { publications: Vec<(u64,u64,u32)> }',
        'impl Sink { fn query_store(&mut self, addr:u64, result:u64, seq:u32) { self.publications.push((addr,result,seq)); }',
        'fn current(&mut self, range:&[u8], list:&[(u64,u32,u64,bool,usize)]) {', current, '}',
        'fn previous(&mut self, range:&[u8], list:&[(u64,u32,u64,bool,usize)]) {', previous, '}', '}', TESTS,
    ])
    fixture = output / 'actual-loop-harness.rs'
    fixture.write_text(harness)
    rustc = shutil.which('rustc')
    if not rustc:
        raise RuntimeError('rustc unavailable')
    with tempfile.TemporaryDirectory(prefix='gta-occlusion-visibility-') as temp:
        binary = Path(temp) / 'actual-loop-tests'
        commands = [[rustc,'--edition=2021','--test',str(fixture),'-o',str(binary)],
                    [str(binary),'occlusion_visibility_tests','--test-threads=1']]
        log_parts = []
        codes = []
        for command in commands:
            result = subprocess.run(command,text=True,capture_output=True)
            codes.append(result.returncode)
            log_parts.append('$ ' + ' '.join(command) + '\n' + result.stdout + result.stderr)
            if result.returncode:
                break
        log = output / 'actual-loop-tests.log'
        log.write_text('\n'.join(log_parts))
    unchanged = inputs == {str(p.relative_to(root)): sha(p.read_bytes()) for p in (gpu,before,memory)}
    passed = codes == [0,0] and unchanged and '9 passed; 0 failed' in log.read_text()
    report = {
        'generatedAt': datetime.now(timezone.utc).isoformat(),
        'passed': passed, 'executedRegressionTests': 9 if passed else None,
        'differentialNonoverflowCases': 1024 if passed else None,
        'inputs': inputs, 'sourceUnchangedDuringRun': unchanged,
        'extractedSourceSHA256': {'newLoop':sha(current.encode()),'previousLoop':sha(previous.encode()),'checkedValue':sha(checked.encode())},
        'preservedSlotChecks': common_checks,
        'commands': commands, 'exitCodes': codes,
        'artifacts': {p.name:sha(p.read_bytes()) for p in (fixture,log)},
        'scope': 'Host execution of byte-extracted production aggregation loops, production checked_value and complete production gpu_memory module.',
        'substitutedLeaves': {'memory_violation':'panic only; no ffi state or support file I/O','query_store':'records addr/result/seq; no Wasmtime guest atomics or notify'},
        'limits': ['No GPU/query availability simulation, Vulkan/driver execution, complete map/unmap lifecycle or device regression.',
                   'No claim that the observed nonzero operands prove driver correctness; nonprecise query semantics allow their magnitude.',
                   'Five embedded gpu_memory unit tests compile but are intentionally not selected; nine focused extracted-loop tests execute.'],
    }
    report_path = output / 'actual-loop-validation.json'
    report_path.write_text(json.dumps(report,indent=2)+'\n')
    print(json.dumps({'passed':passed,'tests':report['executedRegressionTests'],'validation':str(report_path),'sha256':sha(report_path.read_bytes())}))
    if not passed:
        raise SystemExit(1)


if __name__ == '__main__':
    main()
