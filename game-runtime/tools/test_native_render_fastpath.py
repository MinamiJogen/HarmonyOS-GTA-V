#!/usr/bin/env python3
"""Run the production fastpath module and isolated startup-flag checks."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import tempfile


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument('--out', type=Path, required=True)
    args = ap.parse_args()
    source = Path(__file__).resolve().parents[1] / 'src/native_render_fastpath.rs'
    env = dict(os.environ)
    logs = []
    cases = []

    def run(command, case_env=None):
        result = subprocess.run(command, env=case_env or env, text=True,
                                capture_output=True, timeout=120)
        logs.append(result.stdout + result.stderr)
        if result.returncode:
            raise RuntimeError(f'{command[0]} failed: {result.stdout}{result.stderr}')
        return result

    with tempfile.TemporaryDirectory(prefix='native-render-fastpath-') as temporary:
        work = Path(temporary)
        tests = work / 'unit-tests'
        run(['rustc', '--edition=2021', '--test', str(source), '-o', str(tests)])
        unit = run([str(tests)])
        driver = work / 'startup.rs'
        driver.write_text('#[path = ' + json.dumps(str(source), ensure_ascii=False) + ']\nmod actual;\n' + r'''
fn main() {
    let expected=std::env::args().nth(1).unwrap()=="1";
    assert_eq!(actual::requested(),expected);
    // A hot config/environment edit is not allowed to change this process's
    // policy halfway through a graph; A/B always starts a new process.
    std::env::set_var("MUGUET_NATIVE_RENDER_FASTPATH",if expected{"0"}else{"1"});
    assert_eq!(actual::requested(),expected);
}
''')
        startup = work / 'startup'
        run(['rustc', '--edition=2021', '-A', 'dead_code', str(driver), '-o', str(startup)])
        for value in [None, '0', '1', 'true', '01', '']:
            isolated = dict(env)
            isolated.pop('MUGUET_NATIVE_RENDER_FASTPATH', None)
            if value is not None:
                isolated['MUGUET_NATIVE_RENDER_FASTPATH'] = value
            run([str(startup), '1' if value == '1' else '0'], isolated)
            cases.append({'environmentValue': value, 'enabled': value == '1',
                          'stableAfterEnvironmentMutation': True, 'exitCode': 0})
    report = {
        'method': 'rustc --test executes the actual production module. Six fresh processes '
                  'include that exact source and verify strict default-OFF startup selection '
                  'and OnceLock immutability. No mirrored cache/policy implementation.',
        'source': str(source),
        'sourceSha256': hashlib.sha256(source.read_bytes()).hexdigest(),
        'unitTestOutput': unit.stdout,
        'startupCases': cases,
        'allPassed': True,
        'scope': 'Host allocation-policy, key isolation, cache bounds/lifetimes and factory '
                 'ordering only; no Vulkan/SR pixel equivalence or device FPS claim.',
    }
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(json.dumps(report, indent=2) + '\n')
    args.out.with_suffix('.log').write_text('\n'.join(logs))
    print(json.dumps({'allPassed': True, 'startupCases': len(cases),
                      'report': str(args.out)}, indent=2))


if __name__ == '__main__':
    main()
