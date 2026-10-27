#!/usr/bin/env python3
"""Analyze explicit GTA benchmark batches without inventing device evidence.

Repeat --trial MANIFEST.json for OFF/ON pairs. All paths are relative to their
manifest; existing prepare/collect outputs are inputs and are never modified.
Built-in FPS reports, guest boundaries, native source-call cadence and exclusive
host-thread interval phases remain separate measurement domains.
"""
import argparse
import csv
import hashlib
import io
import json
import math
from pathlib import Path
import re
import statistics

from collect_native_trial import summarize, AppFiles, validate_native_size

FEATURES = ('pipeline_cache', 'occlusion_pool', 'native_render_fastpath', 'io_handle_lru',
            'inline_upload_borrow', 'native_command_fastpath', 'skip_empty_init_transition',
            'skip_empty_render_prepass', 'pso_warmup')
NATIVE_BOOLEAN_FLAGS = ('shared_memory_fastpath', 'cached_readback', 'occlusion_pool', 'subresource_proxies',
                        'pipeline_cache', 'native_render_fastpath', 'io_handle_lru', 'pipeline_cache_read_only',
                        'inline_upload_borrow', 'native_command_fastpath', 'skip_empty_init_transition',
                        'skip_empty_render_prepass', 'pso_warmup', 'pso_warmup_read_only')
BAD_CONSTANT = lambda value: (_ for _ in ()).throw(ValueError('non-finite JSON: ' + value))


def validate_fixed_native_features(value, feature):
    if value is None:
        return {}
    if not isinstance(value, dict):
        raise ValueError('fixed native features must be a name/boolean object')
    for key, enabled in value.items():
        if key not in NATIVE_BOOLEAN_FLAGS:
            raise ValueError('fixed feature must be a native boolean flag: ' + str(key))
        if key == feature:
            raise ValueError('fixed feature conflicts with independently tested feature: ' + key)
        if not isinstance(enabled, bool):
            raise ValueError('fixed native feature values must be true/false booleans: ' + key)
    return dict(value)


def number(value):
    return isinstance(value, (int, float)) and not isinstance(value, bool) and math.isfinite(value)


def integer(value):
    return isinstance(value, int) and not isinstance(value, bool)


def read_json(path):
    return json.loads(path.read_text(), parse_constant=BAD_CONSTANT)


def provenance(path):
    raw = path.read_bytes()
    return {'path': str(path.resolve()), 'bytes': len(raw), 'sha256': hashlib.sha256(raw).hexdigest()}


def distribution(values):
    values = sorted(values)
    return {'samples': len(values), 'min': values[0] if values else None,
            **{f'p{p}': values[max(0, math.ceil(len(values) * p / 100) - 1)] if values else None
               for p in (50, 95, 99)}, 'max': values[-1] if values else None}


def duration_metrics(values):
    """Count/sum FPS and explicitly defined tails on one duration sequence."""
    total = math.fsum(values)
    tail_count = max(1, math.ceil(len(values) * .01)) if values else 0
    tail_mean = statistics.fmean(sorted(values, reverse=True)[:tail_count]) if values else None
    tail_point_one_count = max(1, math.ceil(len(values) * .001)) if values else 0
    tail_point_one_mean = statistics.fmean(sorted(values, reverse=True)[:tail_point_one_count]) if values else None
    slow_count = run_count = longest_count = current_count = 0
    longest_ms = current_ms = 0.0
    for value in values:
        if value > 50.0:
            slow_count += 1
            run_count += int(current_count == 0)
            current_count += 1
            current_ms += value
            longest_count = max(longest_count, current_count)
            longest_ms = max(longest_ms, current_ms)
        else:
            current_count, current_ms = 0, 0.0
    return {'throughputFps': 1000 * len(values) / total if total else None,
            'onePercentLowFps': 1000 / tail_mean if tail_mean else None,
            'onePercentLowDefinition': '1000 / arithmetic mean of the slowest ceil(N*0.01) positive frame/interval durations in ms; not 1000/p99',
            'onePercentLowSamples': tail_count,
            'pointOnePercentLowFps': 1000 / tail_point_one_mean if tail_point_one_mean else None,
            'pointOnePercentLowSamples': tail_point_one_count,
            'pointOnePercentLowDefinition': '1000 / arithmetic mean of the slowest ceil(N*0.001) positive durations in ms; small tail sample counts are not long-run statistical assurance',
            'maximumFrameTimeMs': max(values) if values else None,
            'slowFrame50ms': {'thresholdMs': 50.0, 'comparison': 'strictly greater',
                              'frames': slow_count, 'framePercent': 100 * slow_count / len(values) if values else None,
                              'framesPerMinute': 60000 * slow_count / total if total else None,
                              'consecutiveRuns': run_count, 'runsPerMinute': 60000 * run_count / total if total else None,
                              'longestConsecutiveFrames': longest_count, 'longestConsecutiveDurationMs': longest_ms,
                              'rateDenominator': 'sum of retained durations, not wall time outside this sequence',
                              'scope': 'slow frames and contiguous runs of slow frames; neither establishes a perceptual stutter event'}}


def resolve(manifest, key, root):
    value = manifest.get(key)
    return (root / value).resolve() if isinstance(value, str) and value else None


