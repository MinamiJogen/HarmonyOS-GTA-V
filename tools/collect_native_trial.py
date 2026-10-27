#!/usr/bin/env python3
"""Read-only fixed-configuration source-frame trial; never injects game input.

Guest source boundaries, native source/AI/repeat calls, and actual compositor
display are distinct. Percentiles require in-process monotonic frame telemetry;
coarse polling never fabricates per-frame latency or display FPS.
"""
import argparse
import hashlib
import json
import math
from pathlib import Path, PurePosixPath
import re
import subprocess
import time

HDC = '/Applications/DevEco-Studio.app/Contents/sdk/default/openharmony/toolchains/hdc'
FILES = '/data/storage/el2/base/haps/entry/files/'


def validate_native_size(value):
    if (not isinstance(value, (list, tuple)) or len(value) != 2
            or any(type(n) is not int or n <= 0 or n > 8192 or n % 2 for n in value)
            or min(value) != 800):
        raise ValueError('native size requires two positive even integers, short edge800 and extents<=8192')
    return list(value)


def parse_native_size(value):
    if not isinstance(value, str) or not re.fullmatch(r'[1-9][0-9]*x[1-9][0-9]*', value):
        raise ValueError('expected native size must be WIDTHxHEIGHT')
    return validate_native_size([int(n) for n in value.split('x')])


class AppFiles:
    """Explicit app-file transport; never falls back or changes permissions."""
    def __init__(self, bundle, mode='sandbox', root=None):
        if mode not in ('sandbox', 'direct'):
            raise ValueError('unsupported file transfer mode')
        if mode == 'sandbox':
            if root is not None:
                raise ValueError('--files-root requires direct file transfer mode')
            self.root = FILES
        else:
            if (not isinstance(bundle, str) or not bundle or not isinstance(root, str)
                    or not root.startswith('/data/app/el2/') or '\\' in root
                    or any(part in ('.', '..', '') for part in root.strip('/').split('/'))
                    or str(PurePosixPath(root)) != root.rstrip('/')
                    or PurePosixPath(root).parts[-4:] != (bundle, 'haps', 'entry', 'files')):
                raise ValueError('direct transfer requires explicit normalized /data/app/el2/.../BUNDLE/haps/entry/files root')
            self.root = root.rstrip('/') + '/'
        self.bundle, self.mode = bundle, mode

    def path(self, name):
        if not isinstance(name, str) or name.startswith('/') or '..' in PurePosixPath(name).parts:
            raise ValueError('app-file name must stay inside files root')
        return self.root + name

    def command(self, operation, source, destination, compressed=False):
        if operation not in ('recv', 'send'):
            raise ValueError('unsupported file operation')
        return ['file', operation] + (['-z'] if compressed else []) + (['-b', self.bundle] if self.mode == 'sandbox' else []) + [str(source), str(destination)]

    def conditions(self):
        return {'mode': self.mode, 'filesRoot': self.root}


def add_device_file_options(parser):
    parser.add_argument('--file-transfer-mode', choices=('sandbox', 'direct'), default='sandbox')
    parser.add_argument('--files-root', help='explicit already-readable app files root; required only for direct mode')
    parser.add_argument('--expected-native-size', help='explicit WIDTHxHEIGHT with even extents and short edge800; default1200x800')


def read_runtime(wrapper):
    runtime = json.loads(wrapper['runtimeJson'])
    if runtime.get('error'):
        raise ValueError('runtime error: ' + runtime['error'])
    if not runtime.get('worldPlayerObserved') or not wrapper.get('uiAppForeground'):
        raise ValueError('require foreground game world, not a loading screen or menu')
    if runtime.get('appForeground') is False or any(runtime.get(key) for key in
            ('presentationPaused', 'stopRequested', 'saveLoadPending', 'surfaceRecoveryRequired')):
        raise ValueError('native game is paused, stopping, loading or awaiting surface recovery')
    return runtime


