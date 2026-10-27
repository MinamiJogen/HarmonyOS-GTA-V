#!/usr/bin/env python3
"""Derive benchmark scene brackets from saved engine position observations.

Reads local evidence only. No marker is an exact teleport timestamp. Native
status clocks bracket the coordinate change; source interval expansion preserves
whole crossing intervals without dividing their durations between scenarios.
"""
import argparse
import json
import math
from pathlib import Path
import re

from analyze_native_benchmark import integer, number, parse_jsonl, provenance, read_json

SCENES = [
    ('city', (-9.8369, -1474.0880, 30.5453)),
    ('hill', (806.0358, 1101.2482, 307.1000)),
    ('desert', (2006.0, 3803.6, 32.2)),
    ('coast', (-1713.3, -1077.5, 13.0)),
    ('forest', (-663.8303, 4484.1050, 70.2756)),
]


def classify(position, tolerance=10.0):
    matches = [name for name, expected in SCENES if math.dist(position, expected) <= tolerance]
    if len(matches) > 1:
        raise ValueError('scene coordinate match is ambiguous')
    return matches[0] if matches else None


def observations(paths, pid, tolerance):
    result = []
    for path in paths:
        wrapper = read_json(path)
        runtime = json.loads(wrapper['runtimeJson'])
        # Startup may expose the preceding process snapshot. Preserve it on disk
        # but never assign it to this process's semantic scenes.
        if runtime.get('processId') != pid:
            continue
        native = runtime.get('nativeTelemetry') or {}
        clock = native.get('monotonicNs')
        if native.get('clock') != 'CLOCK_MONOTONIC_ns' or not integer(clock) or clock < 0:
            continue
        if (wrapper.get('uiAppForeground') is not True or runtime.get('appForeground') is not True
                or runtime.get('error') or any(runtime.get(key) for key in
                    ('presentationPaused', 'stopRequested', 'saveLoadPending', 'surfaceRecoveryRequired'))):
            continue
        observed = runtime.get('touchControlTransport', {}).get('inputObservation') or {}
        position = observed.get('position') or {}
        if position.get('known') is not True or position.get('source') != 'GetEntityCoords(player,false)':
            continue
        value = position.get('value')
        if not isinstance(value, dict):
            continue
        coordinates = [value.get(axis) for axis in ('x', 'y', 'z')]
        if not all(number(value) for value in coordinates):
            continue
        result.append({'path': path.name, 'provenance': provenance(path), 'pid': pid,
                       'monotonicNs': clock, 'coordinates': coordinates,
                       'scene': classify(coordinates, tolerance), 'positionObservation': position,
                       'observationSequence': observed.get('seq'),
                       'observationGeneration': observed.get('generation')})
    result.sort(key=lambda row: row['monotonicNs'])
    clocks = [row['monotonicNs'] for row in result]
    if len(set(clocks)) != len(clocks):
        raise ValueError('status native timestamps are duplicated')
    return result


def expand_to_source_intervals(start, end, rows):
    selected = [row for row in rows if row['beginMonotonicNs'] < end and row['endMonotonicNs'] > start]
    if not selected:
        raise ValueError('no retained source interval overlaps semantic scene bracket')
    return min(start, min(row['beginMonotonicNs'] for row in selected)), max(end, max(row['endMonotonicNs'] for row in selected))


def native_snapshots(paths, pid):
    result = []
    for path in paths:
        wrapper = read_json(path)
        runtime = json.loads(wrapper['runtimeJson'])
        native = runtime.get('nativeTelemetry') or {}
        clock = native.get('monotonicNs')
        if (runtime.get('processId') == pid and native.get('clock') == 'CLOCK_MONOTONIC_ns'
                and integer(clock) and clock >= 0 and wrapper.get('uiAppForeground') is True
                and runtime.get('appForeground') is True and not runtime.get('error')
                and not any(runtime.get(key) for key in ('presentationPaused', 'stopRequested', 'saveLoadPending', 'surfaceRecoveryRequired'))):
            result.append({'path': path.name, 'provenance': provenance(path), 'pid': pid,
                           'monotonicNs': clock, 'worldPlayerObserved': runtime.get('worldPlayerObserved'),
                           'guestPresents': runtime.get('guestPresents')})
    result.sort(key=lambda row: row['monotonicNs'])
    return result


