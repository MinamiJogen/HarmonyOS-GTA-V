#!/usr/bin/env python3
"""Compile both production pool-create-info snippets with the real ash ABI."""
from pathlib import Path
import hashlib
import json
import os
import subprocess

root = Path(__file__).resolve().parents[1]
command = (root / 'vendor/wgpu-hal/src/vulkan/command.rs').read_text()
device = (root / 'vendor/wgpu-hal/src/vulkan/device.rs').read_text()
start = device.index('        let pool_flags = ', device.index('unsafe fn create_command_encoder('))
initial = device[start:device.index('        let raw = unsafe {', start)]
start = command.index('            let info = vk::CommandPoolCreateInfo::default()', command.index('unsafe fn begin_encoding('))
fallback = command[start:command.index('            self.raw = unsafe {', start)]
assert '.reset_command_buffer(' not in command, 'pool-only policy cannot individually reset buffers'
assert 'RESET_COMMAND_BUFFER' not in initial and 'RESET_COMMAND_BUFFER' not in fallback
assert 'const ALLOCATION_GRANULARITY: u32 = 16;' in command
assert '.flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT)' in command
source = r'''
use ash::vk;
struct Queue { family_index: u32 }
struct Desc { queue: Queue }
fn initial(family: u32)->vk::CommandPoolCreateInfo<'static> {
    let desc = Desc {queue: Queue {family_index: family}};
INITIAL
    vk_info
}
struct Encoder {queue_family_index: u32}
impl Encoder {
    fn recreated(&self)->vk::CommandPoolCreateInfo<'static> {
FALLBACK
        info
    }
}
fn check(info:vk::CommandPoolCreateInfo<'static>,family:u32) {
    assert_eq!(info.s_type,vk::StructureType::COMMAND_POOL_CREATE_INFO);
    assert!(info.p_next.is_null());
    assert_eq!(info.flags,vk::CommandPoolCreateFlags::TRANSIENT);
    assert!(!info.flags.contains(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER));
    assert_eq!(info.queue_family_index,family);
}
#[test]fn initial_pool_is_transient_only_for_both_queue_families(){for family in [0,7]{check(initial(family),family);}}
#[test]fn recreated_pool_is_transient_only_and_matches_initial(){for family in [0,7]{let info=Encoder{queue_family_index:family}.recreated();check(info,family);assert_eq!(info.flags,initial(family).flags);}}
'''.replace('INITIAL', initial).replace('FALLBACK', fallback)
out = root / 'logs/transient-command-pool-flags-model'
out.mkdir(exist_ok=True)
rs = out / 'actual-pool-create-info-tests.rs'
rs.write_text(source)
deps = root / 'logs/device-fault-collector-model/target/debug/deps'
ash = next(deps.glob('libash-*.rlib'))
env = dict(os.environ)
env['CARGO_HOME'] = str(root.parent / 'runtime-probe/.toolchain/cargo')
env['RUSTUP_HOME'] = str(root.parent / 'runtime-probe/.toolchain/rustup')
env['PATH'] = env['CARGO_HOME'] + '/bin:' + env.get('PATH', '')
cases, logs = [], []
for mode in ['ohos', 'host']:
    binary = out / ('tests-' + mode)
    args = ['rustc', '--edition=2021', '--test', str(rs), '--extern', 'ash=' + str(ash), '-L', 'dependency=' + str(deps), '-o', str(binary)]
    if mode == 'ohos':
        args += ['-Aexplicit_builtin_cfgs_in_flags', '--cfg', 'target_env="ohos"']
    build = subprocess.run(args, env=env, text=True, capture_output=True)
    logs += [build.stdout, build.stderr]
    if build.returncode:
        cases.append({'mode': mode, 'buildExit': build.returncode})
        continue
    run = subprocess.run([str(binary)], text=True, capture_output=True)
    logs += [run.stdout, run.stderr]
    cases.append({'mode': mode, 'buildExit': 0, 'testsExit': run.returncode})
(root / 'logs/transient-command-pool-flags-tests.log').write_text(''.join(logs))
report = {
    'method': 'Both actual production pool-create-info snippets extracted verbatim, real cached ash0.38 structs, OHOS and host cfg. No driver execution.',
    'sourceSHA256': {'command.rs': hashlib.sha256(command.encode()).hexdigest(), 'device.rs': hashlib.sha256(device.encode()).hexdigest()},
    'extractedSHA256': {'initial': hashlib.sha256(initial.encode()).hexdigest(), 'fallback': hashlib.sha256(fallback.encode()).hexdigest()},
    'stateAssertions': ['No production per-command-buffer reset call in command.rs', 'Both pool creation paths exclude RESET_COMMAND_BUFFER', 'Allocation granularity remains16', 'Recording retains ONE_TIME_SUBMIT'],
    'cases': cases,
    'allPassed': len(cases) == 2 and all(c.get('testsExit') == 0 for c in cases),
}
(out / 'report.json').write_text(json.dumps(report, indent=2) + '\n')
print(json.dumps(report, indent=2))
raise SystemExit(0 if report['allPassed'] else 1)
