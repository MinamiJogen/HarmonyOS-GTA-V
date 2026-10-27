#!/usr/bin/env python3
"""Explicitly authorized PSO device session; preserves config and existing data.

Only changes the selected app's config and lifecycle, and sends documented
camera swipes in demo mode. A full benchmark receives no game input. Evidence
includes raw commands, native-clock samples, screenshots and process traces.
"""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import time

from prepare_native_trial import launch_demo

HDC = '/Applications/DevEco-Studio.app/Contents/sdk/default/openharmony/toolchains/hdc'
BUNDLE = 'com.minami.harmonyruntimeprobe'
FILES = '/data/app/el2/100/base/' + BUNDLE + '/haps/entry/files/'


def write(path, value):
    path.write_text(json.dumps(value, ensure_ascii=False, indent=2) + '\n')


def nodes(value):
    if isinstance(value, dict):
        if 'attributes' in value:
            yield value['attributes']
        for key, child in value.items():
            if key != 'attributes':
                yield from nodes(child)
    elif isinstance(value, list):
        for child in value:
            yield from nodes(child)


class Trial:
    def __init__(self, args):
        self.args = args
        self.out = args.directory.resolve()
        self.out.mkdir()
        self.commands = (self.out / 'commands.jsonl').open('w')
        self.prefix = [args.hdc, '-t', args.device]
        self.samples = []
        self.pid = None
        self.image_index = 0

    def call(self, command, timeout=35):
        began = time.monotonic()
        result = subprocess.run(self.prefix + command, capture_output=True, text=True, timeout=timeout)
        self.commands.write(json.dumps({'hostBeginMonotonic': began, 'hostEndMonotonic': time.monotonic(),
            'command': self.prefix + command, 'exitCode': result.returncode,
            'stdout': result.stdout, 'stderr': result.stderr}, ensure_ascii=False) + '\n')
        self.commands.flush()
        if result.returncode:
            raise RuntimeError(result.stdout + result.stderr)
        return result.stdout

    def receive(self, name, destination, private=False, optional=False):
        command = ['file', 'recv'] + (['-b', BUNDLE] if private else [])
        source = ('/data/storage/el2/base/haps/entry/files/' if private else FILES) + name
        try:
            self.call(command + [source, str(destination)], timeout=60)
            if not destination.exists():
                raise RuntimeError('file transfer produced no file: ' + source)
            return destination
        except Exception:
            if not optional:
                raise
            return None

    def sample(self):
        path = self.out / f'status-{len(self.samples):04d}.json'
        self.receive('game-status.json', path)
        wrapper = json.loads(path.read_text())
        runtime = json.loads(wrapper.get('runtimeJson', '{}'))
        if runtime.get('processId') != self.pid:
            return None
        clock = runtime.get('nativeTelemetry', {}).get('monotonicNs')
        if not isinstance(clock, int) or clock <= 0:
            return None
        if self.samples and clock <= self.samples[-1]['monotonicNs']:
            return None
        if runtime.get('error'):
            raise RuntimeError(runtime['error'])
        if runtime.get('appForeground') is False or runtime.get('presentationPaused'):
            raise RuntimeError('target game left foreground or paused')
        row = {'monotonicNs': clock, 'hostMonotonic': time.monotonic(), 'pid': self.pid,
            'guestPresents': runtime.get('guestPresents'), 'presents': runtime.get('presents'),
            'worldPlayerObserved': runtime.get('worldPlayerObserved'),
            'renderSize': runtime.get('renderSize'), 'conditions': wrapper.get('uiBenchmarkConditions'),
            'statusFile': path.name}
        self.samples.append(row)
        write(self.out / 'samples.json', self.samples)
        return row

    def picture(self, label):
        self.image_index += 1
        remote = f'/data/local/tmp/pso-{self.pid}-{self.image_index}.jpeg'
        path = self.out / f'{label}.jpeg'
        self.call(['shell', 'snapshot_display', '-f', remote])
        self.call(['file', 'recv', remote, str(path)])
        self.call(['shell', 'rm', '-f', remote])
        item = {'path': path.name, 'sha256': hashlib.sha256(path.read_bytes()).hexdigest(),
            'previousSampleMonotonicNs': self.samples[-1]['monotonicNs'] if self.samples else None,
            'hostMonotonic': time.monotonic()}
        with (self.out / 'screens.jsonl').open('a') as stream:
            stream.write(json.dumps(item) + '\n')
        return item

    def layout(self, label):
        remote = f'/data/local/tmp/pso-{self.pid}-layout.json'
        path = self.out / f'{label}.json'
        self.call(['shell', 'uitest', 'dumpLayout', '-p', remote])
        self.call(['file', 'recv', remote, str(path)])
        self.call(['shell', 'rm', '-f', remote])
        return json.loads(path.read_text())

    def start(self):
        config = json.loads(self.args.baseline.read_text())
        config.update(pso_warmup=self.args.enabled == 'true', pso_warmup_read_only=self.args.read_only == 'true',
            frame_interval_trace=True, frame_interpolation_slow_trace=True)
        if self.args.mode == 'suite':
            config['extra_args'] = ['-benchmark', '-benchmarkFrameTimes', '-benchmarkIterations=1',
                                    *config.get('extra_args', [])]
        write(self.out / 'config-requested.json', config)
        self.call(['shell', 'aa', 'force-stop', BUNDLE])
        self.call(['file', 'send', str(self.out / 'config-requested.json'), FILES + 'config.json'])
        self.receive('config.json', self.out / 'config-effective.json')
        assert json.loads((self.out / 'config-effective.json').read_text()) == config
        self.call(['shell', 'aa', 'start', '-b', BUNDLE, '-a', 'EntryAbility'])
        time.sleep(2)
        result = self.call(['shell', 'pidof', BUNDLE]).strip().split()
        assert len(result) == 1 and result[0].isdigit(), result
        self.pid = int(result[0])
        remote = f'/data/local/tmp/pso-{self.pid}-launcher.json'
        launch_demo(self.call, self.out / 'launcher-layout.json', remote, timeout=45, bundle=BUNDLE)
        write(self.out / 'run.json', {'pid': self.pid, 'mode': self.args.mode, 'enabled': self.args.enabled,
            'readOnly': self.args.read_only, 'baseline': str(self.args.baseline.resolve()),
            'scope': 'native source cadence and host pipeline wall time; visual scene evidence retained separately'})

    def capture(self):
        began = time.monotonic()
        ready = None
        camera_start = None
        camera_end = None
        swipes = 0
        camera = None
        images = []
        next_picture = 0
        while time.monotonic() - began < self.args.seconds:
            row = self.sample()
            if row is None:
                time.sleep(1)
                continue
            elapsed = time.monotonic() - began
            if elapsed >= next_picture:
                images.append(self.picture(f'scene-{int(elapsed):03d}'))
                next_picture = elapsed + (15 if self.args.mode == 'suite' else 30)
            if row['worldPlayerObserved'] and ready is None:
                ready = row
                write(self.out / 'world-first-observed.json', ready)
            if self.args.mode == 'demo' and ready and row['monotonicNs'] - ready['monotonicNs'] >= 12_000_000_000:
                if camera is None:
                    tree = self.layout('camera-layout')
                    surfaces = [n for n in nodes(tree) if n.get('type') == 'XComponent' and n.get('visible') == 'true']
                    assert len(surfaces) == 1, 'require one visible current game XComponent'
                    import re
                    x0,y0,x1,y1 = map(int, re.findall(r'-?\d+', surfaces[0]['bounds']))
                    camera = [round(x0+(x1-x0)*.42),round(y0+(y1-y0)*.4),
                              round(x0+(x1-x0)*.66),round(y0+(y1-y0)*.4)]
                    write(self.out / 'camera-path.json', {'bounds': [x0,y0,x1,y1], 'path': camera,
                        'scope': 'central visible game surface; validate camera movement in retained screenshots'})
                    images.append(self.picture('camera-before'))
                    row = self.sample() or row
                    camera_start = row['monotonicNs']
                if swipes < 8:
                    path = camera if swipes % 4 < 2 else camera[2:] + camera[:2]
                    self.call(['shell', 'uitest', 'uiInput', 'swipe', *map(str, path), '600'])
                    swipes += 1
                elif camera_end is None:
                    camera_end = row['monotonicNs']
                    images.append(self.picture('camera-after'))
                elif row['monotonicNs'] - camera_end > 12_000_000_000:
                    break
            if len(self.samples) % 5 == 0:
                print(json.dumps({'run':self.out.name,'pid':self.pid,'seconds':round(elapsed),
                    'world':row['worldPlayerObserved'],'guestPresents':row['guestPresents'],'swipes':swipes}),flush=True)
            time.sleep(2)
        if not self.samples:
            raise RuntimeError('no fresh native status samples')
        windows = []
        if ready:
            entry_end = camera_start or min(ready['monotonicNs'] + 12_000_000_000, self.samples[-1]['monotonicNs'])
            windows.append({'name':'first_entry','startMonotonicNs':self.samples[0]['monotonicNs'],
                'endMonotonicNs':entry_end,'evidence':{'firstWorldStatus': 'world-first-observed.json',
                    'scope':'enclosing startup-to-first-world plus12s bracket; source intervals begin after first present'}})
        if camera_start and camera_end:
            windows.append({'name':'camera_rotation','startMonotonicNs':camera_start,'endMonotonicNs':camera_end,
                'evidence':{'swipes':swipes,'layout':'camera-layout.json','before':'camera-before.jpeg','after':'camera-after.jpeg'}})
        if self.args.mode == 'suite':
            windows.append({'name':'full_suite_observation','startMonotonicNs':self.samples[0]['monotonicNs'],
                'endMonotonicNs':self.samples[-1]['monotonicNs'],
                'evidence':{'images':images,'scope':'observed full benchmark route; completion and scene subwindows require visual/report validation'}})
        write(self.out/'windows.json',{'schemaVersion':1,'pid':self.pid,'clock':'CLOCK_MONOTONIC_ns','windows':windows})

    def finish(self):
        if self.pid is None:
            return
        self.receive('game-runtime.log', self.out / 'runtime-all.log', optional=True)
        path = self.out / 'runtime-all.log'
        if path.exists():
            data=path.read_bytes();begin=data.rfind(b'[config] effective -FrameQueueLimit=')
            (self.out/'runtime-session.log').write_bytes(data[begin:] if begin>=0 else data)
            # Keep the exact current-session bytes; the device's append-only
            # file also contains old runs already captured in their own dirs.
            path.unlink()
        self.receive(f'native-frame-intervals-{self.pid}.jsonl',self.out/'frame-intervals.jsonl',optional=True)
        self.receive(f'gpu-slow-operations-{self.pid}.jsonl',self.out/'slow-operations.jsonl',private=True,optional=True)
        self.receive('native-pso-warmup-v1/recipes.json',self.out/'recipes.json',private=True,optional=True)
        self.receive('config.json',self.out/'config-after.json',optional=True)
        if self.args.mode=='suite':
            self.receive('saves/userdata/Documents/Rockstar Games/GTA V/Benchmarks',self.out/'benchmark-results',optional=True)
        self.commands.close()


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--hdc',default=HDC)
    parser.add_argument('--device',required=True)
    parser.add_argument('--baseline',type=Path,required=True)
    parser.add_argument('--directory',type=Path,required=True)
    parser.add_argument('--enabled',choices=['true','false'],required=True)
    parser.add_argument('--read-only',choices=['true','false'],default='true')
    parser.add_argument('--mode',choices=['demo','suite'],default='demo')
    parser.add_argument('--seconds',type=float,default=240)
    args=parser.parse_args()
    trial=Trial(args)
    try:
        trial.start()
        trial.capture()
    except Exception as error:
        write(trial.out/'error.json',{'error':str(error),'pid':trial.pid})
        raise
    finally:
        trial.finish()


if __name__=='__main__':
    main()