def scene_log_markers(path, tolerance):
    if not path.is_file():
        return {'available': False}
    markers = []
    for line_number, line in enumerate(path.read_text().splitlines(), 1):
        match = re.search(r'COMMAND_CLEAR_AREA from "benchmark" Position \(([-\d.]+),([-\d.]+),([-\d.]+)\)', line)
        if match:
            coords = [float(value) for value in match.groups()]
            markers.append({'line': line_number, 'scene': classify(coords, tolerance), 'coordinates': coords})
    return {'available': True, 'provenance': provenance(path), 'markers': markers,
            'scope': 'engine benchmark coordinate corroboration; these log lines have no native monotonic timestamp and never set timing boundaries'}


def city_segments(observed, snapshots, source, log_markers):
    cities = [row for row in observed if row['scene'] == 'city']
    if not cities:
        return [], ['canonical_city_segments:city_not_observed']
    first_city = cities[0]
    pre_city = [row for row in snapshots if row['monotonicNs'] < first_city['monotonicNs']]
    if not pre_city:
        return [], ['canonical_city_entry:pre_city_native_snapshot_missing']
    stable_city = [row for row in cities if row['monotonicNs'] >= first_city['monotonicNs']+5_000_000_000]
    if not stable_city:
        return [], ['canonical_city_segments:five_second_city_tail_missing']
    stable_start = stable_city[0]
    if any(row['scene'] != 'city' for row in observed
           if first_city['monotonicNs'] <= row['monotonicNs'] <= stable_start['monotonicNs']):
        return [], ['canonical_city_segments:city_tail_not_continuous']
    hill = [row for row in observed if row['scene'] == 'hill' and row['monotonicNs'] > stable_start['monotonicNs']]
    if not hill:
        return [], ['canonical_city_segments:later_hill_not_observed']
    before_hill = [row for row in observed if row['monotonicNs'] < hill[0]['monotonicNs']]
    if before_hill[-1]['scene'] != 'city':
        return [], ['canonical_city_segments:city_not_immediate_hill_predecessor']
    last_city = before_hill[-1]
    windows, missing = [], []
    for name, begin, end, scope in [
        ('canonical_city_entry', pre_city[-1], stable_start,
         'benchmark canonical city establishment: prior native snapshot through first known city position plus at least5s; does not claim ordinary Demo first-entry acceptance'),
        ('benchmark_scripted_camera_city', stable_start, last_city,
         'same benchmark scripted city camera workload after first known city position plus at least5s and before hill teleport; no touch input or exact camera-path phase boundary is inferred'),
    ]:
        if begin['monotonicNs'] >= end['monotonicNs']:
            missing.append(name+':empty_semantic_interval')
            continue
        start, finish = expand_to_source_intervals(begin['monotonicNs'], end['monotonicNs'], source)
        windows.append({'name': name, 'startMonotonicNs': start, 'endMonotonicNs': finish,
            'evidence': {'method': 'canonical city coordinate identity and native snapshot bracket expanded to whole overlapping source intervals',
                         'beginStatus': begin, 'endStatus': end, 'firstCityStatus': first_city,
                         'firstHillStatus': hill[0], 'requestedStartMonotonicNs': begin['monotonicNs'],
                         'requestedEndMonotonicNs': end['monotonicNs'],
                         'startExpansionNs': begin['monotonicNs']-start, 'endExpansionNs': finish-end['monotonicNs'],
                         'benchmarkLogCorroboration': log_markers,
                         'scope': scope+'; no exact teleport or first-visible-frame time; edge intervals included whole and never split; windows can overlap and must not be added'}})
    return windows, missing