def parse_builtin(path, expected_passes=None):
    """Recognize labeled FPS tables only; bare numbers never imply a benchmark."""
    if path is None or not path.is_file():
        return {'recognized': False, 'completePassTable': False, 'reason': 'no_result_artifact'}
    raw = path.read_text(encoding='utf-8-sig')
    records, format_name, frame_counts, milliseconds, percentiles = [], None, {}, {}, {}
    if path.suffix.lower() == '.json':
        value = read_json(path)
        if isinstance(value, dict) and value.get('kind') == 'gta_v_builtin' and isinstance(value.get('passes'), list):
            records = value['passes']
            format_name = 'explicit_gta_v_builtin_json'
    else:
        rows = list(csv.reader(io.StringIO(raw)))
        if rows:
            header = [re.sub(r'[^a-z]', '', cell.lower()) for cell in rows[0]]
            needed = ('pass', 'minfps', 'maxfps', 'avgfps')
            if all(key in header for key in needed):
                format_name = 'labeled_fps_csv'
                for row in rows[1:]:
                    if not row or all(not cell.strip() for cell in row):
                        continue
                    if len(row) != len(header):
                        raise ValueError('benchmark CSV column count changed')
                    record = {'pass': row[header.index('pass')]}
                    for key, output in [('minfps', 'minFps'), ('maxfps', 'maxFps'), ('avgfps', 'avgFps')]:
                        record[output] = float(row[header.index(key)])
                    records.append(record)
        if format_name is None:
            # Keep the declared Min,Max,Avg order. Other unlabeled numeric text
            # and unrelated RIFE/NPU benchmark reports remain unsupported.
            section = False
            for line in raw.splitlines():
                if re.search(r'Frames\s+Per\s+Second.*Min\s*,\s*Max\s*,\s*Avg', line, re.I):
                    section, format_name = True, 'gta_labeled_fps_text'
                    continue
                match = re.fullmatch(r'\s*Pass\s+(\d+)\s*,\s*([\d.eE+-]+)\s*,\s*([\d.eE+-]+)\s*,\s*([\d.eE+-]+)\s*', line)
                if section and match:
                    index, low, high, mean = match.groups()
                    records.append({'pass': index, 'minFps': float(low), 'maxFps': float(high), 'avgFps': float(mean)})
                elif section and line.strip() and not match and records:
                    section = False
            # These are separate engine summaries, not substitutes for the
            # actual frame-time distribution. Keep their reported definitions.
            time_section, percentile_pass = False, None
            for line in raw.splitlines():
                if re.search(r'^Time in milliseconds.*Min\s*,\s*Max\s*,\s*Avg', line, re.I):
                    time_section = True
                    continue
                match = re.fullmatch(r'\s*Pass\s+(\d+)\s*,\s*([\d.eE+-]+)\s*,\s*([\d.eE+-]+)\s*,\s*([\d.eE+-]+)\s*', line)
                if time_section and match:
                    index, low, high, mean = match.groups()
                    milliseconds[index] = dict(zip(('min', 'max', 'avg'), map(float, (low, high, mean))))
                    continue
                if time_section and line.strip():
                    time_section = False
                count = re.fullmatch(r'\s*Pass\s+(\d+):\s*\d+/(\d+)\s+frames\s*\([\d.]+%\)\s*', line)
                if count:
                    index, total = count.groups()
                    frame_counts.setdefault(index, set()).add(int(total))
                heading = re.fullmatch(r'\s*Percentiles in ms for pass\s+(\d+)\s*', line, re.I)
                if heading:
                    percentile_pass = heading[1]
                    continue
                value = re.fullmatch(r'\s*(\d+)%,\s*([\d.eE+-]+)\s*', line)
                if percentile_pass is not None and value:
                    percentiles.setdefault(percentile_pass, {})['p' + value[1]] = float(value[2])
                elif line.strip():
                    percentile_pass = None
    normalized, seen = [], set()
    for row in records:
        if not isinstance(row, dict) or not isinstance(row.get('pass'), (str, int)) or isinstance(row.get('pass'), bool):
            raise ValueError('invalid built-in pass identifier')
        identity = str(row['pass'])
        if identity in seen:
            raise ValueError('duplicate benchmark pass: ' + identity)
        seen.add(identity)
        values = [row.get(key) for key in ('minFps', 'avgFps', 'maxFps')]
        if not all(number(value) for value in values) or not 0 <= values[0] <= values[1] <= values[2] or values[2] <= 0:
            raise ValueError('invalid min/average/max FPS ordering')
        counts = frame_counts.get(identity, set())
        if len(counts) > 1:
            raise ValueError('conflicting reported frame counts')
        count = next(iter(counts), row.get('reportedFrames'))
        if count is not None and (not integer(count) or count <= 0):
            raise ValueError('invalid reported frame count')
        normalized.append({'pass': identity, **{key: row[key] for key in ('minFps', 'avgFps', 'maxFps')},
                           'reportedFrames': count, 'reportedFrameTimeMs': milliseconds.get(identity),
                           'reportedPercentileMs': percentiles.get(identity, {})})
    complete = bool(normalized) and isinstance(expected_passes, list) and {str(p) for p in expected_passes} == seen
    return {'recognized': format_name is not None and bool(normalized), 'format': format_name,
            'completePassTable': complete, 'passes': normalized, 'provenance': provenance(path),
            'scope': 'FPS reported by the built-in artifact only; no native-monotonic linkage, source/AI identity, or successful completion is inferred from this table'}


def parse_frame_times(path):
    """Parse the observed GTA Frame/Time(ms) table without assuming a clock."""
    if path is None or not path.is_file():
        return {'recognized': False, 'completeFrameSequence': False, 'reason': 'no_frame_times_artifact'}
    rows = list(csv.reader(io.StringIO(path.read_text(encoding='utf-8-sig')), delimiter='\t'))
    if not rows or [cell.strip() for cell in rows[0]] != ['Frame', 'Time(ms)']:
        return {'recognized': False, 'completeFrameSequence': False,
                'reason': 'unsupported_frame_times_header', 'provenance': provenance(path)}
    indices, values = [], []
    for row in rows[1:]:
        if not row or all(not cell.strip() for cell in row):
            continue
        if len(row) != 2 or not re.fullmatch(r'\d+', row[0].strip()):
            raise ValueError('invalid raw frame index or column count')
        value = float(row[1])
        if not number(value) or value <= 0:
            raise ValueError('raw frame time must be positive and finite')
        indices.append(int(row[0]))
        values.append(value)
    complete = bool(values) and indices == list(range(len(values)))
    total = math.fsum(values)
    return {'recognized': True, 'provenance': provenance(path), 'frames': len(values),
            'completeFrameSequence': complete, 'firstFrame': indices[0] if indices else None,
            'lastFrame': indices[-1] if indices else None, 'totalFrameTimeMs': total,
            'meanFrameTimeMs': total / len(values) if values else None,
            'throughputFps': 1000 * len(values) / total if total else None,
            'meanInstantaneousFps': statistics.fmean(1000 / value for value in values) if values else None,
            **duration_metrics(values),
            'frameTimeMs': distribution(values),
            'quantileDefinition': 'nearest rank on all retained raw Time(ms) values; never copied from the engine percentile table',
            'scope': 'engine-recorded frame durations and frame indices; count/sum throughput differs from arithmetic mean instantaneous FPS; this file alone has no PID, native monotonic timestamps, source/AI identity or scanout timing'}


