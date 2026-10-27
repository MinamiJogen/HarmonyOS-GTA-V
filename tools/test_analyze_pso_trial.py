#!/usr/bin/env python3
"""PSO scenario measurement provenance and interval-coverage contracts."""
import copy
import json
from pathlib import Path
import tempfile
import unittest

from analyze_pso_trial import analyze, scenario_windows, slow_spans


class PsoAnalysisTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix='pso-window-analysis-')
        self.root = Path(self.temporary.name)
        self.rows = [{'type': 'header', 'schemaVersion': 1, 'pid': 7,
                      'clock': 'CLOCK_MONOTONIC_ns', 'phaseNames': ['pipelineCreate', 'loopOther']}]
        begin = 100_000_000
        for index, duration in enumerate((100, 101, 500, 501, 1000, 1001), 1):
            ns = duration * 1_000_000
            self.rows.append({'type': 'frame_interval', 'schemaVersion': 1, 'pid': 7,
                'recordIndex': index, 'boundaryKind': 'source_present_call',
                'beginMonotonicNs': begin, 'endMonotonicNs': begin + ns, 'durationNs': ns,
                'beginSourceSequence': index, 'endSourceSequence': index + 1,
                'presentationGeneration': 1, 'cacheEpoch': 1,
                'phaseWallNs': {'pipelineCreate': ns // 2, 'loopOther': ns - ns // 2},
                'accountedWallNs': ns, 'coverageComplete': True,
                'countersAtEnqueue': {'writerQueueDrops': 0, 'clockFailures': 0, 'stackErrors': 0}})
            begin += ns
        self.document = {'schemaVersion': 1, 'clock': 'CLOCK_MONOTONIC_ns', 'pid': 7,
                         'windows': [{'name': 'first_entry', 'startMonotonicNs': 100_000_000,
                                      'endMonotonicNs': begin, 'evidence': 'observed world status bracket'}]}

    def tearDown(self):
        self.temporary.cleanup()

    def artifacts(self):
        trace, windows = self.root / 'trace.jsonl', self.root / 'windows.json'
        trace.write_text(''.join(json.dumps(row) + '\n' for row in self.rows))
        windows.write_text(json.dumps(self.document))
        return trace, windows

    def test_strict_counts_phase_totals_and_preserved_evidence(self):
        result = analyze(*self.artifacts())
        scenario = result['scenarios'][0]
        self.assertEqual(scenario['sourceLongIntervalCounts'], {'over100Ms': 5, 'over500Ms': 3, 'over1000Ms': 1})
        self.assertEqual(scenario['pipelineCreateWallTotalNs'], 1_601_500_000)
        self.assertEqual(scenario['pipelineCreatePerIntervalMaxNs'], 500_500_000)
        self.assertEqual(scenario['sourceInterval']['intervalMs']['max'], 1001)
        self.assertEqual(scenario['window']['evidence'], 'observed world status bracket')
        self.assertFalse(scenario['intervalTraceSessionCoverageComplete'])
        self.assertEqual(result['traceProvenance']['bytes'], self.artifacts()[0].stat().st_size)

    def test_boundary_crossing_interval_is_retained_without_prorating(self):
        self.document['windows'][0]['startMonotonicNs'] = 150_000_000
        scenario = analyze(*self.artifacts())['scenarios'][0]
        self.assertEqual(scenario['sourceInterval']['intervals'], 5)
        self.assertEqual(len(scenario['excludedBoundaryCrossingIntervals']), 1)
        self.assertEqual(scenario['excludedBoundaryCrossingIntervals'][0]['durationNs'], 100_000_000)
        self.assertFalse(scenario['sourceInterval']['windowFullyCovered'])

    def test_missing_interior_interval_suppresses_comparison_metrics(self):
        del self.rows[3]
        scenario = analyze(*self.artifacts())['scenarios'][0]
        self.assertFalse(scenario['sourceInterval']['qualifiedInterior'])
        self.assertIn('missing_or_overlapping_interval', scenario['sourceInterval']['issues'])
        self.assertIsNone(scenario['sourceLongIntervalCounts'])
        self.assertIsNone(scenario['pipelineCreateWallTotalNs'])

    def test_bad_phase_partition_suppresses_metrics(self):
        self.rows[2]['phaseWallNs']['loopOther'] -= 1
        scenario = analyze(*self.artifacts())['scenarios'][0]
        self.assertIsNone(scenario['sourceLongIntervalCounts'])

    def test_wrong_trace_pid_rejected(self):
        self.document['pid'] = 8
        with self.assertRaisesRegex(ValueError, 'PID'):
            analyze(*self.artifacts())

    def test_cross_process_window_rejected(self):
        self.document['windows'][0]['pid'] = 8
        with self.assertRaisesRegex(ValueError, 'PID'):
            scenario_windows(self.document)

    def test_duplicate_scenario_name_rejected(self):
        self.document['windows'].append(copy.deepcopy(self.document['windows'][0]))
        with self.assertRaisesRegex(ValueError, 'unique'):
            scenario_windows(self.document)

    def test_scenario_label_requires_evidence(self):
        del self.document['windows'][0]['evidence']
        with self.assertRaisesRegex(ValueError, 'evidence'):
            scenario_windows(self.document)

    def test_pso_statistics_keep_source_lines_and_do_not_claim_scenario_timestamps(self):
        log = self.root / 'session.log'
        log.write_text('[gpu] pso-warmup store: {"loaded":99}\n'
                       '[gpu] pso-warmup loading complete: {"built":4,"warmFirstDraws":0}\n'
                       '[gpu] pso-warmup: {"built":4,"warmFirstDraws":3}\n')
        result = analyze(*self.artifacts(), log=log)['psoLog']
        self.assertEqual(len(result['entries']), 2)
        self.assertEqual(result['entries'][0]['line'], 2)
        self.assertEqual(result['loadingEntries'], 1)
        self.assertEqual(result['latestStatistics']['warmFirstDraws'], 3)

    def test_slow_nested_stages_reported_separately(self):
        slow = self.root / 'slow.jsonl'
        rows = [{'type': 'header', 'pid': 7, 'clock': 'CLOCK_MONOTONIC_ns', 'thresholdNsExclusive': 10_000_000}]
        for stage, begin, end in [('pso_loading_warmup', 110_000_000, 180_000_000),
                                  ('pso_warmup_compile', 120_000_000, 150_000_000)]:
            rows.append({'type': 'slow_operation', 'pid': 7, 'stage': stage,
                         'beginMonotonicNs': begin, 'endMonotonicNs': end, 'wallNs': end-begin,
                         'writerQueueDrops': 0, 'eventLimitReached': False})
        slow.write_text(''.join(json.dumps(row)+'\n' for row in rows))
        result = analyze(*self.artifacts(), slow_trace=slow)['slowTrace']
        self.assertEqual(result['allRetainedPsoSpans']['pso_loading_warmup']['wallNs'], 70_000_000)
        self.assertEqual(result['allRetainedPsoSpans']['pso_warmup_compile']['wallNs'], 30_000_000)
        self.assertNotIn('totalWallNs', result)
        with self.assertRaisesRegex(ValueError, 'PID'):
            slow_spans(slow, 8, scenario_windows(self.document))


if __name__ == '__main__':
    unittest.main()