def derive(directory, trace, tail_seconds=10.0, tolerance=10.0, transitions=None, include_city_segments=False):
    if not number(tail_seconds) or tail_seconds <= 0 or not number(tolerance) or not 0 < tolerance < 100:
        raise ValueError('require positive finite tail seconds and coordinate tolerance between0 and100m')
    rows = parse_jsonl(trace)
    headers = [row for row in rows if row.get('type') == 'header']
    if (len(headers) != 1 or headers[0].get('clock') != 'CLOCK_MONOTONIC_ns'
            or not integer(headers[0].get('pid')) or headers[0]['pid'] <= 0):
        raise ValueError('require one native monotonic trace header with explicit PID')
    pid = headers[0]['pid']
    source = [row for row in rows if row.get('type') == 'frame_interval' and row.get('boundaryKind') == 'source_present_call']
    for row in source:
        if (row.get('pid') != pid or not integer(row.get('beginMonotonicNs'))
                or not integer(row.get('endMonotonicNs'))
                or not 0 <= row['beginMonotonicNs'] < row['endMonotonicNs']):
            raise ValueError('invalid source interval PID or boundaries')
    observed = observations(sorted(directory.glob('status-*.json')), pid, tolerance)
    wanted = transitions or [(SCENES[i][0], SCENES[i+1][0]) for i in range(len(SCENES)-1)]
    windows, missing = [], []
    for before, after in wanted:
        # A benchmark performs each semantic scene once. Require the expected
        # predecessor before its first observed destination, never a random
        # coordinate jump from an unrelated session.
        destinations = [row for row in observed if row['scene'] == after]
        if not destinations:
            missing.append(f'{before}_to_{after}:destination_not_observed')
            continue
        first_after = destinations[0]
        preceding = [row for row in observed if row['monotonicNs'] < first_after['monotonicNs']]
        if not preceding or preceding[-1]['scene'] != before:
            missing.append(f'{before}_to_{after}:immediate_predecessor_not_observed')
            continue
        last_before = preceding[-1]
        tail = [row for row in observed if row['scene'] == after
                and row['monotonicNs'] >= first_after['monotonicNs'] + round(tail_seconds * 1e9)]
        if not tail:
            missing.append(f'{before}_to_{after}:insufficient_destination_tail')
            continue
        tail_end = tail[0]
        if any(row['scene'] != after for row in observed
               if first_after['monotonicNs'] <= row['monotonicNs'] <= tail_end['monotonicNs']):
            missing.append(f'{before}_to_{after}:destination_not_continuously_observed_through_tail')
            continue
        requested_start, requested_end = last_before['monotonicNs'], tail_end['monotonicNs']
        start, end = expand_to_source_intervals(requested_start, requested_end, source)
        windows.append({'name': f'scene_{before}_to_{after}', 'startMonotonicNs': start, 'endMonotonicNs': end,
            'evidence': {'method': 'known GetEntityCoords engine observations matched by position; native snapshot bracket expanded to whole overlapping source intervals',
                         'beforeScene': before, 'afterScene': after, 'coordinateToleranceMeters': tolerance,
                         'tailSecondsRequested': tail_seconds, 'lastBeforeStatus': last_before,
                         'firstAfterStatus': first_after, 'tailEndStatus': tail_end,
                         'requestedStartMonotonicNs': requested_start, 'requestedEndMonotonicNs': requested_end,
                         'startExpansionNs': requested_start-start, 'endExpansionNs': end-requested_end,
                         'scope': 'observed snapshot bracket, not exact teleport, render-scene activation or first-visible-frame time; edge intervals included whole and never split; scene windows can overlap and must not be added'}})
    if include_city_segments:
        city, city_missing = city_segments(observed, native_snapshots(sorted(directory.glob('status-*.json')), pid), source,
                                           scene_log_markers(directory/'runtime-session.log', tolerance))
        windows = city + windows
        missing.extend(city_missing)
    if not windows:
        raise ValueError('no semantic transition window could be established: ' + '; '.join(missing))
    return {'schemaVersion': 1, 'pid': pid, 'clock': 'CLOCK_MONOTONIC_ns', 'windows': windows,
            'missingRequestedTransitions': missing, 'traceProvenance': provenance(trace),
            'observations': observed, 'sceneDefinitions': [{'name': name, 'coordinates': list(coords)} for name, coords in SCENES],
            'scope': 'derived local benchmark scene brackets only; compare same semantic scenes and fixed package/config/cache, not equal startup elapsed seconds'}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--directory', type=Path, required=True)
    parser.add_argument('--trace', type=Path)
    parser.add_argument('--tail-seconds', type=float, default=10.0)
    parser.add_argument('--tolerance-meters', type=float, default=10.0)
    parser.add_argument('--transition', action='append', choices=[f'{SCENES[i][0]}:{SCENES[i+1][0]}' for i in range(len(SCENES)-1)])
    parser.add_argument('--include-city-segments', action='store_true', help='also derive canonical city establishment and scripted city camera brackets')
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    transitions = [tuple(value.split(':')) for value in args.transition] if args.transition else None
    value = derive(args.directory, args.trace or args.directory/'frame-intervals.jsonl', args.tail_seconds, args.tolerance_meters, transitions, args.include_city_segments)
    with args.output.open('x') as output:
        json.dump(value, output, ensure_ascii=False, indent=2, allow_nan=False)
        output.write('\n')


if __name__ == '__main__':
    main()
