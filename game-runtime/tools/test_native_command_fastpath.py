#!/usr/bin/env python3
"""Execute the production host helper and strict/frozen startup environment matrix."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import tempfile


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--out', type=Path, required=True)
    args = parser.parse_args()
    source = Path(__file__).resolve().parents[1] / 'src/native_command_fastpath.rs'
    logs = []

    def run(command, env=None):
        result = subprocess.run(command, env=env, text=True, capture_output=True, timeout=120)
        logs.append(result.stdout + result.stderr)
        if result.returncode:
            raise RuntimeError(f'{command[0]} failed: {result.stdout}{result.stderr}')
        return result

    cases = []
    with tempfile.TemporaryDirectory(prefix='native-command-fastpath-') as temporary:
        work = Path(temporary)
        tests = work / 'unit-tests'
        run(['rustc', '--edition=2021', '--test', str(source), '-o', str(tests)])
        unit = run([str(tests)])
        if '6 passed' not in unit.stdout:
            raise RuntimeError('The expected six actual-module tests did not execute')
        driver = work / 'startup.rs'
        driver.write_text('#[path=' + json.dumps(str(source), ensure_ascii=False) + ']\nmod actual;\n' + r'''
fn main() {
    let expected_feature = std::env::args().nth(1).unwrap() == "1";
    let expected_diagnostics = std::env::args().nth(2).unwrap() == "1";
    let selected = actual::StartupOptions::requested();
    assert_eq!(selected.enabled, expected_feature);
    assert_eq!(selected.vertex_diagnostics, expected_diagnostics);
    assert_eq!(selected.should_trace_vertex_bind(), !expected_feature || expected_diagnostics);
    std::env::set_var("MUGUET_NATIVE_COMMAND_FASTPATH", if expected_feature { "0" } else { "1" });
    std::env::set_var("MUGUET_GPU_DIAGNOSTICS", if expected_diagnostics { "0" } else { "1" });
    assert_eq!(actual::StartupOptions::requested(), selected);
}
''')
        binary = work / 'startup'
        run(['rustc', '--edition=2021', '-A', 'dead_code', str(driver), '-o', str(binary)])
        for feature in [None, '0', '1', 'true', '01', '']:
            for diagnostics in [None, '0', '1', 'true', '01', '']:
                env = dict(os.environ)
                for name, value in [('MUGUET_NATIVE_COMMAND_FASTPATH', feature),
                                    ('MUGUET_GPU_DIAGNOSTICS', diagnostics)]:
                    env.pop(name, None)
                    if value is not None:
                        env[name] = value
                run([str(binary), '1' if feature == '1' else '0',
                     '1' if diagnostics == '1' else '0'], env)
                cases.append({'feature': feature, 'diagnostics': diagnostics,
                              'enabled': feature == '1', 'vertexDiagnostics': diagnostics == '1',
                              'frozenAfterBothEnvironmentMutations': True})
    report = {
        'allPassed': True,
        'source': str(source),
        'sourceSha256': hashlib.sha256(source.read_bytes()).hexdigest(),
        'actualModuleTests': 6,
        'unitTestOutput': unit.stdout,
        'startupChildProcesses': len(cases),
        'startupCases': cases,
        'method': 'rustc --test executes actual production helper. A thread-local counting '
                  'allocator measures constructor requests. Every environment case uses a '
                  'fresh child that includes the same actual module.',
        'scope': 'Complete37word pass-key equality, fallback and host allocations/startup '
                 'guards only. No GPU integration/pixel equivalence or device performance claim.',
    }
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(json.dumps(report, indent=2) + '\n')
    args.out.with_suffix('.log').write_text('\n'.join(logs))
    print(json.dumps({'allPassed': True, 'actualModuleTests': 6,
                      'startupChildProcesses': len(cases), 'report': str(args.out)}))


if __name__ == '__main__':
    main()
