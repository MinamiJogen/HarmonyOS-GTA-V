#!/usr/bin/env python3
"""Exercise the actual runner main with file-producing fake HDC and clocks."""
import copy
import json
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest
from unittest.mock import patch

import run_builtin_benchmark as runner
from analyze_native_benchmark import analyze


# Synthetic timings and a dummy timestamp exercise the report parser without
# publishing a device's benchmark capture.
FIXTURE = Path(__file__).parent / 'fixtures/gta_builtin_single_pass'
REPORT = 'Benchmark-00-00-00-00-00-00.txt'
RAW = 'FrameTimes-Pass0-00-00-00-00-00-00.txt'


class FakeDevice:
    def __init__(self, base, *, inventory_failure=False, old_artifacts=False,
                 freeze_after=None, exit_on_stable=False, wrong_startup_fg=False,
                 mutate_config=False, late_old_artifacts=False, report_pass=0,
                 raw_pass=None, extra_raw_pass=None, multiple_report_passes=False,
                 source_target=20, missing_source_target=False, render_size=(1200,800), fg_size=None,
                 hidden_demo=False, parent_inventory=None, report_missing_polls=0, report_parent_inventory=None):
        self.config = copy.deepcopy(base)
        self.now = 0.0
        self.running = late_old_artifacts
        self.pid = 41
        self.status_reads = 0
        self.frame_reads = 0
        self.report_polls = 0
        self.inventory_failure = inventory_failure
        self.old_artifacts = old_artifacts
        self.freeze_after = freeze_after
        self.exit_on_stable = exit_on_stable
        self.wrong_startup_fg = wrong_startup_fg
        self.mutate_config = mutate_config
        self.late_old_artifacts = late_old_artifacts
        self.report_pass = report_pass
        self.raw_pass = report_pass if raw_pass is None else raw_pass
        self.extra_raw_pass = extra_raw_pass
        self.multiple_report_passes = multiple_report_passes
        self.source_target = source_target
        self.missing_source_target = missing_source_target
        self.sent = []
        self.commands = []
        self.render_size = list(render_size)
        self.fg_size = list(fg_size or render_size)
        self.hidden_demo = hidden_demo
        self.scrolls = 0
        self.parent_inventory = parent_inventory
        self.report_missing_polls = report_missing_polls
        self.report_parent_inventory = report_parent_inventory

    def monotonic(self):
        return self.now

    def sleep(self, seconds):
        self.now += seconds

    def wrapper(self):
        # First complete artifact is returned at t=6, status read 4. A frozen
        # status at exactly that boundary must never be accepted at t=12.
        self.status_reads += 1
        tick = self.now
        if self.freeze_after is not None:
            tick = min(tick, self.freeze_after)
        clock = 100_000_000_000 + int(tick * 1e9)
        runtime = {'processId': self.pid, 'nativeTelemetry': {'monotonicNs': clock,
                   'sourceTargetFps': self.source_target, 'currentSourceFps': 20},
                   'guestPresents': 10 + int(tick * 20), 'presents': 20 + int(tick * 40),
                   'renderSize': self.render_size, 'appForeground': True,
                   'presentationPaused': False, 'error': ''}
        if self.missing_source_target:
            del runtime['nativeTelemetry']['sourceTargetFps']
        conditions = {'enabled': True, 'active': True, 'fresh': True,
                      'generation': 1, 'sampleGeneration': 1, 'sampleAgeMs': 100,
                      'sample': {'uptimeMs': int(tick * 1000), 'completedUptimeMs': int(tick * 1000 + 1),
                                 'batteryTemperatureC': 38.5, 'thermalLevel': 2,
                                 'batterySOC': 70, 'pluggedType': 1, 'chargingStatus': 2,
                                 'apiFailures': []}}
        return {'runtimeJson': json.dumps(runtime), 'uiBenchmarkConditions': conditions}

    def artifacts(self, target):
        target.mkdir(parents=True, exist_ok=True)
        text = (FIXTURE / REPORT).read_text().replace('Pass 0', f'Pass {self.report_pass}').replace('pass 0', f'pass {self.report_pass}')
        if self.multiple_report_passes:
            fps_line = f'Pass {self.report_pass}, 12.500000, 50.000000, 31.335616'
            text = text.replace(fps_line, fps_line + '\nPass 9, 12.500000, 50.000000, 31.335616', 1)
        (target / REPORT).write_text(text)
        for identity in [self.raw_pass] + ([] if self.extra_raw_pass is None else [self.extra_raw_pass]):
            shutil.copyfile(FIXTURE / RAW, target / RAW.replace('Pass0-', f'Pass{identity}-'))

    def run(self, args, **kwargs):
        command = args[3:]
        self.commands.append(command)
        if command[:4] == ['shell','uitest','uiInput','swipe']:
            self.scrolls += 1
        code, output, error = 0, '', ''
        if command[:2] == ['shell', 'pidof']:
            output = str(self.pid) if self.running else ''
        elif command[:3] == ['shell', 'aa', 'force-stop']:
            self.running = False
        elif command[:3] == ['shell', 'aa', 'start']:
            self.running = True
        elif command[:2] == ['file', 'send']:
            self.config = json.loads(Path(command[-2]).read_text())
            self.sent.append(copy.deepcopy(self.config))
        elif command[:2] == ['file', 'recv']:
            remote, target = command[-2], Path(command[-1])
            target.parent.mkdir(parents=True, exist_ok=True)
            if remote.endswith('/Rockstar Games/GTA V/Benchmarks/'):
                if target.name == 'benchmark-inventory-before':
                    if self.inventory_failure:
                        code, error = 1, 'permission denied while receiving benchmark inventory'
                    elif self.old_artifacts or self.late_old_artifacts and not self.running:
                        self.artifacts(target)
                    else:
                        target.mkdir()
                else:
                    self.report_polls += 1
                    if self.report_polls <= self.report_missing_polls:
                        code,error=1,'No such file or directory: Benchmarks'
                    else:
                        target.mkdir()
                        if self.report_polls >= 2:
                            self.artifacts(target)
                    if self.exit_on_stable and self.report_polls >= 3:
                        self.running = False
            elif remote.endswith('/Rockstar Games/GTA V/'):
                mode=self.report_parent_inventory if target.name.startswith('report-poll-') else self.parent_inventory
                if mode == 'failed':
                    code,error=1,'permission denied while receiving parent inventory'
                elif mode == 'not-directory':
                    target.write_text('incomplete directory payload')
                elif mode in ('empty','partial','partial-stderr','existing'):
                    target.mkdir();(target/'Default').mkdir();(target/'Default/settings.xml').write_text('unchanged save settings')
                    if mode=='partial':
                        output='[Fail] a child entry could not be received'
                    elif mode=='partial-stderr':
                        error='[Fail] a child entry could not be received'
                    elif mode=='existing':
                        self.artifacts(target/'Benchmarks')
                else:raise AssertionError('unexpected parent inventory fallback')
            elif remote.endswith('config.json'):
                value = {**self.config, 'fps_limit': 25} if self.mutate_config and target.name == 'config-after.json' else self.config
                target.write_text(json.dumps(value))
            elif remote.endswith('game-status.json'):
                target.write_text(json.dumps(self.wrapper()))
            elif remote.endswith('frame-interpolation-status.json'):
                self.frame_reads += 1
                pid = self.pid - 1 if self.wrong_startup_fg and self.frame_reads == 1 else self.pid
                value = {'pid': pid, 'epoch': 1, 'statusSnapshotGeneratedMonotonicNs': 100_000_000_000 + int(self.now * 1e9),
                         'guestFinalSource': {'width': self.fg_size[0], 'height': self.fg_size[1]}, 'sourcePresents': int(self.now * 20),
                         'aiPresents': int(self.now * 20), 'repeatPresents': 0, 'missedDeadlineSlots': 0,
                         'surfaceAcquireFailures': 0, 'recentPresents': []}
                target.write_text(json.dumps(value))
            elif remote.endswith('gta-builtin-home.json'):
                layout = {'attributes': {'type': 'Button', 'text': '开始 Demo',
                                         'enabled': 'true', 'bounds': '[0,0][100,100]'}}
                if self.hidden_demo and not self.scrolls:
                    layout = {'children': [{'attributes': {'type':'Text','text':'资源已就绪','visible':'true'}},
                                          {'attributes': {'type':'Scroll','scrollable':'true','visible':'true',
                                                          'enabled':'true','bounds':'[100,200][2100,1200]'}}]}
                layout = {'attributes': {'type':'root','bundleName':runner.BUNDLE,'abilityName':'EntryAbility',
                                         'pagePath':'pages/Index','visible':'true'},'children':[layout]}
                target.write_text(json.dumps(layout))
            elif remote.endswith('game-runtime.log'):
                target.write_text('[config] effective -FrameQueueLimit=3; one canonical argument\n')
            elif 'native-frame-intervals-' in remote:
                code, error = 1, 'optional trace not produced by fake device'
            else:
                raise AssertionError('unexpected HDC receive: ' + remote)
        elif command[:2] != ['shell', 'uitest']:
            raise AssertionError('unexpected HDC command: ' + repr(command))
        return subprocess.CompletedProcess(args, code, output, error)


class RunnerTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='builtin-runner-test-')
        self.root = Path(self.temp.name)
        self.base = {'graphics_preset': 'medium', 'spatial_upscale_enabled': True,
                     'frame_interpolation_enabled': True, 'fps_limit': 24,
                     'frame_interpolation_output_fps': 40, 'extra_args': ['-existing-setting'],
                     'untouched': {'keep': [1, 2, 3]}, 'cpu_profile': True}
        (self.root / 'base.json').write_text(json.dumps(self.base))
        (self.root / 'package.hap').write_bytes(b'fake already-installed package identity')

    def tearDown(self):
        self.temp.cleanup()

    def main(self, device, *, benchmark_pass=0, directory_name='trial', baseline_mode='legacy20',
             fixed_features=(), cpu_profile=None, feature='pipeline_cache', render_prepass_diagnostics=None,
             file_transfer_mode=None, files_root=None, expected_native_size=None):
        args = ['run_builtin_benchmark', '--device', 'fake-device', '--hdc', 'fake-hdc',
                '--baseline-config', str(self.root / 'base.json'), '--package', str(self.root / 'package.hap'),
                '--directory', str(self.root / directory_name), '--feature', feature, '--enabled', 'true',
                '--trace', 'true', '--timeout', '40', '--pair-id', 'one', '--trial-id', 'ON']
        args.extend(['--benchmark-pass', str(benchmark_pass)])
        for flag,value in (('--file-transfer-mode',file_transfer_mode),('--files-root',files_root),
                           ('--expected-native-size',expected_native_size)):
            if value is not None:args.extend([flag,value])
        args.extend(['--baseline-mode', baseline_mode])
        for fixed in fixed_features:
            args.extend(['--fixed-feature', fixed])
        if cpu_profile is not None:
            args.extend(['--cpu-profile', cpu_profile])
        if render_prepass_diagnostics is not None:
            args.extend(['--render-prepass-diagnostics', render_prepass_diagnostics])
        with patch('sys.argv', args), patch.object(runner.subprocess, 'run', device.run), \
             patch.object(runner.time, 'monotonic', device.monotonic), patch.object(runner.time, 'sleep', device.sleep), \
             patch('builtins.print'):
            runner.main()

    def test_configuration_preserves_quality_policy_and_unknown_values(self):
        value = runner.benchmark_configuration(self.base, 'occlusion_pool', True, True, 0)
        self.assertEqual(self.base['extra_args'], ['-existing-setting'])
        self.assertEqual(value['untouched'], self.base['untouched'])
        for key in ('graphics_preset', 'spatial_upscale_enabled', 'frame_interpolation_enabled', 'fps_limit', 'frame_interpolation_output_fps'):
            self.assertEqual(value[key], self.base[key])
        self.assertTrue(value['occlusion_pool'])
        self.assertFalse(value['pipeline_cache'])
        self.assertTrue(value['frame_interval_trace'])
        self.assertFalse(value['cpu_profile'])
        self.assertTrue(value['native_benchmark_monitor'])
        with self.assertRaisesRegex(ValueError, 'already contains benchmark'):
            runner.benchmark_configuration({**self.base, 'extra_args': ['-benchmarkPass=2']}, 'pipeline_cache', True, True, 0)

    def test_native_candidates_are_independently_controlled_without_graphics_changes(self):
        base = {**self.base, 'fps_limit': 0, 'spatial_upscale_native_short_edge': 800,
                'native_render_fastpath': True, 'io_handle_lru': True}
        for feature in ('native_render_fastpath', 'io_handle_lru'):
            for enabled in (False, True):
                with self.subTest(feature=feature, enabled=enabled):
                    result = runner.benchmark_configuration(base, feature, enabled, False, 0, 'unlimited-native-800p')
                    self.assertIs(result[feature], enabled)
                    self.assertFalse(result['io_handle_lru' if feature == 'native_render_fastpath' else 'native_render_fastpath'])
                    self.assertEqual(result['fps_limit'], 0)
                    self.assertEqual(result['extra_args'][4:], base['extra_args'])
                    self.assertTrue(result['spatial_upscale_enabled'])
                    self.assertTrue(result['frame_interpolation_enabled'])
        with self.assertRaisesRegex(ValueError, 'unsupported'):
            runner.benchmark_configuration(base, 'graphics_preset', True, False, 0, 'unlimited-native-800p')

    def test_fixed_native_flags_apply_after_defaults_before_independent_feature(self):
        base = {**self.base, **{flag: True for flag in runner.FLAGS}}
        fixed = {'native_render_fastpath': True, 'io_handle_lru': False, 'shared_memory_fastpath': False}
        configurations = [runner.benchmark_configuration(base, 'pipeline_cache', enabled, False, 0,
                                                         fixed_flags=fixed, cpu_profile=True)
                          for enabled in (False, True)]
        for value in configurations:
            for flag in runner.FLAGS:
                if flag != 'pipeline_cache':
                    self.assertIs(value[flag], fixed.get(flag, False))
            self.assertTrue(value['cpu_profile'])
            self.assertEqual(value['fps_limit'], self.base['fps_limit'])
            self.assertEqual(value['extra_args'][4:], self.base['extra_args'])
            self.assertEqual(value['untouched'], self.base['untouched'])
        self.assertEqual([k for k in configurations[0] if configurations[0][k] != configurations[1][k]], ['pipeline_cache'])
        self.assertEqual(fixed, {'native_render_fastpath': True, 'io_handle_lru': False, 'shared_memory_fastpath': False})

    def test_fixed_flag_cli_rejects_graphics_diagnostics_duplicates_conflicts_and_nonbooleans(self):
        self.assertEqual(runner.fixed_feature_options(['native_render_fastpath=true', 'io_handle_lru=false'], 'pipeline_cache'),
                         {'native_render_fastpath': True, 'io_handle_lru': False})
        invalid = [['native_render_fastpath=1'], ['native_render_fastpath=True'], ['io_handle_lru'],
                   ['pipeline_cache=true'], ['native_render_fastpath=true', 'native_render_fastpath=false']]
        invalid += [[key + '=true'] for key in ('fps_limit', 'graphics_preset', 'cpu_profile', 'gpu_profile',
                                                'frame_interval_trace', 'spatial_upscale_enabled', 'frame_interpolation_enabled',
                                                'render_prepass_diagnostics')]
        for options in invalid:
            with self.subTest(options=options), self.assertRaises(ValueError):
                runner.fixed_feature_options(options, 'pipeline_cache')
        for flags in ({'io_handle_lru': 1}, {'io_handle_lru': 'false'}, {'pipeline_cache': False}, [('io_handle_lru', True)]):
            with self.subTest(flags=flags), self.assertRaises(ValueError):
                runner.benchmark_configuration(self.base, 'pipeline_cache', True, True, 0, fixed_flags=flags)
        with self.assertRaises(ValueError):
            runner.benchmark_configuration(self.base, 'pipeline_cache', True, True, 0, cpu_profile='true')

    def test_actual_fixed_flags_and_cpu_diagnostic_are_recorded_without_changing_default_manifest(self):
        self.main(FakeDevice(self.base))
        default = json.loads((self.root / 'trial/manifest.json').read_text())
        self.assertNotIn('fixedNativeFeatures', default['conditions'])
        self.assertIs(default['conditions']['instrumentation']['cpuProfile'], False)
        device = FakeDevice(self.base)
        self.main(device, directory_name='fixed-trial', fixed_features=('native_render_fastpath=true', 'io_handle_lru=false'), cpu_profile='true')
        manifest = json.loads((self.root / 'fixed-trial/manifest.json').read_text())
        self.assertEqual(manifest['conditions']['fixedNativeFeatures'], {'native_render_fastpath': True, 'io_handle_lru': False})
        self.assertTrue(manifest['conditions']['instrumentation']['cpuProfile'])
        self.assertTrue(device.sent[0]['native_render_fastpath'])
        self.assertFalse(device.sent[0]['io_handle_lru'])
        self.assertTrue(device.sent[0]['pipeline_cache'])
        self.assertTrue(device.sent[0]['cpu_profile'])
        self.assertEqual(analyze([self.root / 'fixed-trial/manifest.json'])['trials'][0]['qualificationIssues'], [])

    def test_read_only_pipeline_cache_is_fixed_boolean_and_defaults_off(self):
        base = {**self.base, 'pipeline_cache_read_only': True}
        default = runner.benchmark_configuration(base, 'pipeline_cache', True, True, 0)
        self.assertIs(default['pipeline_cache_read_only'], False)
        for invalid in (1, 'true', None):
            with self.subTest(invalid=invalid), self.assertRaises(ValueError):
                runner.benchmark_configuration(base, 'pipeline_cache', True, True, 0,
                                               fixed_flags={'pipeline_cache_read_only': invalid})
        device = FakeDevice(self.base)
        self.main(device, fixed_features=('pipeline_cache_read_only=true',))
        manifest = json.loads((self.root / 'trial/manifest.json').read_text())
        self.assertIs(device.sent[0]['pipeline_cache_read_only'], True)
        self.assertEqual(manifest['conditions']['fixedNativeFeatures'], {'pipeline_cache_read_only': True})
        self.assertEqual(analyze([self.root / 'trial/manifest.json'])['trials'][0]['qualificationIssues'], [])

    def test_cpu_command_and_empty_init_flags_are_independent_and_fixed_native_booleans(self):
        names = ('inline_upload_borrow', 'native_command_fastpath', 'skip_empty_init_transition',
                 'skip_empty_render_prepass')
        base = {**self.base, **dict.fromkeys(names, True)}
        for feature in names:
            with self.subTest(feature=feature):
                off = runner.benchmark_configuration(base, feature, False, False, 0)
                on = runner.benchmark_configuration(base, feature, True, False, 0)
                self.assertEqual([key for key in on if off[key] != on[key]], [feature])
                for key in names:
                    self.assertIs(off[key], False)
                    self.assertIs(on[key], key == feature)
                self.assertEqual(on['fps_limit'], self.base['fps_limit'])
                self.assertEqual(on['extra_args'][4:], self.base['extra_args'])
                with self.assertRaisesRegex(ValueError, 'explicit booleans'):
                    runner.benchmark_configuration(base, feature, 1, False, 0)
                self.main(FakeDevice(self.base), feature=feature, directory_name=feature)
                manifest = self.root / feature / 'manifest.json'
                self.assertEqual(json.loads(manifest.read_text())['feature'], feature)
                self.assertEqual(analyze([manifest])['trials'][0]['qualificationIssues'], [])
        self.assertEqual(runner.fixed_feature_options([name + '=true' for name in names], 'pipeline_cache'),
                         dict.fromkeys(names, True))

    def test_render_prepass_diagnostic_is_explicit_strict_and_recorded_for_both_values(self):
        self.assertNotIn('render_prepass_diagnostics', runner.FLAGS)
        default = runner.benchmark_configuration({**self.base, 'render_prepass_diagnostics': True},
                                                 'pipeline_cache', False, False, 0)
        self.assertIs(default['render_prepass_diagnostics'], False)
        for invalid in (None, 0, 1, 'true', 'false'):
            with self.subTest(invalid=invalid), self.assertRaisesRegex(ValueError, 'explicit boolean diagnostic'):
                runner.benchmark_configuration(self.base, 'pipeline_cache', False, False, 0,
                                               render_prepass_diagnostics=invalid)
        for value in ('false', 'true'):
            device = FakeDevice(self.base)
            name = 'prepass-' + value
            self.main(device, directory_name=name, feature='skip_empty_render_prepass',
                      render_prepass_diagnostics=value)
            manifest = json.loads((self.root / name / 'manifest.json').read_text())
            self.assertIs(device.sent[0]['render_prepass_diagnostics'], value == 'true')
            self.assertIs(manifest['conditions']['instrumentation']['renderPrepassDiagnostics'], value == 'true')
            self.assertEqual(analyze([self.root / name / 'manifest.json'])['trials'][0]['qualificationIssues'], [])
        for value in ('1', 'True', 'null'):
            device = FakeDevice(self.base)
            with self.subTest(cli=value), patch('sys.stderr'), self.assertRaises(SystemExit):
                self.main(device, render_prepass_diagnostics=value)
            self.assertEqual(device.sent, [])

    def test_direct_files_explicit_size_and_scroll_use_actual_nodes_and_bind_conditions(self):
        self.base.update(fps_limit=0,spatial_upscale_native_short_edge=800)
        (self.root/'base.json').write_text(json.dumps(self.base))
        remote='/data/app/el2/100/base/com.minami.harmonyruntimeprobe/haps/entry/files/'
        device=FakeDevice(self.base,source_target=None,render_size=(1714,800),hidden_demo=True)
        self.main(device,baseline_mode='unlimited-native-800p',file_transfer_mode='direct',files_root=remote,
                  expected_native_size='1714x800')
        manifest=json.loads((self.root/'trial/manifest.json').read_text())
        self.assertEqual(manifest['conditions']['nativeRenderSize'],[1714,800])
        self.assertEqual(manifest['conditions']['fileTransport'],{'mode':'direct','filesRoot':remote})
        self.assertEqual(analyze([self.root/'trial/manifest.json'])['trials'][0]['qualificationIssues'],[])
        self.assertEqual(device.scrolls,1)
        swipe=next(c for c in device.commands if c[:4]==['shell','uitest','uiInput','swipe'])
        self.assertEqual(swipe[4:8],['1600','1000','1600','500'])
        for command in device.commands:
            if command[:2] in (['file','recv'],['file','send']):
                self.assertNotIn('-b',command)
                actual=command[-2] if command[1]=='recv' else command[-1]
                self.assertTrue(actual.startswith(remote) or actual=='/data/local/tmp/gta-builtin-home.json')
        self.assertTrue(any(c[:3]==['shell','aa','start'] and '-b' in c for c in device.commands))

    def test_native_size_mismatch_in_runtime_or_fg_never_accepts_benchmark(self):
        self.base.update(fps_limit=0,spatial_upscale_native_short_edge=800)
        (self.root/'base.json').write_text(json.dumps(self.base))
        for name,options in [('runtime',{}),('fg',{'render_size':(1714,800),'fg_size':(1200,800)})]:
            device=FakeDevice(self.base,source_target=None,**options)
            with self.subTest(name=name), self.assertRaisesRegex(RuntimeError,'native.*differs'):
                self.main(device,directory_name=name,baseline_mode='unlimited-native-800p',expected_native_size='1714x800')
            self.assertFalse((self.root/name/'manifest.json').exists())

    def test_invalid_direct_root_or_size_rejected_before_any_device_call(self):
        for opts in ({'file_transfer_mode':'direct'}, {'files_root':'/data/app/el2/100/base/wrong/haps/entry/files/'},
                     {'file_transfer_mode':'direct','files_root':'/data/app/el2/100/base/wrong/haps/entry/files/'},
                     {'expected_native_size':'1713x800'}, {'expected_native_size':'1200x802'}):
            device=FakeDevice(self.base)
            with self.subTest(options=opts), patch('sys.stderr'), self.assertRaises(SystemExit):self.main(device,**opts)
            self.assertEqual(device.commands,[])

    def test_direct_cold_missing_benchmarks_requires_successful_parent_before_empty_inventory(self):
        remote='/data/app/el2/100/base/com.minami.harmonyruntimeprobe/haps/entry/files/'
        device=FakeDevice(self.base,inventory_failure=True,parent_inventory='empty')
        self.main(device,file_transfer_mode='direct',files_root=remote)
        provenance=json.loads((self.root/'trial/benchmark-inventory-provenance.json').read_text())
        self.assertEqual(provenance['status'],'required_parent_confirms_benchmarks_absent')
        self.assertEqual(json.loads((self.root/'trial/benchmark-inventory-before.json').read_text()),[])
        self.assertTrue((self.root/'trial/inventory-parent/Default/settings.xml').is_file())
        self.assertEqual(analyze([self.root/'trial/manifest.json'])['trials'][0]['qualificationIssues'],[])
        self.assertFalse(any(c[:2]==['shell','mkdir'] for c in device.commands))

    def test_direct_parent_failed_partial_or_nondirectory_never_means_empty(self):
        remote='/data/app/el2/100/base/com.minami.harmonyruntimeprobe/haps/entry/files/'
        for kind in ('failed','partial','partial-stderr','not-directory'):
            device=FakeDevice(self.base,inventory_failure=True,parent_inventory=kind)
            with self.subTest(kind=kind),self.assertRaises(RuntimeError):
                self.main(device,directory_name=kind,file_transfer_mode='direct',files_root=remote)
            provenance=json.loads((self.root/kind/'benchmark-inventory-provenance.json').read_text())
            self.assertEqual(provenance['status'],'rejected')
            self.assertEqual(device.sent,[])
            self.assertFalse((self.root/kind/'manifest.json').exists())

    def test_successful_parent_existing_reports_keep_original_old_file_rejection(self):
        remote='/data/app/el2/100/base/com.minami.harmonyruntimeprobe/haps/entry/files/'
        device=FakeDevice(self.base,inventory_failure=True,parent_inventory='existing')
        with self.assertRaisesRegex(RuntimeError,'no complete stable builtin report'):
            self.main(device,file_transfer_mode='direct',files_root=remote)
        provenance=json.loads((self.root/'trial/benchmark-inventory-provenance.json').read_text())
        self.assertEqual(provenance['status'],'required_parent_existing_benchmarks')
        self.assertEqual(len(json.loads((self.root/'trial/benchmark-inventory-before.json').read_text())),2)
        self.assertFalse((self.root/'trial/manifest.json').exists())

    def test_parent_unreadable_or_linked_benchmarks_never_claim_absence(self):
        original=runner.os.scandir
        for kind in ('unreadable','link'):
            out=self.root/kind;out.mkdir()
            def receive(remote,target):
                if remote.endswith('/Benchmarks/'):raise RuntimeError('primary directory unavailable')
                target.mkdir()
                if kind=='link':(target/'Benchmarks').symlink_to(FIXTURE,target_is_directory=True)
                else:(target/'Benchmarks').mkdir()
            def scandir(path):
                if kind=='unreadable' and Path(path).name=='Benchmarks':raise PermissionError('child directory unreadable')
                return original(path)
            with self.subTest(kind=kind),patch.object(runner.os,'scandir',scandir),self.assertRaises((PermissionError,RuntimeError)):
                runner.capture_before_inventory(receive,'/app/GTA V/Benchmarks/',out,direct=True)
            self.assertEqual(json.loads((out/'benchmark-inventory-provenance.json').read_text())['status'],'rejected')

    def test_report_polls_verify_missing_parent_then_accept_only_later_complete_fresh_report(self):
        remote='/data/app/el2/100/base/com.minami.harmonyruntimeprobe/haps/entry/files/'
        device=FakeDevice(self.base,inventory_failure=True,parent_inventory='empty',
                          report_missing_polls=2,report_parent_inventory='empty')
        self.main(device,file_transfer_mode='direct',files_root=remote)
        for index in (0,3):
            p=self.root/f'trial/report-poll-{index:04d}-provenance.json'
            self.assertEqual(json.loads(p.read_text())['status'],'required_parent_confirms_benchmarks_absent')
        self.assertEqual(json.loads((self.root/'trial/report-poll-0006-provenance.json').read_text())['status'],'primary_inventory_complete')
        self.assertTrue((self.root/'trial/manifest.json').is_file())
        self.assertEqual(analyze([self.root/'trial/manifest.json'])['trials'][0]['qualificationIssues'],[])

    def test_report_poll_parent_errors_refuse_without_stopping_running_game(self):
        remote='/data/app/el2/100/base/com.minami.harmonyruntimeprobe/haps/entry/files/'
        for mode in ('failed','partial','partial-stderr','not-directory'):
            device=FakeDevice(self.base,report_missing_polls=1,report_parent_inventory=mode)
            with self.subTest(mode=mode),self.assertRaises(RuntimeError):
                self.main(device,directory_name=mode,file_transfer_mode='direct',files_root=remote)
            provenance=json.loads((self.root/mode/'report-poll-0000-provenance.json').read_text())
            self.assertEqual(provenance['status'],'rejected')
            self.assertFalse((self.root/mode/'manifest.json').exists())
            self.assertTrue(device.running)
            self.assertEqual(sum(c[:3]==['shell','aa','force-stop'] for c in device.commands),1)

    def test_report_poll_required_parent_can_supply_fresh_accessible_benchmarks(self):
        remote='/data/app/el2/100/base/com.minami.harmonyruntimeprobe/haps/entry/files/'
        device=FakeDevice(self.base,report_missing_polls=2,report_parent_inventory='existing')
        self.main(device,file_transfer_mode='direct',files_root=remote)
        provenance=json.loads((self.root/'trial/report-poll-0000-provenance.json').read_text())
        self.assertEqual(provenance['status'],'required_parent_existing_benchmarks')
        self.assertEqual(analyze([self.root/'trial/manifest.json'])['trials'][0]['qualificationIssues'],[])

    def test_explicit_unlimited_configuration_keeps_quality_and_rejects_hidden_or_implicit_caps(self):
        base = {**self.base, 'fps_limit': 0, 'spatial_upscale_native_short_edge': 800}
        result = runner.benchmark_configuration(base, 'pipeline_cache', False, True, 0, 'unlimited-native-800p')
        self.assertEqual(result['fps_limit'], 0)
        self.assertEqual(result['extra_args'][4:], base['extra_args'])
        self.assertEqual(result['untouched'], base['untouched'])
        for cap in (False, None, 24):
            with self.subTest(cap=cap), self.assertRaisesRegex(ValueError, 'explicit fps0'):
                runner.benchmark_configuration({**base, 'fps_limit': cap}, 'pipeline_cache', False, True, 0, 'unlimited-native-800p')
        with self.assertRaisesRegex(ValueError, 'fps24'):
            runner.benchmark_configuration(base, 'pipeline_cache', False, True, 0)
        for flag in ('-FrameLimit=20', '-FPS=24', '-fpsLimit'):
            with self.subTest(flag=flag), self.assertRaisesRegex(ValueError, 'hidden'):
                runner.benchmark_configuration({**base, 'extra_args': [flag]}, 'pipeline_cache', False, True, 0, 'unlimited-native-800p')

    def test_actual_unlimited_main_requires_explicit_runtime_null_and_emits_separate_goal(self):
        self.base.update(fps_limit=0, spatial_upscale_native_short_edge=800)
        (self.root / 'base.json').write_text(json.dumps(self.base))
        self.main(FakeDevice(self.base, source_target=None), baseline_mode='unlimited-native-800p')
        manifest = json.loads((self.root / 'trial/manifest.json').read_text())
        self.assertIsNone(manifest['conditions']['sourceTargetFps'])
        self.assertEqual(manifest['conditions']['performanceGoal'], {'averageFps': 30, 'onePercentLowFps': 20})
        analyzed = analyze([self.root / 'trial/manifest.json'])['trials'][0]
        self.assertEqual(analyzed['qualificationIssues'], [])
        self.assertIsNone(analyzed['frameIntervals'].get('chains', {}).get('guest', {}).get('overSourceBudgetIntervals'))
        self.assertGreater(analyzed['sourceTargetObservations']['explicitNull'], 0)
        self.assertFalse(analyzed['performanceGoalAssessment']['nativeSourceGoalEstablished'])
        for index, options in enumerate(({'source_target': 0}, {'source_target': 20}, {'missing_source_target': True})):
            with self.subTest(options=options), self.assertRaisesRegex(RuntimeError, 'explicit native sourceTargetFps:null'):
                self.main(FakeDevice(self.base, **options), baseline_mode='unlimited-native-800p', directory_name=f'rejected-null-{index}')

    def test_report_accepts_observed_format_with_different_raw_suffix_and_rejects_partial(self):
        target = self.root / 'reports'
        target.mkdir()
        shutil.copyfile(FIXTURE / REPORT, target / REPORT)
        shutil.copyfile(FIXTURE / RAW, target / RAW.replace('00-00-00-00-00-00', '00-00-00-00-00-01'))
        detail = runner.complete_report(target / REPORT)
        self.assertIsNotNone(detail)
        self.assertEqual(detail['table']['passes'][0]['reportedFrames'], 219)
        self.assertEqual(detail['reportPasses'], ['0'])
        self.assertIsNone(runner.complete_report(target / REPORT, ('0', '1')))
        raw_path = detail['frameFiles'][0][1]
        raw_path.write_text('\n'.join(raw_path.read_text().splitlines()[:-1]) + '\n')
        self.assertIsNone(runner.complete_report(target / REPORT))

    def test_actual_main_success_closes_native_bracket_after_stable_read_and_analyzes(self):
        device = FakeDevice(self.base, wrong_startup_fg=True)
        self.main(device)
        trial = self.root / 'trial'
        manifest = json.loads((trial / 'manifest.json').read_text())
        binding = json.loads((trial / 'artifact-binding.json').read_text())
        closing = json.loads(json.loads((trial / 'closing-status.json').read_text())['runtimeJson'])
        self.assertEqual(manifest['benchmark']['windowScope'], 'enclosing_native_monotonic_bracket')
        self.assertFalse(binding['exactPhaseBoundary'])
        self.assertGreater(binding['window']['endMonotonicNs'], 112_000_000_000)
        self.assertEqual(binding['window']['endMonotonicNs'], closing['nativeTelemetry']['monotonicNs'])
        self.assertEqual(len(list(trial.glob('startup-fg-other-pid-*.json'))), 1)
        self.assertTrue(all(json.loads(path.read_text())['pid'] == device.pid for path in (trial / 'capture').glob('fg-*.json')))
        analyzed = analyze([trial / 'manifest.json'])['trials'][0]
        self.assertEqual(analyzed['qualificationIssues'], [])
        self.assertAlmostEqual(analyzed['rawFrameTimes']['passes'][0]['throughputFps'], 1000 * 219 / 10920)
        self.assertEqual(analyzed['thermal']['batteryTemperatureC']['p50'], 38.5)

    def test_inventory_transfer_failure_never_treats_old_files_as_fresh(self):
        with self.assertRaisesRegex(RuntimeError, 'permission denied|inventory'):
            self.main(FakeDevice(self.base, inventory_failure=True))
        self.assertFalse((self.root / 'trial/manifest.json').exists())

    def test_status_frozen_at_first_complete_report_never_succeeds_before_stale_timeout(self):
        with self.assertRaisesRegex(RuntimeError, 'stale|complete stable'):
            self.main(FakeDevice(self.base, freeze_after=6))
        self.assertFalse((self.root / 'trial/manifest.json').exists())

    def test_process_exit_after_stable_read_never_accepts_a_stale_closing_status(self):
        with self.assertRaisesRegex(RuntimeError, 'disappeared|complete stable'):
            self.main(FakeDevice(self.base, exit_on_stable=True))
        self.assertFalse((self.root / 'trial/manifest.json').exists())

    def test_preexisting_identical_report_is_not_new_evidence(self):
        with self.assertRaisesRegex(RuntimeError, 'complete stable'):
            self.main(FakeDevice(self.base, old_artifacts=True))
        self.assertFalse((self.root / 'trial/manifest.json').exists())

    def test_config_mutation_at_finish_prevents_success(self):
        with self.assertRaisesRegex(RuntimeError, 'changed trial configuration'):
            self.main(FakeDevice(self.base, mutate_config=True))
        self.assertFalse((self.root / 'trial/manifest.json').exists())

    def test_prior_process_last_artifact_before_stop_is_in_before_inventory(self):
        # Simulate an old writer publishing its final report after an inventory
        # read while still alive. Inventory after stop must see those same bytes
        # and reject them as old rather than binding them to the successor PID.
        with self.assertRaisesRegex(RuntimeError, 'complete stable'):
            self.main(FakeDevice(self.base, late_old_artifacts=True))
        self.assertFalse((self.root / 'trial/manifest.json').exists())

    def test_actual_main_cli4_binds_report_pass4_and_raw_pass4(self):
        device = FakeDevice(self.base, report_pass=4)
        self.main(device, benchmark_pass=4)
        directory = self.root / 'trial'
        manifest = json.loads((directory / 'manifest.json').read_text())
        binding = json.loads((directory / 'artifact-binding.json').read_text())
        self.assertEqual(manifest['benchmark']['sceneId'], 'commandline-benchmarkPass=4')
        self.assertEqual(manifest['benchmark']['expectedPasses'], ['4'])
        self.assertEqual(manifest['benchmark']['frameTimes'][0]['pass'], '4')
        self.assertEqual(set(binding['frameTimesSha256']), {'4'})
        self.assertIn('-benchmarkPass=4', device.sent[0]['extra_args'])
        analyzed = analyze([directory / 'manifest.json'])['trials'][0]
        self.assertEqual(analyzed['qualificationIssues'], [])
        self.assertEqual(analyzed['benchmark']['passes'][0]['pass'], '4')
        self.assertEqual(analyzed['rawFrameTimes']['passes'][0]['frames'], 219)

    def test_actual_main_cli4_can_bind_report_pass0_without_relabeling_scene(self):
        device = FakeDevice(self.base, report_pass=0)
        self.main(device, benchmark_pass=4)
        directory = self.root / 'trial'
        manifest = json.loads((directory / 'manifest.json').read_text())
        self.assertEqual(manifest['benchmark']['sceneId'], 'commandline-benchmarkPass=4')
        self.assertEqual(manifest['benchmark']['expectedPasses'], ['0'])
        self.assertEqual(manifest['benchmark']['frameTimes'][0]['pass'], '0')
        analyzed = analyze([directory / 'manifest.json'])['trials'][0]
        self.assertEqual(analyzed['qualificationIssues'], [])
        self.assertEqual(analyzed['benchmarkSceneId'], 'commandline-benchmarkPass=4')
        self.assertEqual(analyzed['benchmark']['passes'][0]['pass'], '0')

    def test_single_scene_main_rejects_multiple_report_passes_and_mixed_or_wrong_raw(self):
        options = [{'multiple_report_passes': True}, {'extra_raw_pass': 0}, {'raw_pass': 0}]
        for index, option in enumerate(options):
            with self.subTest(option=option):
                name = 'rejected-' + str(index)
                with self.assertRaisesRegex(RuntimeError, 'complete stable'):
                    self.main(FakeDevice(self.base, report_pass=4, **option), benchmark_pass=4, directory_name=name)
                self.assertFalse((self.root / name / 'manifest.json').exists())

    def test_explicit_expected_pass_still_restricts_dynamic_report(self):
        directory = self.root / 'report-four'
        FakeDevice(self.base, report_pass=4).artifacts(directory)
        self.assertIsNotNone(runner.complete_report(directory / REPORT, ('4',)))
        self.assertIsNone(runner.complete_report(directory / REPORT, ('0',)))
        self.assertIsNone(runner.complete_report(directory / REPORT, ('0', '4')))


if __name__ == '__main__':
    unittest.main()
