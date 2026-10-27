#!/usr/bin/env python3
"""Measurement identity, coverage, paired controls and uncertainty contracts."""
import copy
import hashlib
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

from analyze_native_benchmark import analyze, analyze_intervals, analyze_frame_times, duration_metrics, main, parse_builtin, parse_frame_times, profile_log, thermal


class BenchmarkTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix='native-benchmark-analysis-')
        self.root = Path(self.temporary.name)
        self.next_pid = 7

    def tearDown(self):
        self.temporary.cleanup()

    def write(self, path, value):
        path = self.root / path
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(json.dumps(value))
        return path

    def interval_file(self, name='intervals.jsonl', pid=7, gap=False, context=False, drops=False):
        rows = [{'type': 'header', 'schemaVersion': 1, 'pid': pid, 'clock': 'CLOCK_MONOTONIC_ns'}]
        for kind, field in [('guest', 'Guest'), ('source_present_call', 'Source')]:
            for index in range(3):
                if gap and index == 1:
                    continue
                rows.append({'type': 'frame_interval', 'schemaVersion': 1, 'pid': pid, 'boundaryKind': kind,
                             'recordIndex': len(rows),
                             'beginMonotonicNs': 100_000_000 + index * 50_000_000,
                             'endMonotonicNs': 150_000_000 + index * 50_000_000, 'durationNs': 50_000_000,
                             'begin' + field + 'Sequence': index, 'end' + field + 'Sequence': index + 1,
                             'presentationGeneration': 1, 'cacheEpoch': index if context else 0,
                             'phaseWallNs': {'commandsOther': 20_000_000, 'sourcePacing': 30_000_000},
                             'accountedWallNs': 50_000_000, 'coverageComplete': True,
                             'countersAtEnqueue': {'writerQueueDrops': int(drops and index > 0), 'clockFailures': 0, 'stackErrors': 0}})
        path = self.root / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text('\n'.join(json.dumps(row) for row in rows) + '\n')
        return path

    def window(self, pid=7):
        return {'clock': 'CLOCK_MONOTONIC_ns', 'pid': pid,
                'startMonotonicNs': 100_000_000, 'endMonotonicNs': 250_000_000}

    def trial(self, enabled=False, pair='one', feature='pipeline_cache', name=None):
        name = name or ('on' if enabled else 'off')
        directory = self.root / name
        directory.mkdir()
        pid, self.next_pid = self.next_pid, self.next_pid + 1
        config = {'graphics_preset': 'medium', 'spatial_upscale_enabled': True,
                  'frame_interpolation_enabled': True, 'frame_interpolation_output_fps': 40,
                  'pipeline_cache': False, 'occlusion_pool': False,
                  'extra_args': ['-benchmark', '-benchmarkFrameTimes', '-benchmarkIterations=1'],
                  'custom': {'kept': True}, feature: enabled}
        for suffix in ('before', 'after'):
            self.write(name + '/config-' + suffix + '.json', config)
        samples = [{'pid': pid, 'monotonicNs': at, 'guestPresents': frames,
                    'presents': frames * 2, 'renderSize': [1200, 800],
                    'nativeTelemetry': {'sourceTargetFps': 20}}
                   for at, frames in [(100_000_000, 10), (250_000_000, 13)]]
        self.write(name + '/samples.json', samples)
        self.write(name + '/summary.json', {'device': 'same-device', 'bundle': 'same-bundle'})
        self.write(name + '/fg-000.json', {'pid': pid, 'guestFinalSource': {'width': 1200, 'height': 800}})
        result = self.write(name + '/builtin.json', {'kind': 'gta_v_builtin', 'passes': [
            {'pass': 0, 'minFps': 10, 'avgFps': 13 if enabled else 12, 'maxFps': 18, 'reportedFrames': 3}]})
        frame_times = directory / 'frame-times.txt'
        frame_times.write_text('Frame\tTime(ms)\n0\t50.000\n1\t50.000\n2\t50.000\n')
        window = self.window(pid)
        self.write(name + '/binding.json', {'resultSha256': hashlib.sha256(result.read_bytes()).hexdigest(),
                                         'window': window, 'completed': True,
                                         'frameTimesSha256': {'0': hashlib.sha256(frame_times.read_bytes()).hexdigest()},
                                         'completionEvidence': 'fixture explicit completion marker and file inventory'})
        (directory / 'runtime.log').write_text('[config] effective -FrameQueueLimit=3; one canonical argument; default 3\n')
        self.interval_file(name + '/intervals.jsonl', pid)
        manifest = {'schemaVersion': 1, 'trialId': name, 'pairId': pair, 'feature': feature,
                    'enabled': enabled, 'order': int(enabled), 'captureDirectory': name,
                    'benchmark': {'kind': 'gta_v_builtin', 'sceneId': 'fixed-script-v1',
                                  'resultPath': name + '/builtin.json', 'expectedPasses': [0],
                                  'windowScope': 'enclosing_native_monotonic_bracket',
                                  'frameTimes': [{'pass': 0, 'path': name + '/frame-times.txt'}],
                                  'window': window, 'bindingPath': name + '/binding.json'},
                    'runtimeLogPath': name + '/runtime.log', 'frameIntervalsPath': name + '/intervals.jsonl',
                    'conditions': {'packageSha256': 'a' * 64, 'gameSettingsSha256': 'b' * 64,
                                   'queueDepth': 3, 'sourceTargetFps': 20, 'sceneWarmupPasses': 0,
                                   'workloadCacheState': 'first_scene_visit'}}
        return self.write(name + '-manifest.json', manifest)

    def test_labeled_builtin_formats_and_unknown_reports(self):
        csv = self.root / 'result.csv'
        csv.write_text('Pass,Min FPS,Max FPS,Avg FPS\n0,10,20,15\n1,12,25,18\n')
        self.assertTrue(parse_builtin(csv, [0, 1])['completePassTable'])
        text = self.root / 'result.txt'
        text.write_text('Frames Per Second (Higher is better) Min, Max, Avg\nPass 0, 10, 20, 15\n')
        self.assertTrue(parse_builtin(text, [0])['recognized'])
        self.assertFalse(parse_builtin(text, [0, 1])['completePassTable'])
        unrelated = self.write('npu-result.json', {'benchmark': 'NNRT', 'fps': 60, 'status': 'pass'})
        self.assertFalse(parse_builtin(unrelated, [0])['recognized'])

    def declare_device_size(self, path, size, transport=None):
        manifest=json.loads(path.read_text());directory=self.root/manifest['captureDirectory']
        manifest['conditions'].update(baselineMode='unlimited-native-800p',sourceTargetFps=None,
            performanceGoal={'averageFps':30,'onePercentLowFps':20},nativeRenderSize=size)
        if transport is not None:manifest['conditions']['fileTransport']=transport
        path.write_text(json.dumps(manifest))
        for suffix in ('before','after'):
            cp=directory/f'config-{suffix}.json';config=json.loads(cp.read_text())
            config.update(fps_limit=0,spatial_upscale_native_short_edge=800);cp.write_text(json.dumps(config))
        cp=directory/'samples.json';samples=json.loads(cp.read_text())
        for row in samples:row['renderSize']=size;row['nativeTelemetry']['sourceTargetFps']=None
        cp.write_text(json.dumps(samples))
        cp=directory/'fg-000.json';fg=json.loads(cp.read_text());fg['guestFinalSource']={'width':size[0],'height':size[1]};cp.write_text(json.dumps(fg))
        if transport is not None:
            cp=directory/'summary.json';summary=json.loads(cp.read_text());summary['fileTransport']=transport;cp.write_text(json.dumps(summary))

    def test_declared800_size_is_exact_and_pairs_cannot_mix_sizes_or_devices(self):
        paths=[self.trial(False),self.trial(True)]
        transport={'mode':'direct','filesRoot':'/data/app/el2/100/base/same-bundle/haps/entry/files/'}
        for path in paths:self.declare_device_size(path,[1714,800],transport)
        self.assertTrue(analyze(paths)['pairs'][0]['qualifiedConfigurationAndBenchmarkBinding'])
        self.declare_device_size(paths[1],[1800,800],transport)
        self.assertIn('condition_changed:nativeRenderSize',analyze(paths)['pairs'][0]['issues'])
        self.declare_device_size(paths[1],[1714,800],transport)
        cp=self.root/'on/summary.json';summary=json.loads(cp.read_text());summary['device']='other-device';cp.write_text(json.dumps(summary))
        self.assertIn('device_or_bundle_changed',analyze(paths)['pairs'][0]['issues'])

    def test_declared_size_malformed_or_actual_texture_mismatch_is_rejected(self):
        path=self.trial();self.declare_device_size(path,[1714,800])
        cp=self.root/'off/fg-000.json';fg=json.loads(cp.read_text());fg['guestFinalSource']['width']=1200;cp.write_text(json.dumps(fg))
        self.assertIn('actual_source_texture_declared_native_size_unconfirmed',analyze([path])['trials'][0]['qualificationIssues'])
        for size in (None,'1714x800',[1713,800],[1714,802],[True,800],[1714.0,800],[]):
            manifest=json.loads(path.read_text());manifest['conditions']['nativeRenderSize']=size;path.write_text(json.dumps(manifest))
            with self.subTest(size=size):self.assertIn('invalid_declared_native_render_size',analyze([path])['trials'][0]['qualificationIssues'])

    def test_direct_transport_declaration_must_match_collector_and_bundle_root(self):
        path=self.trial();transport={'mode':'direct','filesRoot':'/data/app/el2/100/base/same-bundle/haps/entry/files/'}
        self.declare_device_size(path,[1714,800],transport)
        cp=self.root/'off/summary.json';summary=json.loads(cp.read_text());del summary['fileTransport'];cp.write_text(json.dumps(summary))
        self.assertIn('file_transport_declaration_disagrees_with_collector',analyze([path])['trials'][0]['qualificationIssues'])
        for declaration in (None,{'mode':'auto','filesRoot':'/'},{'mode':'direct','filesRoot':'/data/app/el2/100/base/other/haps/entry/files/'}):
            manifest=json.loads(path.read_text());manifest['conditions']['fileTransport']=declaration;path.write_text(json.dumps(manifest))
            with self.subTest(declaration=declaration):self.assertIn('file_transport_declaration_disagrees_with_collector',analyze([path])['trials'][0]['qualificationIssues'])

    def test_matching_fixed_native_features_and_cpu_profile_form_strict_pair(self):
        paths = [self.trial(False), self.trial(True)]
        for path in paths:
            manifest = json.loads(path.read_text())
            manifest['conditions']['fixedNativeFeatures'] = {'native_render_fastpath': True, 'io_handle_lru': False}
            manifest['conditions']['instrumentation'] = {'cpuProfile': True}
            path.write_text(json.dumps(manifest))
            for suffix in ('before', 'after'):
                config_path = self.root / manifest['captureDirectory'] / f'config-{suffix}.json'
                config = json.loads(config_path.read_text())
                config.update(native_render_fastpath=True, io_handle_lru=False, cpu_profile=True)
                config_path.write_text(json.dumps(config))
        pair = analyze(paths)['pairs'][0]
        self.assertTrue(pair['qualifiedConfigurationAndBenchmarkBinding'])
        self.assertEqual(pair['unapprovedConfigurationKeys'], [])
        manifest = json.loads(paths[1].read_text())
        manifest['conditions']['fixedNativeFeatures']['native_render_fastpath'] = False
        paths[1].write_text(json.dumps(manifest))
        pair = analyze(paths)['pairs'][0]
        self.assertIn('condition_changed:fixedNativeFeatures', pair['issues'])
        self.assertIn('ON:fixed_native_feature_disagrees_with_config:native_render_fastpath', pair['issues'])

    def test_malformed_fixed_features_cannot_declare_graphics_diagnostics_or_tested_flag(self):
        path = self.trial()
        original = json.loads(path.read_text())
        for value in (None, [], {'fps_limit': True}, {'cpu_profile': False}, {'gpu_profile': False},
                      {'render_prepass_diagnostics': True},
                      {'pipeline_cache': True}, {'native_render_fastpath': 1}, {'native_render_fastpath': 'true'}):
            with self.subTest(value=value):
                manifest = copy.deepcopy(original)
                manifest['conditions']['fixedNativeFeatures'] = value
                path.write_text(json.dumps(manifest))
                self.assertIn('invalid_fixed_native_features', analyze([path])['trials'][0]['qualificationIssues'])

    def test_cpu_profile_declaration_must_match_actual_config(self):
        path = self.trial()
        original = json.loads(path.read_text())
        for value in (True, 'false', 0):
            with self.subTest(value=value):
                manifest = copy.deepcopy(original)
                manifest['conditions']['instrumentation'] = {'cpuProfile': value}
                path.write_text(json.dumps(manifest))
                self.assertIn('cpu_profile_instrumentation_disagrees_with_config', analyze([path])['trials'][0]['qualificationIssues'])

    def test_fixed_read_only_pipeline_cache_requires_boolean_matching_config(self):
        path = self.trial()
        manifest = json.loads(path.read_text())
        manifest['conditions']['fixedNativeFeatures'] = {'pipeline_cache_read_only': True}
        path.write_text(json.dumps(manifest))
        self.assertIn('fixed_native_feature_disagrees_with_config:pipeline_cache_read_only', analyze([path])['trials'][0]['qualificationIssues'])
        for suffix in ('before', 'after'):
            config_path = self.root / 'off' / f'config-{suffix}.json'
            config = json.loads(config_path.read_text())
            config['pipeline_cache_read_only'] = True
            config_path.write_text(json.dumps(config))
        self.assertEqual(analyze([path])['trials'][0]['qualificationIssues'], [])
        manifest['conditions']['fixedNativeFeatures']['pipeline_cache_read_only'] = 1
        path.write_text(json.dumps(manifest))
        self.assertIn('invalid_fixed_native_features', analyze([path])['trials'][0]['qualificationIssues'])

    def test_new_native_cpu_and_empty_init_features_require_actual_boolean_config(self):
        for feature in ('inline_upload_borrow', 'native_command_fastpath', 'skip_empty_init_transition',
                        'skip_empty_render_prepass'):
            with self.subTest(feature=feature):
                path = self.trial(True, feature=feature, name=feature)
                self.assertEqual(analyze([path])['trials'][0]['qualificationIssues'], [])
                for suffix in ('before', 'after'):
                    cp = self.root / feature / f'config-{suffix}.json'
                    config = json.loads(cp.read_text())
                    config[feature] = 1
                    cp.write_text(json.dumps(config))
                self.assertIn('manifest_feature_disagrees_with_config', analyze([path])['trials'][0]['qualificationIssues'])

    def test_render_prepass_diagnostics_require_strict_matching_instrumentation(self):
        paths = [self.trial(False, feature='skip_empty_render_prepass'),
                 self.trial(True, feature='skip_empty_render_prepass')]
        for enabled in (False, True):
            for path in paths:
                manifest = json.loads(path.read_text())
                manifest['conditions']['instrumentation'] = {'renderPrepassDiagnostics': enabled}
                path.write_text(json.dumps(manifest))
                for suffix in ('before', 'after'):
                    cp = self.root / manifest['captureDirectory'] / f'config-{suffix}.json'
                    config = json.loads(cp.read_text())
                    config['render_prepass_diagnostics'] = enabled
                    cp.write_text(json.dumps(config))
            self.assertTrue(analyze(paths)['pairs'][0]['qualifiedConfigurationAndBenchmarkBinding'])
        path = paths[1]
        for declared in (False, 'true', 1, None):
            manifest = json.loads(path.read_text())
            manifest['conditions']['instrumentation']['renderPrepassDiagnostics'] = declared
            path.write_text(json.dumps(manifest))
            result = analyze(paths)
            self.assertIn('render_prepass_diagnostics_instrumentation_disagrees_with_config',
                          result['trials'][1]['qualificationIssues'])
            self.assertFalse(result['pairs'][0]['qualifiedConfigurationAndBenchmarkBinding'])
        manifest = json.loads(path.read_text())
        del manifest['conditions']['instrumentation']
        path.write_text(json.dumps(manifest))
        self.assertIn('render_prepass_diagnostics_instrumentation_disagrees_with_config',
                      analyze([path])['trials'][0]['qualificationIssues'])
        manifest['conditions']['instrumentation'] = {'renderPrepassDiagnostics': True}
        path.write_text(json.dumps(manifest))
        for invalid in (0, 1, 'true', None):
            for suffix in ('before', 'after'):
                cp = self.root / manifest['captureDirectory'] / f'config-{suffix}.json'
                config = json.loads(cp.read_text())
                config['render_prepass_diagnostics'] = invalid
                cp.write_text(json.dumps(config))
            self.assertIn('render_prepass_diagnostics_instrumentation_disagrees_with_config',
                          analyze([path])['trials'][0]['qualificationIssues'])

    def test_one_percent_low_uses_slowest_tail_mean_and_strict_slow_runs(self):
        metrics = duration_metrics([10.0] * 198 + [100.0, 200.0])
        self.assertEqual(metrics['onePercentLowSamples'], 2)
        self.assertAlmostEqual(metrics['onePercentLowFps'], 1000 / 150)
        self.assertNotEqual(metrics['onePercentLowFps'], 1000 / 10)
        slow = duration_metrics([50.0, 60.0, 70.0, 20.0, 90.0])['slowFrame50ms']
        self.assertEqual(slow['frames'], 3)
        self.assertEqual(slow['framePercent'], 60)
        self.assertAlmostEqual(slow['framesPerMinute'], 60000 * 3 / 290)
        self.assertEqual(slow['consecutiveRuns'], 2)
        self.assertEqual(slow['longestConsecutiveFrames'], 2)
        self.assertEqual(slow['longestConsecutiveDurationMs'], 130)
        self.assertIsNone(duration_metrics([])['onePercentLowFps'])
        self.assertIsNone(duration_metrics([])['slowFrame50ms']['framesPerMinute'])

    def test_unknown_source_budget_remains_null_but_goal30_is_measured(self):
        parsed = analyze_intervals(self.interval_file(), self.window(), target_fps=None, goal_fps=30)
        source = parsed['chains']['source_present_call']
        self.assertIsNone(source['sourceTargetFps'])
        self.assertIsNone(source['overSourceBudgetIntervals'])
        self.assertEqual(source['overGoalBudgetIntervals'], 3)
        self.assertEqual(source['durationMetrics']['throughputFps'], 20)
        self.assertEqual(source['durationMetrics']['onePercentLowFps'], 20)
        self.assertEqual(source['durationMetrics']['slowFrame50ms']['frames'], 0)
        for target in (0, False, -1):
            with self.subTest(target=target), self.assertRaises(ValueError):
                analyze_intervals(self.interval_file(), self.window(), target_fps=target)

    def test_point_one_percent_low_ceil_boundary_and_maximum_frame(self):
        metrics = duration_metrics([10.0] * 999 + [100.0, 300.0])
        self.assertEqual(metrics['pointOnePercentLowSamples'], 2)
        self.assertEqual(metrics['pointOnePercentLowFps'], 5)
        self.assertEqual(metrics['maximumFrameTimeMs'], 300)

    def test_point_one_percent_low_single_and_empty_sequences_expose_tail_count(self):
        metrics = duration_metrics([25.0])
        self.assertEqual(metrics['pointOnePercentLowSamples'], 1)
        self.assertEqual(metrics['pointOnePercentLowFps'], 40)
        self.assertEqual(metrics['maximumFrameTimeMs'], 25)
        empty = duration_metrics([])
        self.assertEqual(empty['pointOnePercentLowSamples'], 0)
        self.assertIsNone(empty['pointOnePercentLowFps'])
        self.assertIsNone(empty['maximumFrameTimeMs'])

    def test_explicit_unlimited_baseline_null_identity_and_goal_do_not_relabel_native_success(self):
        path = self.trial()
        manifest = json.loads(path.read_text())
        manifest['conditions'].update(baselineMode='unlimited-native-800p', sourceTargetFps=None,
                                      performanceGoal={'averageFps': 30, 'onePercentLowFps': 20})
        path.write_text(json.dumps(manifest))
        for suffix in ('before', 'after'):
            file = self.root / f'off/config-{suffix}.json'
            config = json.loads(file.read_text())
            config.update(fps_limit=0, spatial_upscale_native_short_edge=800)
            file.write_text(json.dumps(config))
        samples_path = self.root / 'off/samples.json'
        samples = json.loads(samples_path.read_text())
        for row in samples:
            row['nativeTelemetry']['sourceTargetFps'] = None
        samples_path.write_text(json.dumps(samples))
        result = analyze([path])['trials'][0]
        self.assertEqual(result['qualificationIssues'], [])
        self.assertEqual(result['sourceTargetObservations'], {'samples': 2, 'explicitNull': 2, 'missing': 0, 'numericValues': []})
        assessment = result['performanceGoalAssessment']
        self.assertFalse(assessment['rawPasses'][0]['meetsRecordedDurationGoal'])
        self.assertFalse(assessment['nativeSourceGoalEstablished'])
        for invalid in (0, 20, 'missing'):
            with self.subTest(invalid=invalid):
                if invalid == 'missing':
                    samples[0]['nativeTelemetry'].pop('sourceTargetFps', None)
                else:
                    samples[0]['nativeTelemetry']['sourceTargetFps'] = invalid
                samples_path.write_text(json.dumps(samples))
                self.assertIn('unlimited_runtime_target_not_explicit_null', analyze([path])['trials'][0]['qualificationIssues'])

    def test_duplicate_pass_and_invalid_numeric_order_rejected(self):
        path = self.write('bad.json', {'kind': 'gta_v_builtin', 'passes': [
            {'pass': 0, 'minFps': 10, 'avgFps': 15, 'maxFps': 20}] * 2})
        with self.assertRaisesRegex(ValueError, 'duplicate'):
            parse_builtin(path, [0])
        value = json.loads(path.read_text())
        value['passes'] = [{'pass': 0, 'minFps': 20, 'avgFps': 15, 'maxFps': 10}]
        path.write_text(json.dumps(value))
        with self.assertRaisesRegex(ValueError, 'ordering'):
            parse_builtin(path, [0])

    def test_synthetic_single_pass_raw_throughput_is_not_reported_fps_mean(self):
        fixture = Path(__file__).parent / 'fixtures/gta_builtin_single_pass'
        report_path = fixture / 'Benchmark-00-00-00-00-00-00.txt'
        raw_path = fixture / 'FrameTimes-Pass0-00-00-00-00-00-00.txt'
        report = parse_builtin(report_path, [0])
        self.assertTrue(report['completePassTable'])
        self.assertEqual(report['passes'][0]['reportedFrames'], 219)
        self.assertEqual(report['passes'][0]['reportedPercentileMs']['p99'], 75)
        self.assertEqual(report['passes'][0]['reportedFrameTimeMs']['avg'], 32.0)
        raw = parse_frame_times(raw_path)
        self.assertTrue(raw['completeFrameSequence'])
        self.assertEqual(raw['frames'], 219)
        self.assertAlmostEqual(raw['totalFrameTimeMs'], 10920)
        self.assertAlmostEqual(raw['meanFrameTimeMs'], 10920 / 219)
        self.assertAlmostEqual(raw['throughputFps'], 1000 * 219 / 10920)
        self.assertAlmostEqual(raw['meanInstantaneousFps'], (110 * 50 + 109 * 12.5) / 219)
        self.assertEqual(raw['frameTimeMs']['p50'], 20)
        self.assertEqual(raw['frameTimeMs']['p95'], 80)
        self.assertEqual(raw['frameTimeMs']['p99'], 80)
        result = analyze_frame_times([{'pass': 0, 'path': str(raw_path)}], report,
                                     {'frameTimesSha256': {'0': raw['provenance']['sha256']}}, fixture, True)
        self.assertTrue(result['qualifiedAllPasses'])
        row = result['passes'][0]
        self.assertLess(abs(row['reportedAvgMinusMeanInstantaneousFps']), 0.0001)
        self.assertGreater(row['reportedAvgMinusThroughputFps'], 5)
        self.assertAlmostEqual(row['reportedMinusRawPercentileMs']['p99'], -5)

    def test_raw_gaps_duplicates_nonfinite_and_missing_header_are_rejected_or_unqualified(self):
        raw = self.root / 'raw.txt'
        raw.write_text('Frame\tTime(ms)\n0\t50\n2\t50\n')
        self.assertFalse(parse_frame_times(raw)['completeFrameSequence'])
        raw.write_text('Frame\tTime(ms)\n0\t50\n0\t50\n')
        self.assertFalse(parse_frame_times(raw)['completeFrameSequence'])
        for duration in ('0', '-1', 'nan', 'inf'):
            raw.write_text('Frame\tTime(ms)\n0\t' + duration + '\n')
            with self.assertRaisesRegex(ValueError, 'positive and finite'):
                parse_frame_times(raw)
        raw.write_text('Frame,Time(ms)\n0,50\n')
        self.assertFalse(parse_frame_times(raw)['recognized'])

    def test_raw_artifact_hash_count_and_pass_set_are_required(self):
        manifest_path = self.trial()
        binding = json.loads((self.root / 'off/binding.json').read_text())
        binding['frameTimesSha256']['0'] = '0' * 64
        self.write('off/binding.json', binding)
        self.assertIn('raw_frame_times_unqualified:0', analyze([manifest_path])['trials'][0]['qualificationIssues'])
        manifest = json.loads(manifest_path.read_text())
        manifest['benchmark']['frameTimes'] = []
        manifest_path.write_text(json.dumps(manifest))
        self.assertIn('raw_frame_time_pass_set_missing_or_changed', analyze([manifest_path])['trials'][0]['qualificationIssues'])
        report_path = self.root / 'off/builtin.json'
        value = json.loads(report_path.read_text())
        value['passes'][0]['reportedFrames'] = 4
        report_path.write_text(json.dumps(value))
        manifest['benchmark']['frameTimes'] = [{'pass': 0, 'path': 'off/frame-times.txt'}]
        manifest_path.write_text(json.dumps(manifest))
        self.assertFalse(analyze([manifest_path])['trials'][0]['rawFrameTimes']['passes'][0]['reportedFrameCountMatches'])

    def test_native_chains_remain_separate_and_exactly_accounted(self):
        report = analyze_intervals(self.interval_file(), self.window())
        for chain in report['chains'].values():
            self.assertEqual(chain['intervalMs']['p99'], 50)
            self.assertEqual(chain['coveredDurationNs'], 150_000_000)
            self.assertTrue(chain['windowFullyCovered'])
            self.assertEqual(sum(chain['exclusivePhaseWallNs'].values()), 150_000_000)
        self.assertNotIn('combinedExclusiveWallNs', report)

    def test_missing_interval_context_change_and_drops_suppress_quantiles(self):
        for option in ('gap', 'context', 'drops'):
            report = analyze_intervals(self.interval_file(option + '.jsonl', **{option: True}), self.window())
            for chain in report['chains'].values():
                self.assertFalse(chain['qualifiedInterior'])
                self.assertIsNone(chain['intervalMs']['p99'])

    def test_pid_clock_and_phase_accounting_cannot_be_guessed(self):
        path = self.interval_file()
        with self.assertRaisesRegex(ValueError, 'PID'):
            analyze_intervals(path, self.window(9))
        self.assertFalse(analyze_intervals(path, None)['qualified'])
        rows = [json.loads(line) for line in path.read_text().splitlines()]
        rows[1]['accountedWallNs'] -= 1
        path.write_text('\n'.join(map(json.dumps, rows)))
        self.assertFalse(analyze_intervals(path, self.window())['chains']['guest']['qualifiedInterior'])

    def test_partial_window_edges_are_exposed(self):
        window = {**self.window(), 'startMonotonicNs': 120_000_000}
        report = analyze_intervals(self.interval_file(), window)
        chain = report['chains']['guest']
        self.assertTrue(chain['qualifiedInterior'])
        self.assertFalse(chain['windowFullyCovered'])
        self.assertEqual(chain['intervals'], 2)
        self.assertEqual(chain['firstBeginMonotonicNs'], 150_000_000)

    def test_clean_drain_and_record_limit_are_distinct_from_window_coverage(self):
        path = self.interval_file()
        self.assertIsNone(analyze_intervals(path, self.window())['producerClosed'])
        counts = {'intervalsAttempted': 6, 'intervalsEnqueued': 6, 'intervalsWritten': 6, 'intervalsFlushed': 6,
                  'recordLimitReached': False, 'writerQueueDrops': 0, 'clockFailures': 0, 'stackErrors': 0,
                  'writerErrors': 0, 'stageEntries': 123, 'clockReads': 456, 'boundaryCalls': 8}
        footer = {'type': 'footer', 'schemaVersion': 1, 'pid': 7, 'producerClosed': True, 'counters': counts}
        with path.open('a') as output:
            output.write(json.dumps(footer) + '\n')
        result = analyze_intervals(path, self.window())
        self.assertTrue(result['sessionCoverageComplete'])
        self.assertFalse(result['observerTimeMeasured'])
        self.assertEqual(result['observerIntensity']['clockReads'], 456)
        text = path.read_text().replace('"recordLimitReached": false', '"recordLimitReached": true')
        path.write_text(text)
        result = analyze_intervals(path, self.window())
        self.assertFalse(result['sessionCoverageComplete'])
        self.assertTrue(result['chains']['guest']['windowFullyCovered'])

    def test_fixed_config_pair_is_qualified_but_never_invents_causal_gain(self):
        off, on = self.trial(False), self.trial(True)
        result = analyze([off, on])
        pair = result['pairs'][0]
        self.assertTrue(pair['qualifiedConfigurationAndBenchmarkBinding'], pair['issues'])
        self.assertEqual(pair['reportedBuiltinDifferences'][0]['reportedAvgFpsDelta'], 1)
        self.assertFalse(pair['thermalAvailableBoth'])
        self.assertFalse(pair['causalPerformanceImprovementEstablished'])
        self.assertEqual(result['trials'][0]['collector']['guestSourceFps'], 20)
        self.assertEqual(result['trials'][0]['benchmark']['passes'][0]['avgFps'], 12)
        self.assertEqual(pair['primaryRawFrameTimeDifferences'][0]['throughputFpsDelta'], 0)
        self.assertEqual(pair['primaryRawFrameTimeDifferences'][0]['rawFrameTimeMsDelta']['p99'], 0)

    def test_cli_cannot_overwrite_raw_inputs_or_capture_and_runs_valid_output(self):
        path = self.trial()
        for protected in (path, self.root / 'off/analysis.json', self.root / 'off/frame-times.txt'):
            before = protected.read_bytes() if protected.exists() else None
            with patch('sys.argv', ['analyze_native_benchmark', '--trial', str(path), '--out', str(protected)]):
                with self.assertRaisesRegex(ValueError, 'outside raw evidence'):
                    main()
            self.assertEqual(protected.read_bytes() if protected.exists() else None, before)
        output = self.root / 'analysis.json'
        with patch('sys.argv', ['analyze_native_benchmark', '--trial', str(path), '--out', str(output)]), patch('builtins.print'):
            main()
        report = json.loads(output.read_text())
        self.assertTrue(report['trials'][0]['rawFrameTimes']['qualifiedAllPasses'])

    def test_missing_report_cannot_be_bound_by_two_absent_hashes(self):
        path = self.trial()
        manifest = json.loads(path.read_text())
        del manifest['benchmark']['resultPath']
        path.write_text(json.dumps(manifest))
        binding = json.loads((self.root / 'off/binding.json').read_text())
        del binding['resultSha256']
        self.write('off/binding.json', binding)
        self.assertFalse(analyze([path])['trials'][0]['artifactBindingQualified'])

    def test_other_flags_extra_args_and_null_versus_missing_are_not_ignored(self):
        off, on = self.trial(False), self.trial(True)
        value = json.loads((self.root / 'on/config-before.json').read_text())
        value['occlusion_pool'] = True
        value['extra_args'].append('-different-quality=1')
        value['unknown_null'] = None
        for suffix in ('before', 'after'):
            (self.root / ('on/config-' + suffix + '.json')).write_text(json.dumps(value))
        pair = analyze([off, on])['pairs'][0]
        self.assertFalse(pair['qualifiedConfigurationAndBenchmarkBinding'])
        self.assertEqual(pair['unapprovedConfigurationKeys'], ['extra_args', 'occlusion_pool', 'unknown_null'])

    def test_workload_cache_warmup_and_package_must_match(self):
        off, on = self.trial(False), self.trial(True)
        value = json.loads(on.read_text())
        value['conditions']['sceneWarmupPasses'] = 1
        value['conditions']['packageSha256'] = 'c' * 64
        on.write_text(json.dumps(value))
        issues = analyze([off, on])['pairs'][0]['issues']
        self.assertIn('condition_changed:sceneWarmupPasses', issues)
        self.assertIn('condition_changed:packageSha256', issues)

    def test_existing_result_without_native_window_binding_is_unqualified(self):
        path = self.trial()
        value = json.loads(path.read_text())
        del value['benchmark']['window']
        path.write_text(json.dumps(value))
        report = analyze([path])['trials'][0]
        self.assertIn('benchmark_native_monotonic_window_missing', report['qualificationIssues'])
        self.assertIn('builtin_artifact_not_bound_to_current_run', report['qualificationIssues'])
        self.assertFalse(report['frameIntervals']['qualified'])

    def test_legacy_nested_profile_samples_are_never_added_or_scaled(self):
        path = self.root / 'cpu.log'
        record = {'windowMs': 10000, 'frames': 200, 'processCpuMs': 42,
                  'wallSpans': {'commands': {'totalMs': 100}, 'queueSubmit': {'totalMs': 70},
                                'drawSample64': {'samples': 2, 'sampleEvery': 64, 'totalMs': 25},
                                'bindingProbeOverheadSample': {'samples': 2, 'totalMs': 1}}}
        path.write_text('[cpu-profile] ' + json.dumps(record) + '\n')
        report = profile_log(path)
        self.assertEqual(report['fileScopeCpuMs'], 42)
        self.assertEqual(report['bindingProbeOverheadSampleMs'], 1)
        self.assertFalse(report['totalInstrumentationOverheadKnown'])
        self.assertIn('unavailable', report['windowAssociation'])
        path.write_text('no clock samples\n')
        self.assertIsNone(profile_log(path)['fileScopeCpuMs'])
        self.assertIsNone(profile_log(path)['bindingProbeOverheadSampleMs'])

    def test_wrong_device_or_same_process_cannot_form_independent_pair(self):
        off, on = self.trial(False), self.trial(True)
        self.write('on/summary.json', {'device': 'different-device', 'bundle': 'same-bundle'})
        self.assertIn('device_or_bundle_changed', analyze([off, on])['pairs'][0]['issues'])

    def test_artifact_hash_mismatch_and_duplicate_trial_ids_rejected(self):
        off, on = self.trial(False), self.trial(True)
        binding = json.loads((self.root / 'on/binding.json').read_text())
        binding['resultSha256'] = '0' * 64
        self.write('on/binding.json', binding)
        self.assertIn('ON:builtin_artifact_not_bound_to_current_run', analyze([off, on])['pairs'][0]['issues'])
        with self.assertRaisesRegex(ValueError, 'duplicate trialId'):
            analyze([off, off])

    def test_reverse_order_replicates_use_runs_not_frame_count(self):
        off, on = self.trial(False), self.trial(True)
        second_off = self.trial(False, pair='two', name='second-off')
        second_on = self.trial(True, pair='two', name='second-on')
        for path, order in [(second_off, 1), (second_on, 0)]:
            value = json.loads(path.read_text())
            value['order'] = order
            path.write_text(json.dumps(value))
        report = analyze([off, on, second_on, second_off])
        self.assertEqual(len(report['replicates']), 1)
        replicate = report['replicates'][0]
        self.assertEqual(replicate['independentPairs'], 2)
        self.assertTrue(replicate['reverseOrdersObserved'])
        self.assertEqual(replicate['reportedPassDeltas']['0']['samples'], 2)
        self.assertIsNone(replicate['confidenceInterval'])

    def test_relabeling_captures_does_not_create_independent_replicates(self):
        off, on = self.trial(False), self.trial(True)
        paths = [off, on]
        for original in (off, on):
            value = json.loads(original.read_text())
            value.update({'trialId': 'copy-' + value['trialId'], 'pairId': 'copy'})
            paths.append(self.write(value['trialId'] + '.json', value))
        with self.assertRaisesRegex(ValueError, 'duplicate capture evidence'):
            analyze(paths)

    def test_distinct_capture_paths_do_not_prove_fresh_process_when_pid_is_reused(self):
        off, on = self.trial(False), self.trial(True)
        second_off = self.trial(False, pair='two', name='second-off')
        second_on = self.trial(True, pair='two', name='second-on')
        # Copy an existing process's evidence to a distinct collection directory;
        # relabeling paths does not establish independent application launches.
        for source, dest in [('off', 'second-off'), ('on', 'second-on')]:
            for file in (self.root / source).iterdir():
                (self.root / dest / file.name).write_bytes(file.read_bytes())
        for source_manifest, dest_manifest in [(off, second_off), (on, second_on)]:
            source = json.loads(source_manifest.read_text())
            dest = json.loads(dest_manifest.read_text())
            dest['benchmark']['window'] = source['benchmark']['window']
            dest_manifest.write_text(json.dumps(dest))
        report = analyze([off, on, second_off, second_on])
        self.assertEqual(report['replicates'], [])
        self.assertTrue(all('process_identity_reused_across_trials_no_fresh_process_proof' in row['qualificationIssues'] for row in report['trials']))

    def test_instrumentation_conditions_must_match_and_devices_do_not_pool_replicates(self):
        off, on = self.trial(False), self.trial(True)
        manifest = json.loads(on.read_text())
        manifest['conditions']['instrumentation'] = {'frameIntervalTrace': False}
        on.write_text(json.dumps(manifest))
        self.assertIn('condition_changed:instrumentation', analyze([off, on])['pairs'][0]['issues'])
        del manifest['conditions']['instrumentation']
        on.write_text(json.dumps(manifest))
        second_off = self.trial(False, pair='two', name='second-off')
        second_on = self.trial(True, pair='two', name='second-on')
        for name in ('second-off', 'second-on'):
            self.write(name + '/summary.json', {'device': 'second-device', 'bundle': 'same-bundle'})
        report = analyze([off, on, second_off, second_on])
        self.assertEqual(len(report['replicates']), 2)
        self.assertEqual([row['independentPairs'] for row in report['replicates']], [1, 1])

    def test_battery_and_thermal_level_snapshots_deduplicate_uptime_without_claiming_core_clock(self):
        snapshot = {'enabled': True, 'active': True, 'fresh': True, 'generation': 2,
                    'sampleGeneration': 2, 'sampleAgeMs': 100,
                    'sample': {'uptimeMs': 90, 'completedUptimeMs': 91, 'batteryTemperatureC': 38.5,
                               'thermalLevel': 3, 'batterySOC': 78, 'pluggedType': 1,
                               'chargingStatus': 2, 'apiFailures': ['frequency unavailable']}}
        rows = [{'pid': 7, 'monotonicNs': at, 'conditions': snapshot} for at in (110_000_000, 130_000_000)]
        rows.append({'pid': 7, 'monotonicNs': 160_000_000, 'conditions': {**snapshot, 'fresh': False}})
        rows.append({'pid': 8, 'monotonicNs': 180_000_000, 'conditions': snapshot})
        path = self.root / 'conditions.jsonl'
        path.write_text('\n'.join(map(json.dumps, rows)))
        report = thermal(path, self.window())
        self.assertTrue(report['available'])
        self.assertEqual(report['uniqueFreshSensorSamples'], 1)
        self.assertEqual(report['associatedStatusSnapshots'], 3)
        self.assertEqual(report['rejectedStaleOrInvalidSnapshots'], 1)
        self.assertEqual(report['batteryTemperatureC']['p50'], 38.5)
        self.assertEqual(report['thermalLevelCounts'], {'3': 1})
        self.assertIsNone(report['controlled'])
        self.assertFalse(report['coreTemperatureKnown'])
        self.assertIn('not assumed identical', report['sensorClock'])
        self.assertEqual(report['sensorCollectionUptimeMs'], [{'begin': 90, 'end': 91}])


if __name__ == '__main__':
    unittest.main()
