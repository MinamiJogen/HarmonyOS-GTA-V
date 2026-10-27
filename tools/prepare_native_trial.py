#!/usr/bin/env python3
"""Restart a debug game with explicit native-only A/B flags and warm the world.

Preserves all graphics selections, resources and saves. The Demo spawn/camera
is not deterministic: this is a controlled configuration, not a replay proof.
"""
import argparse
import json
from pathlib import Path
import re
import subprocess
import time

from collect_native_trial import HDC, FILES, AppFiles, add_device_file_options, parse_native_size

FLAGS = {'shared_memory_fastpath', 'cached_readback', 'occlusion_pool',
         'subresource_proxies', 'pipeline_cache', 'native_render_fastpath', 'io_handle_lru',
         'pipeline_cache_read_only', 'inline_upload_borrow', 'native_command_fastpath', 'skip_empty_init_transition',
         'skip_empty_render_prepass', 'pso_warmup', 'pso_warmup_read_only'}


def nodes(tree):
    if isinstance(tree, dict):
        if isinstance(tree.get('attributes'), dict):
            yield tree['attributes']
        for key, value in tree.items():
            if key != 'attributes':
                yield from nodes(value)
    elif isinstance(tree, list):
        for value in tree:
            yield from nodes(value)


def valid_bounds(node):
    value = node.get('bounds')
    if not isinstance(value, str) or not re.fullmatch(r'\[-?\d+,-?\d+\]\[-?\d+,-?\d+\]', value):
        return None
    box = [int(n) for n in re.findall(r'-?\d+', value)]
    return box if box[0] >= 0 and box[1] >= 0 and box[2] > box[0] and box[3] > box[1] else None


def launcher_subtree(tree, bundle):
    """Require one visible target ability/page root from this fresh layout."""
    found = []
    def visit(value):
        if isinstance(value, dict):
            attrs = value.get('attributes', {})
            if (attrs.get('bundleName', bundle) != bundle
                    or attrs.get('abilityName', 'EntryAbility') != 'EntryAbility'
                    or attrs.get('pagePath', 'pages/Index') != 'pages/Index'):
                return
            if (attrs.get('type') == 'root' and attrs.get('bundleName') == bundle
                    and attrs.get('abilityName') == 'EntryAbility' and attrs.get('pagePath') == 'pages/Index'
                    and attrs.get('visible', 'true') == 'true'):
                found.append(value)
                return
            for key, child in value.items():
                if key != 'attributes':
                    visit(child)
        elif isinstance(value, list):
            for child in value:
                visit(child)
    visit(tree)
    return found[0] if len(found) == 1 else None


def launcher_nodes(tree, bundle):
    if isinstance(tree, dict):
        attrs = tree.get('attributes', {})
        if (attrs.get('bundleName', bundle) != bundle
                or attrs.get('abilityName', 'EntryAbility') != 'EntryAbility'
                or attrs.get('pagePath', 'pages/Index') != 'pages/Index'):
            return
        if attrs:
            yield attrs
        for key, child in tree.items():
            if key != 'attributes':
                yield from launcher_nodes(child, bundle)
    elif isinstance(tree, list):
        for child in tree:
            yield from launcher_nodes(child, bundle)