def summarize(samples):
    first, last = samples[0], samples[-1]
    elapsed = (last['monotonicNs'] - first['monotonicNs']) / 1e9
    if elapsed <= 0 or any(b['monotonicNs'] <= a['monotonicNs'] for a, b in zip(samples, samples[1:])):
        raise ValueError('non-advancing or stale process timestamps')
    if len({s['pid'] for s in samples}) != 1 or not first['pid']:
        raise ValueError('process changed or PID unavailable')
    if len({tuple(s['renderSize']) for s in samples}) != 1:
        raise ValueError('render size changed')
    if any(b['guestPresents'] < a['guestPresents'] for a, b in zip(samples, samples[1:])):
        raise ValueError('source counter rewound')
    result = {'pid': first['pid'], 'elapsedSeconds': elapsed, 'renderSize': first['renderSize'],
              'guestSourceFrames': last['guestPresents'] - first['guestPresents'],
              'guestSourceFps': (last['guestPresents'] - first['guestPresents']) / elapsed,
              'nativePresentCalls': last['presents'] - first['presents'],
              'nativePresentCallsPerSecond': (last['presents'] - first['presents']) / elapsed,
              'scope': 'guest source boundaries and host present calls; not GPU timestamps, compositor/display FPS, or input-to-display latency'}
    windows = []
    for a, b in zip(samples, samples[1:]):
        span = (b['monotonicNs'] - a['monotonicNs']) / 1e9
        windows.append((b['guestPresents'] - a['guestPresents']) / span)
    result['sourceFpsWindows'] = windows
    return result


def presentation_summary(snapshots):
    first, last = snapshots[0], snapshots[-1]
    if len({(s['pid'], s['epoch']) for s in snapshots}) != 1:
        raise ValueError('FG process or history epoch changed')
    start, end = first['statusSnapshotGeneratedMonotonicNs'], last['statusSnapshotGeneratedMonotonicNs']
    if any(b['statusSnapshotGeneratedMonotonicNs'] <= a['statusSnapshotGeneratedMonotonicNs'] for a,b in zip(snapshots,snapshots[1:])):
        raise ValueError('FG snapshots stale')
    elapsed = (end - start) / 1e9
    counters = {}
    for key in ('sourcePresents', 'aiPresents', 'repeatPresents', 'missedDeadlineSlots', 'surfaceAcquireFailures'):
        delta = last[key] - first[key]
        if delta < 0:
            raise ValueError('FG counters rewound')
        counters[key] = delta
    # Overlapping bounded traces are merged by actual output slot, and only
    # real source calls in this interval enter source-cadence percentiles.
    rows = {}
    for snapshot in snapshots:
        for row in snapshot.get('recentPresents', []):
            timestamp = row.get('presentBeginMonotonicNs', 0)
            if row.get('nativePresentCalled') and row.get('kind') == 'source' and start <= timestamp <= end:
                rows[row['slot']] = row
    ordered = sorted(rows.values(), key=lambda row: row['presentBeginMonotonicNs'])
    intervals = sorted((b['presentBeginMonotonicNs'] - a['presentBeginMonotonicNs']) / 1e6
                       for a,b in zip(ordered, ordered[1:]))
    complete = len(ordered) == counters['sourcePresents']
    # Missing retained rows must never become an enormous invented frame time.
    # Cumulative native source counts independently establish trace coverage.
    percentiles = {f'p{p}Ms': intervals[max(0, math.ceil(len(intervals)*p/100)-1)] if complete and intervals else None
                   for p in (50,95,99)}
    return {'elapsedSeconds': elapsed, 'counterDeltas': counters,
            'newSourcePresentCallsPerSecond': counters['sourcePresents']/elapsed,
            'newAiPresentCallsPerSecond': counters['aiPresents']/elapsed,
            'sourceAndAiPresentCallsPerSecond': (counters['sourcePresents']+counters['aiPresents'])/elapsed,
            'sourcePresentInterval': {'samples': len(intervals), **percentiles},
            'sourceTraceCoverage': {'complete': complete, 'retainedSourceCalls': len(ordered),
                                    'expectedSourceCalls': counters['sourcePresents'],
                                    'percentilesAvailable': complete and bool(intervals)},
            'scope': 'deduplicated native source-call cadence; repeats excluded; not scanout or input-to-display latency'}