def analyze_frame_times(entries, builtin, binding, root, report_binding_qualified):
    if entries is None:
        entries = []
    if not isinstance(entries, list):
        raise ValueError('benchmark.frameTimes must be a list of pass/path objects')
    reports = {row['pass']: row for row in builtin.get('passes', [])}
    passes, issues = {}, []
    hashes = binding.get('frameTimesSha256', {})
    if not isinstance(hashes, dict):
        raise ValueError('binding.frameTimesSha256 must be a pass/hash object')
    for entry in entries:
        if not isinstance(entry, dict) or not isinstance(entry.get('pass'), (str, int)) or isinstance(entry.get('pass'), bool):
            raise ValueError('invalid frameTimes pass identifier')
        identity = str(entry['pass'])
        if identity in passes:
            raise ValueError('duplicate frameTimes pass identifier')
        row = parse_frame_times(resolve(entry, 'path', root))
        report = reports.get(identity, {})
        count_matches = bool(row.get('frames')) and row.get('frames') == report.get('reportedFrames')
        bound = bool(row.get('provenance')) and hashes.get(identity) == row['provenance']['sha256']
        qualified = row.get('completeFrameSequence') is True and count_matches and bound and bool(report) and report_binding_qualified
        if not qualified:
            issues.append('raw_frame_times_unqualified:' + identity)
        avg = report.get('avgFps')
        engine_ms = report.get('reportedFrameTimeMs') or {}
        engine_percentiles = report.get('reportedPercentileMs') or {}
        raw_ms = row.get('frameTimeMs', {})
        passes[identity] = {**row, 'pass': identity, 'qualifiedArtifactAndCount': qualified,
                            'boundByHash': bound, 'reportedFrameCountMatches': count_matches,
                            'reportedAvgFps': avg,
                            'reportedAvgMinusThroughputFps': avg - row['throughputFps'] if number(avg) and number(row.get('throughputFps')) else None,
                            'reportedAvgMinusMeanInstantaneousFps': avg - row['meanInstantaneousFps'] if number(avg) and number(row.get('meanInstantaneousFps')) else None,
                            'reportedAvgTimeMinusRawMeanMs': engine_ms['avg'] - row['meanFrameTimeMs'] if number(engine_ms.get('avg')) and number(row.get('meanFrameTimeMs')) else None,
                            'reportedMinusRawPercentileMs': {key: value - raw_ms[key] for key, value in engine_percentiles.items()
                                                            if number(value) and number(raw_ms.get(key))},
                            'interpretation': 'Numerical agreement with mean instantaneous FPS can explain the reported AVG in this sample; AVG is not relabeled as count/sum throughput. Rounded raw milliseconds limit numerical precision.'}
    if passes.keys() != reports.keys() or not passes:
        issues.append('raw_frame_time_pass_set_missing_or_changed')
    return {'passes': [passes[key] for key in sorted(passes)], 'qualificationIssues': sorted(set(issues)),
            'qualifiedAllPasses': not issues,
            'primaryComparison': 'qualified raw count/sum throughput and nearest-rank Time(ms) tails; reported engine averages remain separate'}


def parse_jsonl(path):
    if path is None or not path.is_file():
        return []
    rows = []
    for index, line in enumerate(path.read_text().splitlines(), 1):
        if not line.strip():
            continue
        row = json.loads(line, parse_constant=BAD_CONSTANT)
        if not isinstance(row, dict):
            raise ValueError(f'JSONL row {index} must be an object')
        rows.append(row)
    return rows