def launch_demo(call, layout_path, remote, timeout=35, bundle=None):
    """Reveal at most three pages of the actual launcher Scroll, then click a fresh node."""
    deadline, scrolls, ready_seen = time.monotonic() + timeout, 0, False
    actions = []
    while time.monotonic() < deadline:
        call(['shell', 'uitest', 'dumpLayout', '-p', remote])
        call(['file', 'recv', remote, str(layout_path)])
        tree = json.loads(layout_path.read_text())
        if bundle is not None:
            subtree = launcher_subtree(tree, bundle)
            if subtree is None:
                ready_seen = False
                time.sleep(1)
                continue
            current = list(launcher_nodes(subtree, bundle))
        else:
            # Backwards-compatible pure helper fixtures only. Production
            # runner and prepare calls always pass the selected bundle.
            current = list(nodes(tree))
        ready_seen |= any(n.get('text') == '资源已就绪' and n.get('visible', 'true') == 'true' for n in current)
        options = [n for n in current if n.get('type') == 'Button' and n.get('text') == '开始 Demo'
                   and n.get('enabled') == 'true' and n.get('visible', 'true') == 'true'
                   and n.get('clickable', 'true') == 'true' and valid_bounds(n)]
        if len(options) == 1:
            box = valid_bounds(options[0])
            command = ['shell', 'uitest', 'uiInput', 'click', str((box[0]+box[2])//2), str((box[1]+box[3])//2)]
            call(command)
            actions.append({'action': 'click', 'nodeBounds': box, 'command': command})
            (layout_path.parent / 'launcher-actions.json').write_text(json.dumps(actions, ensure_ascii=False, indent=2)+'\n')
            return
        # Ambiguous buttons must not cause navigation or guessed clicks.
        if not options and ready_seen and scrolls < 3:
            scroll = [n for n in current if n.get('type') == 'Scroll' and n.get('scrollable') == 'true'
                      and n.get('enabled') == 'true' and n.get('visible') == 'true' and valid_bounds(n)]
            if len(scroll) == 1:
                x0,y0,x1,y1 = valid_bounds(scroll[0])
                if x1-x0 >= 100 and y1-y0 >= 100:
                    x = x0+(x1-x0)*3//4
                    command = ['shell','uitest','uiInput','swipe',str(x),str(y0+(y1-y0)*4//5),str(x),str(y0+(y1-y0)*3//10),'600']
                    call(command)
                    actions.append({'action':'scroll','nodeBounds':[x0,y0,x1,y1],'command':command})
                    scrolls += 1
        time.sleep(1)
    (layout_path.parent / 'launcher-actions.json').write_text(json.dumps(actions, ensure_ascii=False, indent=2)+'\n')
    raise TimeoutError('no unique enabled Demo button after bounded current-layout reveal')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--hdc', default=HDC)
    parser.add_argument('--device', required=True)
    parser.add_argument('--bundle', default='com.minami.harmonyruntimeprobe')
    parser.add_argument('--directory', type=Path, required=True)
    parser.add_argument('--flag', action='append', default=[])
    add_device_file_options(parser)
    args = parser.parse_args()
    try:
        files = AppFiles(args.bundle, args.file_transfer_mode, args.files_root)
        expected_size = parse_native_size(args.expected_native_size) if args.expected_native_size is not None else [1200,800]
    except ValueError as error:
        parser.error(str(error))
    flags = {}
    for option in args.flag:
        key, value = option.split('=', 1)
        if key not in FLAGS or value not in ('true', 'false'):
            parser.error('native flag must be a supported name=true/false')
        flags[key] = value == 'true'
    out = args.directory.resolve()
    out.mkdir(parents=True, exist_ok=True)
    if list(out.iterdir()):
        raise ValueError('preserve existing evidence: use an empty setup directory')
    prefix = [args.hdc, '-t', args.device]

    def call(command):
        result = subprocess.run(prefix + command, capture_output=True, text=True, timeout=25)
        if result.returncode:
            raise RuntimeError(result.stdout + result.stderr)
        return result.stdout

    call(files.command('recv', files.path('config.json'), out / 'config-before.json'))
    before = json.loads((out / 'config-before.json').read_text())
    if before.get('graphics_preset') != 'medium':
        raise ValueError('current user baseline must already select medium')
    config = {**before, **flags}
    config_path = out / 'config-requested.json'
    config_path.write_text(json.dumps(config, ensure_ascii=False, indent=2) + '\n')
    print(call(['shell', 'aa', 'force-stop', args.bundle]).strip(), flush=True)
    call(files.command('send', config_path, files.path('config.json')))
    print(call(['shell', 'aa', 'start', '-a', 'EntryAbility', '-b', args.bundle]).strip(), flush=True)
    remote = '/data/local/tmp/gta-native-trial-setup.json'
    launch_demo(call, out / 'home-layout.json', remote, timeout=30, bundle=args.bundle)
    pid = int(call(['shell', 'pidof', args.bundle]).strip())
    deadline = time.monotonic() + 180
    warmed_since = None
    previous_clock = None
    previous_source = None
    while time.monotonic() < deadline:
        call(files.command('recv', files.path('game-status.json'), out / 'warm-status.json'))
        wrapper = json.loads((out / 'warm-status.json').read_text())
        runtime = json.loads(wrapper['runtimeJson'])
        if runtime.get('processId') == pid:
            if runtime.get('error'):
                raise RuntimeError(runtime['error'])
            if runtime.get('guestPresents', 0) > 0 and runtime.get('renderSize') != expected_size:
                raise ValueError('actual native size differs from expected native size')
            clock = runtime.get('nativeTelemetry', {}).get('monotonicNs') or runtime.get('frameGenerationLive', {}).get('observedMonotonicNs')
            source = runtime.get('guestPresents', 0)
            advancing = (isinstance(clock, int) and previous_clock is not None and clock > previous_clock
                         and previous_source is not None and source > previous_source)
            allowed = wrapper.get('uiAppForeground') and not any(runtime.get(key) for key in
                ('presentationPaused', 'stopRequested', 'saveLoadPending', 'surfaceRecoveryRequired'))
            if allowed and advancing and runtime.get('worldPlayerObserved') and source > 200:
                warmed_since = warmed_since or time.monotonic()
                if time.monotonic() - warmed_since > 25:
                    break
            else:
                warmed_since = None
            previous_clock, previous_source = clock, source
        else:
            warmed_since = None
        time.sleep(2)
    else:
        raise TimeoutError('fresh game world did not warm')
    call(files.command('recv', files.path('config.json'), out / 'config-effective.json'))
    effective = json.loads((out / 'config-effective.json').read_text())
    if effective != config:
        raise ValueError('startup changed expected configuration: inspect effective file')
    result = {'pid': pid, 'flagsChanged': flags, 'renderSize': runtime['renderSize'],
              'graphicsPreset': effective['graphics_preset'], 'worldPlayerObserved': True,
              'scope': 'same graphics settings; Demo spawn/camera and temperature are not fixed by this helper'}
    if files.mode != 'sandbox':
        result['fileTransport'] = files.conditions()
    if args.expected_native_size is not None:
        result['nativeRenderSize'] = expected_size
    (out / 'setup.json').write_text(json.dumps(result, ensure_ascii=False, indent=2)+'\n')
    print(json.dumps(result, ensure_ascii=False), flush=True)


if __name__ == '__main__':
    main()
