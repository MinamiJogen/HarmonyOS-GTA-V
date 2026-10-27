#!/usr/bin/env python3
"""Semantic scene identity, status provenance and unsplit edge interval tests."""
import json
from pathlib import Path
import tempfile
import unittest

from derive_pso_scene_windows import classify, derive, SCENES


class SceneWindowsTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix='pso-scene-windows-')
        self.root = Path(self.temporary.name)
        self.trace = self.root/'frame-intervals.jsonl'
        rows = [{'type': 'header', 'pid': 7, 'clock': 'CLOCK_MONOTONIC_ns'}]
        for start, end in [(0, 5), (5, 10), (10, 20)]:
            rows.append({'type': 'frame_interval', 'pid': 7, 'boundaryKind': 'source_present_call',
                         'beginMonotonicNs': start*1_000_000_000, 'endMonotonicNs': end*1_000_000_000})
        self.trace.write_text(''.join(json.dumps(row)+'\n' for row in rows))
        self.status(0, 1, 'city')
        self.status(1, 3, 'hill')
        self.status(2, 14, 'hill')

    def tearDown(self):
        self.temporary.cleanup()

    def status(self, index, at, scene, pid=7, known=True, source='GetEntityCoords(player,false)', foreground=True):
        coords = dict(SCENES)[scene] if scene else (0, 0, 0)
        runtime = {'processId': pid, 'appForeground': foreground,
                   'nativeTelemetry': {'clock': 'CLOCK_MONOTONIC_ns', 'monotonicNs': round(at*1_000_000_000)},
                   'touchControlTransport': {'inputObservation': {'seq': index+1, 'generation': 0,
                       'position': {'known': known, 'source': source, 'generation': 0,
                                    'value': dict(zip(('x', 'y', 'z'), coords))}}}}
        (self.root/f'status-{index:04}.json').write_text(json.dumps({'runtimeJson': json.dumps(runtime), 'uiAppForeground': foreground}))

    def result(self):
        return derive(self.root, self.trace, transitions=[('city', 'hill')])

    def test_semantic_bracket_expands_to_whole_source_intervals(self):
        value = self.result()
        window = value['windows'][0]
        self.assertEqual(window['name'], 'scene_city_to_hill')
        self.assertEqual(window['startMonotonicNs'], 0)
        self.assertEqual(window['endMonotonicNs'], 20_000_000_000)
        evidence = window['evidence']
        self.assertEqual(evidence['startExpansionNs'], 1_000_000_000)
        self.assertEqual(evidence['endExpansionNs'], 6_000_000_000)
        self.assertEqual(evidence['lastBeforeStatus']['path'], 'status-0000.json')
        self.assertEqual(evidence['firstAfterStatus']['path'], 'status-0001.json')
        self.assertEqual(evidence['tailEndStatus']['path'], 'status-0002.json')

    def test_wrong_process_snapshot_cannot_establish_destination(self):
        self.status(1, 3, 'hill', pid=8)
        with self.assertRaisesRegex(ValueError, 'insufficient_destination_tail'):
            self.result()

    def test_unknown_observation_cannot_establish_city(self):
        self.status(0, 1, 'city', known=False)
        with self.assertRaisesRegex(ValueError, 'predecessor'):
            self.result()

    def test_guessed_position_cannot_establish_city(self):
        self.status(0, 1, 'city', source='proposal')
        with self.assertRaisesRegex(ValueError, 'predecessor'):
            self.result()

    def test_foreground_required(self):
        self.status(0, 1, 'city', foreground=False)
        with self.assertRaisesRegex(ValueError, 'predecessor'):
            self.result()

    def test_actual_destination_tail_required(self):
        self.status(2, 12, 'hill')
        with self.assertRaisesRegex(ValueError, 'insufficient_destination_tail'):
            self.result()

    def test_returning_to_destination_does_not_forge_a_continuous_tail(self):
        self.status(2, 8, 'desert')
        self.status(3, 14, 'hill')
        with self.assertRaisesRegex(ValueError, 'continuously'):
            self.result()

    def test_unknown_intermediate_scene_prevents_wrong_pair(self):
        self.status(0, 1, 'city')
        self.status(1, 2, None)
        self.status(2, 3, 'hill')
        self.status(3, 14, 'hill')
        with self.assertRaisesRegex(ValueError, 'predecessor'):
            self.result()

    def test_missing_scene_transitions_remain_explicit(self):
        result = derive(self.root, self.trace)
        self.assertEqual(len(result['windows']), 1)
        self.assertIn('hill_to_desert:destination_not_observed', result['missingRequestedTransitions'])

    def test_coordinate_matching_has_meter_tolerance(self):
        self.assertEqual(classify((-9.8, -1474.1, 30.5)), 'city')
        self.assertIsNone(classify((0, 0, 0)))

    def test_canonical_city_entry_and_scripted_camera_require_city_identity(self):
        self.status(0, .5, None)
        self.status(1, 1, 'city')
        self.status(2, 8, 'city')
        self.status(3, 12, 'city')
        self.status(4, 14, 'hill')
        self.status(5, 25, 'hill')
        result = derive(self.root, self.trace, transitions=[('city', 'hill')], include_city_segments=True)
        entry, camera = result['windows'][:2]
        self.assertEqual(entry['name'], 'canonical_city_entry')
        self.assertEqual(camera['name'], 'benchmark_scripted_camera_city')
        self.assertEqual(entry['evidence']['beginStatus']['path'], 'status-0000.json')
        self.assertEqual(entry['evidence']['requestedEndMonotonicNs'], 8_000_000_000)
        self.assertEqual(camera['evidence']['requestedStartMonotonicNs'], 8_000_000_000)
        self.assertEqual(camera['evidence']['requestedEndMonotonicNs'], 12_000_000_000)
        self.assertEqual(camera['startMonotonicNs'], 5_000_000_000)
        self.assertEqual(camera['endMonotonicNs'], 20_000_000_000)


if __name__ == '__main__':
    unittest.main()