def analyze_intervals(path, window, target_fps=20, goal_fps=None):
    if any(value is not None and (not number(value) or value <= 0) for value in (target_fps, goal_fps)):
        raise ValueError('source target and performance goal must be positive finite FPS or null')
    rows = parse_jsonl(path)
    if not rows:
        return {'available': False, 'reason': 'no_interval_artifact', 'chains': {}}
    headers = [row for row in rows if row.get('type') == 'header']
    if len(headers) != 1 or headers[0].get('schemaVersion') != 1 or headers[0].get('clock') != 'CLOCK_MONOTONIC_ns':
        raise ValueError('interval stream requires one explicit CLOCK_MONOTONIC_ns header')
    header = headers[0]
    if not valid_window(window):
        return {'available': True, 'qualified': False, 'reason': 'no_explicit_native_monotonic_window',
                'provenance': provenance(path), 'chains': {}}
    pid, start, end = window['pid'], window['startMonotonicNs'], window['endMonotonicNs']
    if header.get('pid') != pid:
        raise ValueError('interval header PID disagrees with benchmark window')
    frames = [row for row in rows if row.get('type') == 'frame_interval']
    footers = [row for row in rows if row.get('type') == 'footer']
    if len(footers) > 1 or any(row.get('pid') != pid for row in footers):
        raise ValueError('conflicting interval stream footers')
    if any(row.get('pid', header.get('pid')) != pid for row in frames):
        raise ValueError('mixed/wrong PID in interval stream')
    indices = [row.get('recordIndex') for row in frames]
    if any(not integer(value) or value <= 0 for value in indices) or any(b <= a for a, b in zip(indices, indices[1:])):
        raise ValueError('shared recordIndex must be unique and increasing; gaps may be reported drops')
    chains = {}
    for kind, sequence in [('guest', 'Guest'), ('source_present_call', 'Source')]:
        all_rows = [row for row in frames if row.get('boundaryKind') == kind]
        selected = []
        for row in all_rows:
            begin, finish = row.get('beginMonotonicNs'), row.get('endMonotonicNs')
            if not integer(begin) or not integer(finish) or not integer(row.get('durationNs')) or finish <= begin or row['durationNs'] != finish - begin:
                raise ValueError('invalid interval duration/clock')
            if start <= begin and finish <= end:
                selected.append(row)
        issues = []
        if not selected:
            issues.append('no_complete_intervals_inside_window')
        phases = {}
        for row in selected:
            first_sequence, last_sequence = row.get('begin' + sequence + 'Sequence'), row.get('end' + sequence + 'Sequence')
            if not integer(first_sequence) or not integer(last_sequence) or last_sequence <= first_sequence:
                issues.append('invalid_boundary_sequence')
            values = row.get('phaseWallNs', {})
            if not isinstance(values, dict) or any(not integer(value) or value < 0 for value in values.values()):
                raise ValueError('invalid exclusive phase values')
            accounted = row.get('accountedWallNs')
            if not integer(accounted) or sum(values.values()) != accounted or accounted > row['durationNs']:
                issues.append('phase_accounting_mismatch')
            if row.get('coverageComplete') is True and accounted != row['durationNs']:
                issues.append('unaccounted_exclusive_wall_time')
            if row.get('coverageComplete') is not True:
                issues.append('producer_reports_incomplete_coverage')
            if isinstance(header.get('phaseNames'), list) and set(values) != set(header['phaseNames']):
                issues.append('phase_names_disagree_with_header')
            for name, value in values.items():
                phases[name] = phases.get(name, 0) + value
        for a, b in zip(selected, selected[1:]):
            if (a['endMonotonicNs'] != b['beginMonotonicNs']
                    or a.get('end' + sequence + 'Sequence') != b.get('begin' + sequence + 'Sequence')):
                issues.append('missing_or_overlapping_interval')
            if (a.get('presentationGeneration'), a.get('cacheEpoch')) != (b.get('presentationGeneration'), b.get('cacheEpoch')):
                issues.append('presentation_context_changed')
        counters = {}
        if selected:
            for name in ('writerQueueDrops', 'clockFailures', 'stackErrors'):
                values = [row.get('countersAtEnqueue', row.get('counters', {})).get(name) for row in selected]
                if any(not integer(value) or value < 0 for value in values):
                    issues.append('missing_writer_counter:' + name)
                    continue
                if any(b < a for a, b in zip(values, values[1:])):
                    issues.append('writer_counter_rewound:' + name)
                counters[name] = values[-1] - values[0]
                if counters[name]:
                    issues.append('writer_error_in_window:' + name)
        qualified = not issues
        duration = sum(row['durationNs'] for row in selected)
        statistics_ms = distribution([row['durationNs'] / 1e6 for row in selected]) if qualified else distribution([])
        chains[kind] = {'intervals': len(selected), 'qualifiedInterior': qualified,
                        'issues': sorted(set(issues)), 'intervalMs': statistics_ms,
                        'coveredDurationNs': duration, 'windowDurationNs': end - start,
                        'windowFullyCovered': qualified and bool(selected) and selected[0]['beginMonotonicNs'] == start and selected[-1]['endMonotonicNs'] == end,
                        'firstBeginMonotonicNs': selected[0]['beginMonotonicNs'] if selected else None,
                        'lastEndMonotonicNs': selected[-1]['endMonotonicNs'] if selected else None,
                        'exclusivePhaseWallNs': phases, 'counterDeltas': counters,
                        'sourceTargetFps': target_fps, 'performanceGoalFps': goal_fps,
                        'durationMetrics': duration_metrics([row['durationNs'] / 1e6 for row in selected]) if qualified else None,
                        'overSourceBudgetIntervals': sum(row['durationNs'] > 1e9 / target_fps for row in selected) if qualified and target_fps is not None else None,
                        'overGoalBudgetIntervals': sum(row['durationNs'] > 1e9 / goal_fps for row in selected) if qualified and goal_fps is not None else None,
                        'scope': 'complete retained interior intervals only; exclusive host-thread wall phases, not GPU execution or a causal assignment to source frame N'}
    footer = footers[0] if footers else None
    counts = footer.get('counters', {}) if footer else {}
    session_complete = (bool(footer) and footer.get('producerClosed') is True
                        and counts.get('intervalsAttempted') == counts.get('intervalsEnqueued')
                        == counts.get('intervalsWritten') == counts.get('intervalsFlushed') == len(frames)
                        and counts.get('recordLimitReached') is False
                        and all(counts.get(key) == 0 for key in ('writerQueueDrops', 'clockFailures', 'stackErrors', 'writerErrors')))
    return {'available': True, 'provenance': provenance(path), 'header': header, 'chains': chains,
            'unknownBoundaryKinds': sorted({str(row.get('boundaryKind')) for row in frames
                                            if row.get('boundaryKind') not in ('guest', 'source_present_call')}),
            'producerClosed': footer.get('producerClosed') if footer else None,
            'sessionCoverageComplete': session_complete, 'footerCounters': counts if footer else None,
            'observerIntensity': {key: counts.get(key) for key in ('stageEntries', 'clockReads', 'boundaryCalls')},
            'observerTimeMeasured': False,
            'persistenceScope': 'flushed means successful BufWriter flush, not fsync; absent footer cannot prove a clean full-session drain',
            'scope': 'guest and source_present_call chains describe overlapping wall time and MUST NOT be added'}


def valid_window(window):
    return (isinstance(window, dict) and window.get('clock') == 'CLOCK_MONOTONIC_ns'
            and integer(window.get('pid')) and window['pid'] > 0
            and integer(window.get('startMonotonicNs')) and integer(window.get('endMonotonicNs'))
            and 0 <= window['startMonotonicNs'] < window['endMonotonicNs'])


def profile_log(path):
    if path is None or not path.is_file():
        return {'available': False, 'scope': 'no_cpu_profile_log'}
    windows, pools, pipeline, queue_depths, invalid = [], [], [], set(), 0
    for line in path.read_text(errors='replace').splitlines():
        queue = re.search(r'\[config\] effective -FrameQueueLimit=(\d+);', line)
        if queue:
            queue_depths.add(int(queue[1]))
        for marker, target in [('[cpu-profile] ', windows), ('occlusion-pool: ', pools), ('pipeline-cache: ', pipeline)]:
            if marker in line:
                try:
                    value = json.loads(line.split(marker, 1)[1], parse_constant=BAD_CONSTANT)
                    if not isinstance(value, dict):
                        raise ValueError('not an object')
                    target.append(value)
                except (ValueError, json.JSONDecodeError):
                    invalid += 1
    valid = [row for row in windows if number(row.get('windowMs')) and row['windowMs'] > 0]
    overhead = [row.get('wallSpans', {}).get('bindingProbeOverheadSample', {}).get('totalMs') for row in valid]
    cpu = [row['processCpuMs'] for row in valid if number(row.get('processCpuMs'))]
    frames = [row['frames'] for row in valid if integer(row.get('frames'))]
    return {'available': True, 'provenance': provenance(path), 'cpuWindows': len(windows), 'invalidRecords': invalid,
            'fileScopeCpuMs': sum(cpu) if cpu else None,
            'fileScopeReportedFrames': sum(frames) if frames else None,
            'bindingProbeOverheadSampleMs': sum(value for value in overhead if number(value)) if any(number(value) for value in overhead) else None,
            'sampledOverheadAvailable': any(number(value) for value in overhead),
            'totalInstrumentationOverheadKnown': False,
            'effectiveQueueDepthValues': sorted(queue_depths), 'occlusionSnapshots': pools, 'pipelineCacheSnapshots': pipeline,
            'windowAssociation': 'unavailable: legacy CPU/pool log records have no absolute monotonic window and PID; file totals are not benchmark-window attribution',
            'scope': 'nested wallSpans are not added; every64th/conditional samples are not scaled into measured totals; metadata reuse is not saved bytes'}


