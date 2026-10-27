#!/usr/bin/env python3
"""Analyze explicit native-clock PSO scenario windows without accessing a device.

Reuses the production interval coverage audit. Window labels and visual/scene
evidence come from the collector; this analyzer does not infer a camera or scene
change from a successful input command. Guest and source chains overlap in wall
time and are never added together.
"""
import argparse
import json
from pathlib import Path
import re

from analyze_native_benchmark import (analyze_intervals, distribution, integer,
                                      parse_jsonl, provenance, read_json, valid_window)


def scenario_windows(document):
    if (not isinstance(document, dict) or document.get('schemaVersion') != 1
            or document.get('clock') != 'CLOCK_MONOTONIC_ns'
            or not integer(document.get('pid')) or document['pid'] <= 0
            or not isinstance(document.get('windows'), list) or not document['windows']):
        raise ValueError('require schemaVersion1, CLOCK_MONOTONIC_ns, positive PID and nonempty windows')
    found = set()
    result = []
    for item in document['windows']:
        if not isinstance(item, dict) or not isinstance(item.get('name'), str) or not item['name'].strip():
            raise ValueError('scenario window requires a nonempty name')
        if item['name'] in found:
            raise ValueError('scenario window names must be unique')
        found.add(item['name'])
        if (not isinstance(item.get('evidence'), (str, dict, list)) or not item['evidence']
                or (isinstance(item['evidence'], str) and not item['evidence'].strip())):
            raise ValueError('scenario requires evidence metadata for its label and native clock boundaries')
        window = {**item, 'pid': document['pid'], 'clock': document['clock']}
        if not valid_window(window):
            raise ValueError('scenario requires explicit increasing native monotonic boundaries')
        # A separate process must have its own document/trace, never a forged
        # inherited PID for a game-menu selected-save restart.
        if 'pid' in item and item['pid'] != document['pid']:
            raise ValueError('scenario PID differs from its trace document')
        if 'clock' in item and item['clock'] != document['clock']:
            raise ValueError('scenario clock differs from its trace document')
        result.append(window)
    return result


def threshold_counts(rows):
    return {f'over{milliseconds}Ms': sum(row['durationNs'] > milliseconds * 1_000_000 for row in rows)
            for milliseconds in (100, 500, 1000)}


def pso_log(path):
    if path is None:
        return {'available': False}
    entries = []
    decoder = json.JSONDecoder()
    for index, line in enumerate(path.read_text().splitlines(), 1):
        match = re.search(r'\bpso-warmup( loading complete)?:\s*(\{)', line)
        if not match:
            continue
        value, _ = decoder.raw_decode(line[match.start(2):])
        if not isinstance(value, dict):
            raise ValueError('PSO log statistics must be objects')
        entries.append({'line': index, 'phase': 'loading_complete' if match.group(1) else 'runtime',
                        'statistics': value})
    return {'available': True, 'provenance': provenance(path), 'entries': entries,
            'loadingEntries': sum(item['phase'] == 'loading_complete' for item in entries),
            'latestStatistics': entries[-1]['statistics'] if entries else None,
            'scope': 'collector-supplied session log; individual PSO log lines carry no PID or monotonic timestamp; not assigned to scenario windows'}


