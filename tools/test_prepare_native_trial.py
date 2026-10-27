#!/usr/bin/env python3
"""Exercise actual trial setup with mocked HDC and an advancing host clock."""
import contextlib
import io
import json
from pathlib import Path
import sys
import tempfile
import types
import unittest
from unittest.mock import patch

sys.dont_write_bytecode = True
import prepare_native_trial as prepare


class PrepareTrialTests(unittest.TestCase):
    def trial(self, *, frozen_clock=False, frozen_source=False, pause=False, flags=None, direct=False, render_size=(1200,800)):
        clock = [100.0]
        polls = [0]
        commands = []
        before = {'graphics_preset': 'medium', 'spatial_upscale_enabled': True,
                  'frame_interpolation_enabled': True, 'cached_readback': False,
                  'custom_user_option': {'value': 'preserved'}}
        effective = [before]
        layout = {'attributes': {'type': 'Button', 'text': '开始 Demo',
                                'enabled': 'true', 'bounds': '[0,0][10,10]'}}
        layout = {'attributes': {'type':'root','bundleName':'com.minami.harmonyruntimeprobe',
                                 'abilityName':'EntryAbility','pagePath':'pages/Index','visible':'true'},'children':[layout]}

        def run(command, **_):
            commands.append(command)
            stdout = ''
            if 'pidof' in command:
                stdout = '123\n'
            elif 'file' in command and 'send' in command:
                effective[0] = json.loads(Path(command[-2]).read_text())
            elif 'file' in command and 'recv' in command:
                target = Path(command[-1])
                if target.name == 'config-before.json':
                    value = before
                elif target.name == 'config-effective.json':
                    value = effective[0]
                elif target.name == 'home-layout.json':
                    value = layout
                elif target.name == 'warm-status.json':
                    polls[0] += 1
                    value = {'uiAppForeground': True, 'runtimeJson': json.dumps({
                        'processId': 123, 'worldPlayerObserved': True,
                        'guestPresents': 201 if frozen_source else 300 + polls[0] * 20,
                        'renderSize': list(render_size), 'error': '',
                        'presentationPaused': pause and 8 <= polls[0] <= 10,
                        'nativeTelemetry': {'monotonicNs': 1 if frozen_clock else polls[0] * 1_000_000_000}})}
                else:
                    raise AssertionError(command)
                target.write_text(json.dumps(value))
            return types.SimpleNamespace(returncode=0, stdout=stdout, stderr='')

        with tempfile.TemporaryDirectory(prefix='native-prepare-contract-') as directory:
            selected = {'cached_readback': True} if flags is None else flags
            argv = ['prepare_native_trial.py', '--device', 'fake', '--directory', directory]
            remote='/data/app/el2/100/base/com.minami.harmonyruntimeprobe/haps/entry/files/'
            if direct:
                argv.extend(['--file-transfer-mode','direct','--files-root',remote,
                             '--expected-native-size',f'{render_size[0]}x{render_size[1]}'])
            for key, value in selected.items():
                argv.extend(['--flag', key + '=' + str(value).lower()])
            with patch.object(sys, 'argv', argv), patch.object(prepare.subprocess, 'run', run), \
                    patch.object(prepare.time, 'monotonic', lambda: clock[0]), \
                    patch.object(prepare.time, 'sleep', lambda seconds: clock.__setitem__(0, clock[0] + seconds)), \
                    contextlib.redirect_stdout(io.StringIO()):
                if frozen_clock or frozen_source:
                    with self.assertRaisesRegex(TimeoutError, 'fresh game world did not warm'):
                        prepare.main()
                    self.assertFalse((Path(directory) / 'setup.json').exists())
                    return
                prepare.main()
            result = json.loads((Path(directory) / 'setup.json').read_text())
            self.assertTrue(result['worldPlayerObserved'])
            self.assertEqual(result['pid'], 123)
            self.assertEqual(effective[0], {**before, **selected})
            self.assertGreaterEqual(clock[0] - 100, 46 if pause else 28)
            if direct:
                self.assertEqual(result['fileTransport'],{'mode':'direct','filesRoot':remote})
                self.assertEqual(result['nativeRenderSize'],list(render_size))
                for command in commands:
                    if 'file' in command:
                        self.assertNotIn('-b',command)
                self.assertTrue(any('aa' in c and 'start' in c and '-b' in c for c in commands))

    def test_identical_stale_status_cannot_warm_a_trial(self):
        self.trial(frozen_clock=True, frozen_source=True)

    def test_fresh_timestamps_without_new_source_frames_cannot_warm(self):
        self.trial(frozen_source=True)

    def test_advancing_source_and_clock_preserve_graphics_and_custom_config(self):
        self.trial()

    def test_native_pause_restarts_the_continuous_warm_window(self):
        self.trial(pause=True)

    def test_direct_prepare_preserves_config_and_records_exact_native_size(self):
        self.trial(direct=True,render_size=(1714,800))

    def test_pso_comparison_flags_preserve_graphics_and_custom_config(self):
        for enabled in (False, True):
            self.trial(flags={'pso_warmup': enabled, 'pso_warmup_read_only': True})

    def test_reveal_is_bounded_and_requires_ready_and_one_current_valid_scroll(self):
        scroll={'type':'Scroll','scrollable':'true','visible':'true','enabled':'true','bounds':'[10,20][1010,1020]'}
        ready={'type':'Text','text':'资源已就绪','visible':'true'}
        for label,layout,max_swipes in [('ready',{'children':[{'attributes':ready},{'attributes':scroll}]},3),
                                      ('unready',{'attributes':scroll},0),
                                      ('ambiguous',{'children':[{'attributes':ready},{'attributes':scroll},{'attributes':scroll}]},0),
                                      ('bad-bounds',{'children':[{'attributes':ready},{'attributes':{**scroll,'bounds':'[0,0][0,0]'}}]},0)]:
            with self.subTest(label=label),tempfile.TemporaryDirectory() as directory:
                clock=[0];commands=[];dest=Path(directory)/'layout.json'
                def call(command):
                    commands.append(command)
                    if command[:2]==['file','recv']:dest.write_text(json.dumps(layout))
                with patch.object(prepare.time,'monotonic',lambda:clock[0]),patch.object(prepare.time,'sleep',lambda seconds:clock.__setitem__(0,clock[0]+seconds)):
                    with self.assertRaises(TimeoutError):prepare.launch_demo(call,dest,'/data/local/tmp/layout.json',timeout=8)
                swipes=[c for c in commands if c[:4]==['shell','uitest','uiInput','swipe']]
                self.assertEqual(len(swipes),max_swipes)
                self.assertFalse(any(c[:4]==['shell','uitest','uiInput','click'] for c in commands))
                for swipe in swipes:self.assertEqual(swipe[4:8],['760','820','760','320'])

    def test_ready_then_lock_or_other_app_never_scrolls_or_clicks_foreign_tree(self):
        bundle='com.minami.harmonyruntimeprobe'
        attrs={'type':'root','bundleName':bundle,'abilityName':'EntryAbility','pagePath':'pages/Index','visible':'true'}
        scroll={'attributes':{'type':'Scroll','scrollable':'true','visible':'true','enabled':'true','bounds':'[10,20][1010,1020]'}}
        ready={'attributes':{'type':'Text','text':'资源已就绪','visible':'true'}}
        button={'attributes':{'type':'Button','text':'开始 Demo','enabled':'true','visible':'true','bounds':'[20,20][80,80]'}}
        for owner in ({**attrs,'bundleName':'com.ohos.sceneboard'}, {**attrs,'bundleName':'other.app'},
                      {**attrs,'abilityName':'OtherAbility'}, {**attrs,'pagePath':'pages/Other'},
                      {**attrs,'visible':'false'}, {'type':'root'}):
            with self.subTest(owner=owner),tempfile.TemporaryDirectory() as directory:
                clock=[0];reads=[0];commands=[];dest=Path(directory)/'layout.json'
                def call(command):
                    commands.append(command)
                    if command[:2]==['file','recv']:
                        tree={'attributes':attrs,'children':[ready,scroll]} if reads[0]==0 else {'attributes':owner,'children':[ready,scroll,button]}
                        reads[0]+=1;dest.write_text(json.dumps(tree))
                with patch.object(prepare.time,'monotonic',lambda:clock[0]),patch.object(prepare.time,'sleep',lambda s:clock.__setitem__(0,clock[0]+s)):
                    with self.assertRaises(TimeoutError):prepare.launch_demo(call,dest,'/data/local/tmp/layout.json',timeout=5,bundle=bundle)
                # Exactly the first target-launcher swipe is allowed. No stale
                # readiness grants any action after the owner/page changes.
                self.assertEqual(sum(c[:4]==['shell','uitest','uiInput','swipe'] for c in commands),1)
                self.assertFalse(any(c[:4]==['shell','uitest','uiInput','click'] for c in commands))

    def test_launcher_scope_excludes_foreign_sibling_and_nested_nodes(self):
        bundle='com.minami.harmonyruntimeprobe'
        attrs={'type':'root','bundleName':bundle,'abilityName':'EntryAbility','pagePath':'pages/Index'}
        foreign={'attributes':{'type':'root','bundleName':'other.app'},'children':[{'attributes':{'type':'Button','text':'开始 Demo'}}]}
        target={'attributes':attrs,'children':[{'attributes':{'type':'Text','text':'资源已就绪'}},foreign]}
        tree={'children':[target,foreign]}
        self.assertIs(prepare.launcher_subtree(tree,bundle),target)
        self.assertEqual([n.get('text') for n in prepare.launcher_nodes(target,bundle)],[None,'资源已就绪'])
        self.assertIsNone(prepare.launcher_subtree({'children':[target,target]},bundle))
        self.assertIsNone(prepare.launcher_subtree({'attributes':{'type':'root','bundleName':'com.ohos.sceneboard'},'children':[target]},bundle))

    def test_new_native_allowlist_matches_runner_and_analyzer_and_preserves_config(self):
        import run_builtin_benchmark as runner
        import analyze_native_benchmark as analyzer
        self.assertEqual(prepare.FLAGS, set(runner.FLAGS))
        self.assertEqual(prepare.FLAGS, set(analyzer.NATIVE_BOOLEAN_FLAGS))
        self.assertNotIn('render_prepass_diagnostics', prepare.FLAGS)
        self.trial(flags={'inline_upload_borrow': True, 'native_command_fastpath': False,
                          'skip_empty_init_transition': True, 'pipeline_cache_read_only': False,
                          'skip_empty_render_prepass': True})


if __name__ == '__main__':
    unittest.main()