def thermal(path, window):
    rows = parse_jsonl(path)
    if not rows or not valid_window(window):
        return {'available': False, 'controlled': None, 'reason': 'no_window_bound_thermal_data'}
    if any(isinstance(row.get('conditions'), dict) for row in rows):
        associated, accepted, seen, rejected, failures = [], [], set(), 0, []
        for row in rows:
            if row.get('pid') != window['pid'] or not integer(row.get('monotonicNs')) or not window['startMonotonicNs'] <= row['monotonicNs'] <= window['endMonotonicNs']:
                continue
            snapshot = row.get('conditions')
            if not isinstance(snapshot, dict):
                continue
            associated.append(snapshot)
            sample = snapshot.get('sample')
            fresh = (snapshot.get('enabled') is True and snapshot.get('active') is True
                     and snapshot.get('fresh') is True
                     and integer(snapshot.get('generation')) and snapshot.get('sampleGeneration') == snapshot['generation']
                     and number(snapshot.get('sampleAgeMs')) and 0 <= snapshot['sampleAgeMs'] <= 15000)
            if not fresh or not isinstance(sample, dict) or not number(sample.get('uptimeMs')) or not number(sample.get('completedUptimeMs')) or sample['completedUptimeMs'] < sample['uptimeMs']:
                rejected += 1
                continue
            identity = (snapshot['generation'], snapshot['sampleGeneration'], sample['uptimeMs'], sample['completedUptimeMs'])
            if identity in seen:
                continue
            seen.add(identity)
            accepted.append(sample)
            failures.extend(str(value) for value in sample.get('apiFailures', []))
        battery = [sample['batteryTemperatureC'] for sample in accepted if number(sample.get('batteryTemperatureC'))]
        levels = [sample['thermalLevel'] for sample in accepted if integer(sample.get('thermalLevel')) and 0 <= sample['thermalLevel'] <= 7]
        return {'available': bool(battery or levels), 'controlled': None,
                'provenance': provenance(path), 'associatedStatusSnapshots': len(associated),
                'uniqueFreshSensorSamples': len(accepted), 'rejectedStaleOrInvalidSnapshots': rejected,
                'batteryTemperatureC': distribution(battery),
                'thermalLevelCounts': {str(level): levels.count(level) for level in sorted(set(levels))},
                'batterySOC': distribution([sample['batterySOC'] for sample in accepted if number(sample.get('batterySOC'))]),
                'pluggedTypes': sorted({sample['pluggedType'] for sample in accepted if integer(sample.get('pluggedType'))}),
                'chargingStatuses': sorted({sample['chargingStatus'] for sample in accepted if integer(sample.get('chargingStatus'))}),
                'apiFailures': sorted(set(failures)),
                'sensorClock': 'HarmonyOS systemDateTime STARTUP uptimeMs; not assumed identical to CLOCK_MONOTONIC_ns',
                'nativeWindowAssociation': 'snapshot associated with an in-window native-status clock; sensor collection uptime is retained separately and has no proven exact native-clock mapping',
                'sensorCollectionUptimeMs': [{'begin': sample['uptimeMs'], 'end': sample['completedUptimeMs']} for sample in accepted],
                'coreTemperatureKnown': False,
                'scope': 'battery temperature only; thermalLevel is an ordinal device severity category, not CPU/GPU core temperature or a measured frequency/throttling duration; duplicate five-second sensor results are not independent samples'}
    observed = [row for row in rows if row.get('clock') == 'CLOCK_MONOTONIC_ns'
                and integer(row.get('monotonicNs')) and window['startMonotonicNs'] <= row['monotonicNs'] <= window['endMonotonicNs']
                and number(row.get('temperatureC'))]
    return {'available': bool(observed), 'controlled': None, 'samples': len(observed),
            'temperatureC': distribution([row['temperatureC'] for row in observed]), 'provenance': provenance(path),
            'scope': 'reported sensor samples only; matching thermal/throttling conditions require the sensor identity, coverage and test protocol'}