def collect(args):
    files = AppFiles(args.bundle, getattr(args, 'file_transfer_mode', 'sandbox'), getattr(args, 'files_root', None))
    explicit_size = getattr(args, 'expected_native_size', None)
    expected_size = parse_native_size(explicit_size) if explicit_size is not None else [1200, 800]
    out = args.directory.resolve()
    out.mkdir(parents=True, exist_ok=True)
    if list(out.iterdir()):
        raise ValueError('use an empty trial directory; existing evidence is preserved')
    prefix = [args.hdc, '-t', args.device]

    def call(command):
        done = subprocess.run(prefix + command, capture_output=True, text=True, timeout=25)
        if done.returncode:
            raise RuntimeError(done.stdout + done.stderr)
        return done.stdout

    def receive(name, path):
        call(files.command('recv', files.path(name), path))
        if not path.is_file():
            raise RuntimeError('transfer did not create ' + str(path))
        return json.loads(path.read_text())

    try:
        config = receive('config.json', out / 'config-before.json')
        if config.get('graphics_preset') != 'medium':
            raise ValueError('user-selected baseline requires medium preset')
        pid = int(call(['shell', 'pidof', args.bundle]).strip())
        samples = []
        fg_snapshots = []
        begin = time.monotonic()
        for index in range(args.samples):
            delay = begin + index * args.interval - time.monotonic()
            if delay > 0:
                time.sleep(delay)
            wrapper = receive('game-status.json', out / f'status-{index:03d}.json')
            runtime = read_runtime(wrapper)
            if runtime.get('renderSize') != expected_size:
                raise ValueError('actual native size differs from expected native size')
            live = runtime.get('frameGenerationLive', {})
            sample = {'pid': live.get('processId', pid),
                      'monotonicNs': live.get('observedMonotonicNs'),
                      'guestPresents': runtime['guestPresents'], 'presents': runtime['presents'],
                      'renderSize': runtime['renderSize'], 'nativeTelemetry': runtime.get('nativeTelemetry')}
            if not isinstance(sample['monotonicNs'], int):
                raise ValueError('explicit in-process CLOCK_MONOTONIC timestamps required')
            if sample['pid'] != pid:
                raise ValueError('PID changed')
            samples.append(sample)
            if config.get('frame_interpolation_enabled') is True:
                fg = receive('frame-interpolation-status.json', out / f'fg-{index:03d}.json')
                if fg.get('pid') != pid or fg.get('error'):
                    raise ValueError('FG process changed or execution failed')
                source = fg.get('guestFinalSource') or {}
                if [source.get('width'), source.get('height')] != expected_size:
                    raise ValueError('FG actual native texture differs from expected native size')
                fg_snapshots.append(fg)
            if index % 10 == 0:
                print(json.dumps({'sample': index, 'sourceFrames': sample['guestPresents']}, ensure_ascii=False), flush=True)
        receive('config.json', out / 'config-after.json')
        if (out / 'config-before.json').read_bytes() != (out / 'config-after.json').read_bytes():
            raise ValueError('config changed during trial')
        if int(call(['shell', 'pidof', args.bundle]).strip()) != pid:
            raise ValueError('PID changed during trial')
        report = summarize(samples)
        if fg_snapshots:
            report['presentations'] = presentation_summary(fg_snapshots)
        report.update({'device': args.device, 'bundle': args.bundle, 'samples': len(samples),
                       'graphicsPreset': 'medium', 'configSha256': hashlib.sha256((out / 'config-before.json').read_bytes()).hexdigest(),
                       'superResolution': config.get('spatial_upscale_enabled'),
                       'frameGeneration': config.get('frame_interpolation_enabled')})
        if files.mode != 'sandbox':
            report['fileTransport'] = files.conditions()
        if explicit_size is not None:
            report['nativeRenderSize'] = expected_size
        (out / 'summary.json').write_text(json.dumps(report, ensure_ascii=False, indent=2) + '\n')
        (out / 'samples.json').write_text(json.dumps(samples, ensure_ascii=False, indent=2) + '\n')
        print(json.dumps(report, ensure_ascii=False, indent=2), flush=True)
    except Exception as error:
        (out / 'error.json').write_text(json.dumps({'error': str(error)}, ensure_ascii=False, indent=2) + '\n')
        raise


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--hdc', default=HDC)
    p.add_argument('--device', required=True)
    p.add_argument('--bundle', default='com.minami.harmonyruntimeprobe')
    p.add_argument('--directory', type=Path, required=True)
    p.add_argument('--samples', type=int, default=31)
    p.add_argument('--interval', type=float, default=2)
    add_device_file_options(p)
    args = p.parse_args()
    if args.samples < 2 or not 0.5 <= args.interval <= 30:
        p.error('at least two samples and 0.5..30 second interval required')
    collect(args)


if __name__ == '__main__':
    main()
