#!/usr/bin/env python3
"""Measurement rejection and source/AI/repeat separation contracts."""
import copy
import unittest
from collect_native_trial import summarize, presentation_summary, read_runtime
import json
import contextlib
import io
from pathlib import Path
import tempfile
import types
from unittest.mock import patch
import collect_native_trial as collector

class MeasurementTests(unittest.TestCase):
    def test_explicit_transport_and_native_size_validation_preserve_legacy_defaults(self):
        old=collector.AppFiles('bundle')
        self.assertEqual(old.command('recv',old.path('config.json'),'/tmp/out'),
                         ['file','recv','-b','bundle',collector.FILES+'config.json','/tmp/out'])
        root='/data/app/el2/100/base/bundle/haps/entry/files/'
        direct=collector.AppFiles('bundle','direct',root)
        self.assertEqual(direct.command('send','/tmp/config',direct.path('config.json')),
                         ['file','send','/tmp/config',root+'config.json'])
        self.assertEqual(collector.parse_native_size('1714x800'),[1714,800])
        for bad in ('1713x800','1714x802','0x800','true','1714.0x800','9000x800'):
            with self.subTest(size=bad),self.assertRaises(ValueError):collector.parse_native_size(bad)
        for bad in (None,'/','/data/app/el2/100/base/other/haps/entry/files/',
                    '/data/app/el2/100/base/bundle/../bundle/haps/entry/files/'):
            with self.subTest(root=bad),self.assertRaises(ValueError):collector.AppFiles('bundle','direct',bad)

    def test_actual_collector_direct_transfer_binds_exact_texture_and_preserves_config(self):
        with tempfile.TemporaryDirectory() as temp:
            root='/data/app/el2/100/base/bundle/haps/entry/files/'
            args=types.SimpleNamespace(directory=Path(temp),device='new-device',hdc='fake',bundle='bundle',
                samples=3,interval=.5,file_transfer_mode='direct',files_root=root,expected_native_size='1714x800')
            clock=[100.0];commands=[];config={'graphics_preset':'medium','frame_interpolation_enabled':True,'custom':{'keep':True}}
            def run(command,**kwargs):
                commands.append(command)
                if 'pidof' in command:return types.SimpleNamespace(returncode=0,stdout='42',stderr='')
                remote,target=command[-2],Path(command[-1]);self.assertTrue(remote.startswith(root));self.assertNotIn('-b',command)
                mono=int(clock[0]*1e9);count=int(clock[0]*20)
                if remote.endswith('config.json'):value=config
                elif remote.endswith('game-status.json'):
                    value={'uiAppForeground':True,'runtimeJson':json.dumps({'worldPlayerObserved':True,'error':'',
                        'guestPresents':count,'presents':count*2,'renderSize':[1714,800],
                        'frameGenerationLive':{'processId':42,'observedMonotonicNs':mono}})}
                else:value={'pid':42,'epoch':1,'statusSnapshotGeneratedMonotonicNs':mono,'guestFinalSource':{'width':1714,'height':800},
                    'sourcePresents':count,'aiPresents':count,'repeatPresents':0,'missedDeadlineSlots':0,'surfaceAcquireFailures':0}
                target.write_text(json.dumps(value));return types.SimpleNamespace(returncode=0,stdout='',stderr='')
            with patch.object(collector.subprocess,'run',run),patch.object(collector.time,'monotonic',lambda:clock[0]), \
                    patch.object(collector.time,'sleep',lambda seconds:clock.__setitem__(0,clock[0]+seconds)),contextlib.redirect_stdout(io.StringIO()):
                collector.collect(args)
            summary=json.loads((Path(temp)/'summary.json').read_text())
            self.assertEqual(summary['fileTransport'],{'mode':'direct','filesRoot':root})
            self.assertEqual(summary['nativeRenderSize'],[1714,800]);self.assertEqual(summary['guestSourceFps'],20)
            self.assertEqual(json.loads((Path(temp)/'config-after.json').read_text()),config)

    def test_source_frames_never_use_combined_presents(self):
        a={'pid':2,'monotonicNs':1000000000,'renderSize':[1200,800],'guestPresents':10,'presents':99}
        b={**a,'monotonicNs':3000000000,'guestPresents':30,'presents':179}
        result=summarize([a,b])
        self.assertEqual(result['guestSourceFps'],10)
        self.assertEqual(result['nativePresentCallsPerSecond'],40)
        for patch in ({'pid':3},{'monotonicNs':a['monotonicNs']},{'renderSize':[1202,800]},{'guestPresents':9}):
            with self.assertRaises(ValueError):summarize([a,{**b,**patch}])

    def test_overlapping_trace_rows_are_counted_once_and_repeats_excluded(self):
        a={'pid':2,'epoch':1,'statusSnapshotGeneratedMonotonicNs':1000000000,
           'sourcePresents':1,'aiPresents':1,'repeatPresents':0,'missedDeadlineSlots':0,'surfaceAcquireFailures':0}
        rows=[{'slot':i,'kind':kind,'nativePresentCalled':True,'presentBeginMonotonicNs':1000000000+i*100000000}
              for i,kind in ((1,'source'),(2,'ai'),(3,'repeat'),(4,'source'),(5,'source'))]
        a['recentPresents']=rows[:4]
        b={**a,'statusSnapshotGeneratedMonotonicNs':2000000000,'sourcePresents':4,'aiPresents':2,'repeatPresents':1,'recentPresents':rows[1:]}
        result=presentation_summary([a,b])
        self.assertEqual(result['sourceAndAiPresentCallsPerSecond'],4)
        self.assertEqual(result['sourcePresentInterval']['samples'],2)
        self.assertEqual(result['sourcePresentInterval']['p95Ms'],300)
        self.assertTrue(result['sourceTraceCoverage']['complete'])
        missing=copy.deepcopy(b);missing['sourcePresents']=40
        incomplete=presentation_summary([a,missing])
        self.assertFalse(incomplete['sourceTraceCoverage']['complete'])
        self.assertIsNone(incomplete['sourcePresentInterval']['p99Ms'])
        with self.assertRaises(ValueError):presentation_summary([a,{**b,'epoch':2}])
        with self.assertRaises(ValueError):presentation_summary([a,{**b,'statusSnapshotGeneratedMonotonicNs':a['statusSnapshotGeneratedMonotonicNs']}])

    def test_native_pause_and_stopping_override_historical_world_marker(self):
        runtime={'worldPlayerObserved':True,'error':''}
        read_runtime({'runtimeJson':json.dumps(runtime),'uiAppForeground':True})
        for key in ('presentationPaused','stopRequested','saveLoadPending','surfaceRecoveryRequired'):
            with self.assertRaises(ValueError):
                read_runtime({'runtimeJson':json.dumps({**runtime,key:True}),'uiAppForeground':True})

if __name__=='__main__':unittest.main()