def analyze_trial(path):
    manifest = read_json(path)
    if (manifest.get('schemaVersion') != 1 or manifest.get('feature') not in FEATURES
            or not isinstance(manifest.get('enabled'), bool) or not manifest.get('trialId') or not manifest.get('pairId')):
        raise ValueError('manifest requires schemaVersion=1, trialId, pairId, feature and boolean enabled')
    root = path.parent
    directory = resolve(manifest, 'captureDirectory', root)
    if directory is None:
        raise ValueError('captureDirectory required')
    config = read_json(directory / 'config-before.json')
    after = read_json(directory / 'config-after.json')
    samples = read_json(directory / 'samples.json')
    if len(samples) < 2:
        raise ValueError('at least two collector samples required')
    for row in samples:
        if not all(integer(row.get(key)) for key in ('pid', 'monotonicNs', 'guestPresents', 'presents')):
            raise ValueError('collector identity/clock/counters must be integers')
    summary = summarize(samples)
    collected_summary = read_json(directory / 'summary.json') if (directory / 'summary.json').is_file() else {}
    summary['device'] = collected_summary.get('device')
    summary['bundle'] = collected_summary.get('bundle')
    feature = manifest['feature']
    conditions = manifest.get('conditions', {})
    baseline_mode = conditions.get('baselineMode', 'legacy20')
    unlimited = baseline_mode == 'unlimited-native-800p'
    goal = conditions.get('performanceGoal') if unlimited else None
    issues = []
    try:
        if 'fixedNativeFeatures' in conditions and not isinstance(conditions['fixedNativeFeatures'], dict):
            raise ValueError('declared fixed native features must be an object')
        fixed_flags = validate_fixed_native_features(conditions.get('fixedNativeFeatures'), feature)
    except ValueError:
        fixed_flags = {}
        issues.append('invalid_fixed_native_features')
    for key, enabled in fixed_flags.items():
        if config.get(key) is not enabled:
            issues.append('fixed_native_feature_disagrees_with_config:' + key)
    instrumentation = conditions.get('instrumentation', {})
    if isinstance(instrumentation, dict) and 'cpuProfile' in instrumentation:
        declared_cpu_profile = instrumentation['cpuProfile']
        if not isinstance(declared_cpu_profile, bool) or config.get('cpu_profile', False) is not declared_cpu_profile:
            issues.append('cpu_profile_instrumentation_disagrees_with_config')
    # Missing legacy fields mean OFF. New manifests explicitly record both
    # values; a true/invalid config cannot silently omit the diagnostic label.
    declared_prepass = instrumentation.get('renderPrepassDiagnostics', False) if isinstance(instrumentation, dict) else False
    actual_prepass = config.get('render_prepass_diagnostics', False)
    if (not isinstance(declared_prepass, bool) or not isinstance(actual_prepass, bool)
            or actual_prepass is not declared_prepass):
        issues.append('render_prepass_diagnostics_instrumentation_disagrees_with_config')
    if baseline_mode not in ('legacy20', 'unlimited-native-800p'):
        issues.append('unsupported_baseline_mode')
    if unlimited and goal != {'averageFps': 30, 'onePercentLowFps': 20}:
        issues.append('native30_performance_goal_unconfirmed')
    if unlimited and (not integer(config.get('fps_limit')) or config['fps_limit'] != 0
                      or config.get('spatial_upscale_native_short_edge') != 800):
        issues.append('unlimited_native800_configuration_unconfirmed')
    if unlimited and any(isinstance(value, str) and value.lower().split('=', 1)[0] in
                         ('-framelimit', '-fps', '-fpslimit') for value in config.get('extra_args', [])):
        issues.append('unlimited_hidden_fps_limit_present')
    if (directory / 'error.json').exists():
        issues.append('collector_error_artifact_present')
    if config != after:
        issues.append('config_changed_within_trial')
    if not summary['device'] or not summary['bundle']:
        issues.append('device_or_bundle_identity_missing')
    if config.get(feature, False) is not manifest['enabled']:
        issues.append('manifest_feature_disagrees_with_config')
    expected_size = [1200, 800]
    if 'nativeRenderSize' in conditions:
        try:
            expected_size = validate_native_size(conditions['nativeRenderSize'])
            if not unlimited and expected_size != [1200, 800]:
                raise ValueError('legacy size changed')
        except ValueError:
            issues.append('invalid_declared_native_render_size')
    if 'fileTransport' in conditions:
        try:
            declared = conditions['fileTransport']
            if not isinstance(declared, dict) or set(declared) != {'mode', 'filesRoot'}:
                raise ValueError('invalid transport declaration')
            transport = AppFiles(summary.get('bundle'), declared['mode'],
                                 declared['filesRoot'] if declared['mode']=='direct' else None)
            if transport.conditions() != declared or collected_summary.get('fileTransport') != declared:
                raise ValueError('transport differs from collector')
        except (ValueError, TypeError):
            issues.append('file_transport_declaration_disagrees_with_collector')
    elif 'fileTransport' in collected_summary:
        issues.append('file_transport_declaration_missing')
    if config.get('graphics_preset') != 'medium' or summary['renderSize'] != expected_size:
        issues.append('medium_1200x800_baseline_changed' if 'nativeRenderSize' not in conditions else 'medium_declared_native_size_changed')
    if config.get('spatial_upscale_enabled') is not True or config.get('frame_interpolation_enabled') is not True:
        issues.append('SR_FG_baseline_changed')
    if config.get('frame_interpolation_output_fps') != 40:
        issues.append('output40_configuration_unconfirmed' if unlimited else 'source20_output40_configuration_unconfirmed')
    observed_targets = sorted({row.get('nativeTelemetry', {}).get('sourceTargetFps') for row in samples
                               if number(row.get('nativeTelemetry', {}).get('sourceTargetFps'))})
    null_targets = sum('sourceTargetFps' in row.get('nativeTelemetry', {}) and
                       row['nativeTelemetry']['sourceTargetFps'] is None for row in samples)
    missing_targets = sum('sourceTargetFps' not in row.get('nativeTelemetry', {}) for row in samples)
    if unlimited and null_targets != len(samples):
        issues.append('unlimited_runtime_target_not_explicit_null')
    elif not unlimited and observed_targets != [20]:
        issues.append('source20_runtime_target_unconfirmed')
    benchmark = manifest.get('benchmark', {})
    window = benchmark.get('window')
    if benchmark.get('kind') != 'gta_v_builtin' or not benchmark.get('sceneId'):
        issues.append('builtin_benchmark_identity_unconfirmed')
    if not valid_window(window):
        issues.append('benchmark_native_monotonic_window_missing')
    elif window['pid'] != summary['pid']:
        issues.append('benchmark_window_wrong_pid')
    window_scope = benchmark.get('windowScope')
    if window_scope not in ('benchmark_phase_events', 'enclosing_native_monotonic_bracket'):
        issues.append('benchmark_window_scope_unconfirmed')
    result_path = resolve(benchmark, 'resultPath', root)
    builtin = parse_builtin(result_path, benchmark.get('expectedPasses'))
    if not builtin.get('recognized') or not builtin.get('completePassTable'):
        issues.append('builtin_result_missing_or_incomplete')
    binding_path = resolve(benchmark, 'bindingPath', root)
    binding = read_json(binding_path) if binding_path and binding_path.is_file() else {}
    result_hash = builtin.get('provenance', {}).get('sha256')
    binding_ok = (builtin.get('recognized') is True and isinstance(result_hash, str)
                  and binding.get('resultSha256') == result_hash
                  and binding.get('window') == window and valid_window(window)
                  and binding.get('completed') is True and bool(binding.get('completionEvidence')))
    if not binding_ok:
        issues.append('builtin_artifact_not_bound_to_current_run')
    raw_times = analyze_frame_times(benchmark.get('frameTimes'), builtin, binding, root, binding_ok)
    issues.extend(raw_times['qualificationIssues'])
    log = profile_log(resolve(manifest, 'runtimeLogPath', root))
    if log.get('effectiveQueueDepthValues') != [3]:
        issues.append('effective_queue3_unconfirmed')
    actual_sizes = set()
    for status_path in sorted(directory.glob('fg-*.json')):
        status = read_json(status_path)
        if status.get('pid') != summary['pid']:
            issues.append('FG_snapshot_wrong_pid')
        source = status.get('guestFinalSource') or {}
        if integer(source.get('width')) and integer(source.get('height')):
            actual_sizes.add((source['width'], source['height']))
    if actual_sizes != {tuple(expected_size)}:
        issues.append('actual_source_texture_1200x800_unconfirmed' if 'nativeRenderSize' not in conditions else 'actual_source_texture_declared_native_size_unconfirmed')
    intervals = analyze_intervals(resolve(manifest, 'frameIntervalsPath', root) or resolve(manifest, 'sourceFramesPath', root),
                                  window, target_fps=None if unlimited else 20, goal_fps=30 if unlimited else None)
    heat = thermal(resolve(manifest, 'thermalPath', root), window)
    for key in ('packageSha256', 'gameSettingsSha256'):
        if not isinstance(conditions.get(key), str) or not re.fullmatch(r'[0-9a-fA-F]{64}', conditions[key]):
            issues.append(key + '_missing')
    if (conditions.get('queueDepth') != 3 or
            (unlimited and ('sourceTargetFps' not in conditions or conditions['sourceTargetFps'] is not None)) or
            (not unlimited and conditions.get('sourceTargetFps') != 20)):
        issues.append('declared_queue_or_source_target_changed')
    for key in ('sceneWarmupPasses', 'workloadCacheState'):
        if key not in conditions:
            issues.append(key + '_missing')
    goal_assessment = None
    if unlimited:
        goal_assessment = {'averageFps': 30, 'onePercentLowFps': 20,
                           'rawPasses': [{'pass': row['pass'], 'throughputFps': row.get('throughputFps'),
                                          'onePercentLowFps': row.get('onePercentLowFps'),
                                          'meetsRecordedDurationGoal': (row['throughputFps'] >= 30 and row['onePercentLowFps'] >= 20)
                                          if row.get('qualifiedArtifactAndCount') else None}
                                         for row in raw_times['passes']],
                           'nativeSourceGoalEstablished': False,
                           'scope': 'raw engine duration arithmetic only; source-vs-generated-frame semantics and exact benchmark native phase linkage remain required for native-source goal acceptance'}
    return {'trialId': manifest['trialId'], 'pairId': manifest['pairId'], 'feature': feature,
            'enabled': manifest['enabled'], 'order': manifest.get('order'), 'conditions': conditions,
            'benchmarkSceneId': benchmark.get('sceneId'), 'benchmark': builtin,
            'rawFrameTimes': raw_times,
            'artifactBindingQualified': binding_ok, 'window': window, 'windowScope': window_scope,
            'collector': summary, 'collectorWindowScope': 'collector polling window; not silently clipped or aligned to benchmark window',
            'frameIntervals': intervals, 'cpuProfile': log, 'thermal': heat,
            'observedSourceTargetFpsValues': observed_targets,
            'sourceTargetObservations': {'samples': len(samples), 'explicitNull': null_targets,
                                         'missing': missing_targets, 'numericValues': observed_targets},
            'baselineMode': baseline_mode, 'performanceGoalAssessment': goal_assessment,
            'actualSourceTextureSizes': sorted(map(list, actual_sizes)), 'configuration': config,
            'qualificationIssues': sorted(set(issues)),
            'provenance': {'manifest': provenance(path), 'config': provenance(directory / 'config-before.json'),
                           'samples': provenance(directory / 'samples.json'),
                           'benchmarkBinding': provenance(binding_path) if binding_path and binding_path.is_file() else None},
            'scope': 'one process/run; built-in report FPS, guest boundaries, host source intervals and GPU/display timing are separate domains'}