def slow_spans(path, pid, windows):
    if path is None:
        return {'available': False}
    rows = parse_jsonl(path)
    headers = [row for row in rows if row.get('type') == 'header']
    if (len(headers) != 1 or headers[0].get('pid') != pid
            or headers[0].get('clock') != 'CLOCK_MONOTONIC_ns'):
        raise ValueError('slow trace requires one matching PID and native monotonic header')
    spans = [row for row in rows if row.get('type') == 'slow_operation']
    for row in spans:
        begin, end = row.get('beginMonotonicNs'), row.get('endMonotonicNs')
        if (row.get('pid') != pid or not integer(begin) or not integer(end) or not 0 <= begin < end
                or not integer(row.get('wallNs')) or row['wallNs'] != end - begin):
            raise ValueError('invalid slow trace PID/clock/duration')
    pso = [row for row in spans if row.get('stage') in
           ('pso_loading_warmup', 'pso_warmup_compile', 'pso_draw_miss')]
    def report(selected):
        return {stage: {'spans': len(stage_rows), 'wallNs': sum(row['wallNs'] for row in stage_rows),
                        'wallMs': distribution([row['wallNs'] / 1e6 for row in stage_rows])}
                for stage in sorted({row['stage'] for row in selected})
                for stage_rows in [[row for row in selected if row['stage'] == stage]]}
    return {'available': True, 'provenance': provenance(path), 'header': headers[0],
            'allRetainedPsoSpans': report(pso),
            'scenarioInteriorPsoSpans': {window['name']: report([row for row in pso if
                window['startMonotonicNs'] <= row['beginMonotonicNs']
                and row['endMonotonicNs'] <= window['endMonotonicNs']]) for window in windows},
            'retainedWriterQueueDropsMax': max((row.get('writerQueueDrops', 0) for row in spans), default=0),
            'retainedEventLimitReached': any(row.get('eventLimitReached') for row in spans),
            'scope': 'thresholded retained nested host spans; absence is not zero cost; stages overlap and must not be summed; no full-session completeness proof'}


def analyze(trace, windows_path, log=None, slow_trace=None):
    document = read_json(windows_path)
    windows = scenario_windows(document)
    rows = parse_jsonl(trace)
    source_rows = [row for row in rows if row.get('type') == 'frame_interval'
                   and row.get('boundaryKind') == 'source_present_call']
    scenarios = []
    for window in windows:
        audit = analyze_intervals(trace, window, target_fps=None)
        source = audit.get('chains', {}).get('source_present_call', {})
        start, end = window['startMonotonicNs'], window['endMonotonicNs']
        interior = [row for row in source_rows if start <= row['beginMonotonicNs'] and row['endMonotonicNs'] <= end]
        crossing = [row for row in source_rows if row['beginMonotonicNs'] < end and row['endMonotonicNs'] > start
                    and not (start <= row['beginMonotonicNs'] and row['endMonotonicNs'] <= end)]
        qualified = source.get('qualifiedInterior') is True
        scenarios.append({'name': window['name'], 'window': window,
            'sourceInterval': source, 'sourceLongIntervalCounts': threshold_counts(interior) if qualified else None,
            'pipelineCreateWallTotalNs': source.get('exclusivePhaseWallNs', {}).get('pipelineCreate') if qualified else None,
            'pipelineCreatePerIntervalMaxNs': max((row['phaseWallNs'].get('pipelineCreate', 0) for row in interior), default=0) if qualified else None,
            'longestSourceIntervals': sorted(interior, key=lambda row: row['durationNs'], reverse=True)[:10] if qualified else [],
            'excludedBoundaryCrossingIntervals': crossing,
            'intervalTraceSessionCoverageComplete': audit.get('sessionCoverageComplete'),
            'interpretation': 'source host-present-call intervals; thresholds are strict >100/500/1000ms, not perceptually verified stutter events; edge-crossing intervals are retained separately and never split or prorated'})
    return {'schemaVersion': 1, 'pid': document['pid'], 'clock': document['clock'],
            'traceProvenance': provenance(trace), 'windowsProvenance': provenance(windows_path),
            'scenarios': scenarios, 'psoLog': pso_log(log),
            'slowTrace': slow_spans(slow_trace, document['pid'], windows),
            'scope': 'pure local analysis; collector must establish same package/config/cache and actual scenario execution; no GPU duration, scanout FPS or input-to-display latency'}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--trace', type=Path, required=True)
    parser.add_argument('--windows', type=Path, required=True)
    parser.add_argument('--log', type=Path)
    parser.add_argument('--slow-trace', type=Path)
    parser.add_argument('--output', type=Path)
    args = parser.parse_args()
    result = analyze(args.trace, args.windows, args.log, args.slow_trace)
    serialized = json.dumps(result, ensure_ascii=False, indent=2, allow_nan=False) + '\n'
    if args.output:
        # Raw evidence and earlier analysis are preserved by default.
        with args.output.open('x') as output:
            output.write(serialized)
    else:
        print(serialized, end='')


if __name__ == '__main__':
    main()