def paired_report(trials):
    groups, pairs = {}, []
    for trial in trials:
        groups.setdefault(trial['pairId'], []).append(trial)
    for identity, rows in groups.items():
        issues = []
        off = next((row for row in rows if not row['enabled']), None)
        on = next((row for row in rows if row['enabled']), None)
        if len(rows) != 2 or off is None or on is None or off['feature'] != on['feature']:
            pairs.append({'pairId': identity, 'qualified': False, 'issues': ['requires_exactly_one_OFF_and_one_ON_same_feature']})
            continue
        feature = on['feature']
        before = {k: v for k, v in off['configuration'].items() if k != feature}
        after = {k: v for k, v in on['configuration'].items() if k != feature}
        differences = sorted(key for key in before.keys() | after.keys()
                             if key not in before or key not in after or before[key] != after[key])
        if differences:
            issues.append('unapproved_config_differences')
        if off['benchmarkSceneId'] != on['benchmarkSceneId']:
            issues.append('benchmark_scene_changed')
        if off['windowScope'] != on['windowScope']:
            issues.append('benchmark_window_scope_changed')
        for key in off['conditions'].keys() | on['conditions'].keys():
            if key not in off['conditions'] or key not in on['conditions'] or off['conditions'][key] != on['conditions'][key]:
                issues.append('condition_changed:' + key)
        if off['collector']['pid'] == on['collector']['pid']:
            issues.append('independent_cold_processes_unconfirmed')
        if (off['collector']['device'], off['collector']['bundle']) != (on['collector']['device'], on['collector']['bundle']):
            issues.append('device_or_bundle_changed')
        for label, row in [('OFF', off), ('ON', on)]:
            issues.extend(label + ':' + issue for issue in row['qualificationIssues'])
        off_passes = {row['pass']: row for row in off['benchmark'].get('passes', [])}
        on_passes = {row['pass']: row for row in on['benchmark'].get('passes', [])}
        if off_passes.keys() != on_passes.keys():
            issues.append('benchmark_passes_differ')
        deltas = [{'pass': key, 'reportedAvgFpsOff': off_passes[key]['avgFps'], 'reportedAvgFpsOn': on_passes[key]['avgFps'],
                   'reportedAvgFpsDelta': on_passes[key]['avgFps'] - off_passes[key]['avgFps']}
                  for key in off_passes.keys() & on_passes.keys()]
        off_raw = {row['pass']: row for row in off['rawFrameTimes']['passes'] if row['qualifiedArtifactAndCount']}
        on_raw = {row['pass']: row for row in on['rawFrameTimes']['passes'] if row['qualifiedArtifactAndCount']}
        raw_deltas = [{'pass': key, 'throughputFpsOff': off_raw[key]['throughputFps'],
                       'throughputFpsOn': on_raw[key]['throughputFps'],
                       'throughputFpsDelta': on_raw[key]['throughputFps'] - off_raw[key]['throughputFps'],
                       'onePercentLowFpsDelta': on_raw[key]['onePercentLowFps'] - off_raw[key]['onePercentLowFps'],
                       'slowFrame50msDelta': {metric: on_raw[key]['slowFrame50ms'][metric] - off_raw[key]['slowFrame50ms'][metric]
                                               for metric in ('frames', 'framePercent', 'framesPerMinute', 'consecutiveRuns', 'runsPerMinute', 'longestConsecutiveFrames', 'longestConsecutiveDurationMs')},
                       'rawFrameTimeMsDelta': {metric: on_raw[key]['frameTimeMs'][metric] - off_raw[key]['frameTimeMs'][metric]
                                               for metric in ('p50', 'p95', 'p99')}}
                      for key in sorted(off_raw.keys() & on_raw.keys())]
        order_known = all(integer(row['order']) for row in rows) and len({row['order'] for row in rows}) == 2
        pairs.append({'pairId': identity, 'feature': feature, 'OFF': off['trialId'], 'ON': on['trialId'],
                      'order': [row['trialId'] for row in sorted(rows, key=lambda row: row['order'])] if order_known else None,
                      'featureOrder': [row['enabled'] for row in sorted(rows, key=lambda row: row['order'])] if order_known else None,
                      'controlFingerprint': hashlib.sha256(json.dumps({'config': before, 'conditions': off['conditions'],
                                                                       'sceneId': off['benchmarkSceneId'], 'windowScope': off['windowScope'],
                                                                       'device': off['collector']['device'], 'bundle': off['collector']['bundle']}, sort_keys=True).encode()).hexdigest(),
                      'qualifiedConfigurationAndBenchmarkBinding': not issues,
                      'issues': sorted(set(issues)), 'unapprovedConfigurationKeys': differences,
                      'reportedBuiltinDifferences': sorted(deltas, key=lambda row: row['pass']),
                      'primaryRawFrameTimeDifferences': raw_deltas,
                      'thermalAvailableBoth': off['thermal']['available'] and on['thermal']['available'],
                      'causalPerformanceImprovementEstablished': False,
                      'limitations': ['One pair cannot establish run-to-run variance; repeat reverse order and report paired results.',
                                      ('Unlimited source mode has no declared source pacing budget; the 30 FPS goal is a diagnostic threshold, not a cap.'
                                       if off['baselineMode'] == 'unlimited-native-800p' else
                                       'A source cap of 20 can hide headroom gains; evaluate qualified source interval tails and relevant CPU/cache/pool facts.'),
                                      'Thermal/power/throttling equivalence and built-in artifact source-vs-generated-frame semantics remain separate evidence requirements.']})
    return pairs


def replicates(pairs):
    groups = {}
    for pair in pairs:
        if pair.get('qualifiedConfigurationAndBenchmarkBinding'):
            groups.setdefault((pair['feature'], pair['controlFingerprint']), []).append(pair)
    result = []
    for (feature, identity), rows in groups.items():
        passes, raw_passes = {}, {}
        for pair in rows:
            for delta in pair['reportedBuiltinDifferences']:
                passes.setdefault(delta['pass'], []).append(delta['reportedAvgFpsDelta'])
            for delta in pair['primaryRawFrameTimeDifferences']:
                raw_passes.setdefault(delta['pass'], []).append(delta)
        result.append({'feature': feature, 'controlFingerprint': identity,
                       'pairIds': [row['pairId'] for row in rows], 'independentPairs': len(rows),
                       'reverseOrdersObserved': {tuple(row['featureOrder']) for row in rows if row['featureOrder']} == {(False, True), (True, False)},
                       'reportedPassDeltas': {key: distribution(values) for key, values in sorted(passes.items())},
                       'primaryRawPassDeltas': {key: {'throughputFpsDelta': distribution([value['throughputFpsDelta'] for value in values]),
                                                      'onePercentLowFpsDelta': distribution([value['onePercentLowFpsDelta'] for value in values]),
                                                      'slowFrame50msDelta': {metric: distribution([value['slowFrame50msDelta'][metric] for value in values])
                                                                              for metric in ('framePercent', 'framesPerMinute', 'runsPerMinute', 'longestConsecutiveFrames', 'longestConsecutiveDurationMs')},
                                                      'rawFrameTimeMsDelta': {metric: distribution([value['rawFrameTimeMsDelta'][metric] for value in values])
                                                                              for metric in ('p50', 'p95', 'p99')}}
                                                for key, values in sorted(raw_passes.items())},
                       'confidenceInterval': None,
                       'scope': 'descriptive paired-run deltas only; correlated frames are not treated as independent runs; no causal claim or unmeasured thermal correction'})
    return result


def analyze(paths):
    trials = [analyze_trial(path.resolve()) for path in paths]
    if len({trial['trialId'] for trial in trials}) != len(trials):
        raise ValueError('duplicate trialId')
    captures = [str(Path(row['provenance']['config']['path']).parent) for row in trials]
    if len(set(captures)) != len(captures):
        raise ValueError('duplicate capture evidence cannot be relabeled as independent trials')
    processes = {}
    for row in trials:
        identity = (row['collector']['device'], row['collector']['bundle'], row['collector']['pid'])
        processes.setdefault(identity, []).append(row)
    for rows in processes.values():
        if len(rows) > 1:
            for row in rows:
                row['qualificationIssues'].append('process_identity_reused_across_trials_no_fresh_process_proof')
    pairs = paired_report(trials)
    return {'schemaVersion': 1, 'trials': trials, 'pairs': pairs, 'replicates': replicates(pairs),
            'scope': 'offline evidence analysis; no automated rollout/default-enable decision; unmeasured fields remain unknown'}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--trial', action='append', type=Path, required=True)
    parser.add_argument('--out', type=Path, required=True)
    args = parser.parse_args()
    result = analyze(args.trial)
    output = args.out.resolve()
    protected = [path.resolve() for path in args.trial]
    protected.extend(Path(trial['provenance']['config']['path']).parent for trial in result['trials'])
    def inputs(value):
        if isinstance(value, dict):
            if isinstance(value.get('path'), str) and value.get('sha256'):
                yield Path(value['path']).resolve()
            for child in value.values():
                yield from inputs(child)
        elif isinstance(value, list):
            for child in value:
                yield from inputs(child)
    protected.extend(inputs(result))
    if output in protected or any(path.is_dir() and path in output.parents for path in protected):
        raise ValueError('write analysis outside raw evidence directories and manifest inputs')
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(json.dumps(result, ensure_ascii=False, indent=2, allow_nan=False) + '\n')
    print(json.dumps({'out': str(output), 'trials': len(result['trials']), 'pairs': len(result['pairs'])}))


if __name__ == '__main__':
    main()
