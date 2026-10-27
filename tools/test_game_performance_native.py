#!/usr/bin/env python3
"""Test actual ETS scene reporter with controlled API completion and uptime."""
from pathlib import Path
import subprocess
import tempfile
import sys

sys.dont_write_bytecode = True
from test_game_quality import typescript_path

ROOT = Path(__file__).resolve().parents[1]
HARNESS = r'''
const fs=require('fs'),vm=require('vm'),assert=require('assert/strict');
const ts=require(process.argv[2]);
const source=fs.readFileSync(process.argv[3],'utf8')+'\nexport { GamePerformanceLifecycle };';
const result=ts.transpileModule(source,{reportDiagnostics:true,compilerOptions:{module:ts.ModuleKind.CommonJS,target:ts.ScriptTarget.ES2020}});
assert.equal((result.diagnostics||[]).filter(d=>d.category===ts.DiagnosticCategory.Error).length,0);
let now=1000,updates=[],records=[],hold=null;
const plain=x=>JSON.parse(JSON.stringify(x));
const kit={init:async()=>{},async updateGameInfo(value){updates.push(plain(value));if(hold){const wait=hold;hold=null;await wait;}}};
const io={OpenMode:{WRITE_ONLY:1,CREATE:2,APPEND:4,TRUNC:8},openSync(){return {fd:1};},closeSync(){},writeSync(fd,json){records.push(JSON.parse(json));}};
const moduleRef={exports:{}};
vm.runInNewContext(result.outputText,{module:moduleRef,exports:moduleRef.exports,require(id){
  if(id==='@kit.AbilityKit')return {bundleManager:{BundleFlag:{GET_BUNDLE_INFO_WITH_APPLICATION:1},async getBundleInfoForSelf(){return {name:'actual.installed.bundle',versionName:'from-package'};}}};
  if(id==='@kit.CoreFileKit')return {fileIo:io};
  if(id==='../telemetry/GamePerformanceEvidenceWriter')return {GamePerformanceEvidenceWriter:class{enqueue(dir,json){records.push(JSON.parse(json));}}};
  if(id==='@kit.GameServiceKit')return {gamePerformance:kit};
  if(id==='@kit.BasicServicesKit')return {systemDateTime:{TimeType:{STARTUP:1},getUptime(){return now;}}};
  if(id==='@kit.PerformanceAnalysisKit')return {hilog:{error(){},info(){}}};throw Error(id);
}});
const Reporter=moduleRef.exports.GamePerformanceLifecycle;
const tick=()=>new Promise(r=>setImmediate(r));
async function flush(){for(let i=0;i<8;i++)await tick();}
async function fresh(){updates=[];records=[];hold=null;now=1000;const r=new Reporter({filesDir:'/mock-app'});r.onForeground();await flush();r.onGameStarted();await flush();return r;}
function sample(sequence=1,extra={}){return {processId:501,monotonicNs:sequence*1e9,logicTid:12001,submissionTid:12002,engineRenderTid:12003,sourcePresents:120,currentSourceFps:19.75,sourceSampleAgeMs:5,sourceIntervalSamples:120,...extra};}
const cases=[];
(async()=>{
  let r=await fresh();r.onNativeSample(sample(),20,20);await flush();
  let scene=updates.at(-1);assert.equal(scene.sceneID,7);assert.equal(scene.keyThread,'render|12002|logic|12001');
  assert.equal(scene.currentFps,19.75);assert.equal(scene.recommendedFps,20);assert.equal(scene.maxFps,20);assert.ok(scene.keyThread.length<=32);
  assert.equal(records.at(-1).appVersion,'from-package');cases.push('real snapshot fields and installed bundle metadata');
  let count=updates.length;r.onNativeSample(sample());r.onNativeSample(sample(2));await flush();assert.equal(updates.length,count);
  now+=10000;r.onNativeSample(sample(3));await flush();assert.equal(updates.length,count+1);cases.push('unchanged timestamp and 10-second cadence');
  r.onBackground();r.onNativeSample(sample(4));await flush();scene=updates.at(-1);assert.equal(scene.sceneID,0);assert.equal(scene.keyThread,undefined);assert.equal(scene.currentFps,undefined);
  r.onForeground();await flush();count=updates.length;r.onNativeSample(sample(3));await flush();assert.equal(updates.length,count);cases.push('background clears metrics and replay cannot resume data');
  r=await fresh();r.onNativeSample(sample(1,{logicTid:0,submissionTid:-1,currentSourceFps:NaN,sourceSampleAgeMs:6000}),NaN,-1);await flush();scene=updates.at(-1);
  for(const field of ['keyThread','currentFps','recommendedFps','maxFps'])assert.equal(scene[field],undefined);cases.push('invalid and stale values are omitted');
  count=updates.length;now+=10000;r.onNativeSample(sample(2,{processId:502}));await flush();assert.equal(updates.length,count);cases.push('foreign process snapshot rejected');
  r=await fresh();let release;hold=new Promise(resolve=>release=resolve);r.onNativeSample(sample(),20,20);await tick();r.onBackground();release();await flush();
  assert.equal(updates.at(-1).sceneID,0);assert.ok(records.some(row=>row.phase==='retired-success'));cases.push('late API completion cannot reinstate foreground scene');
  r=await fresh();hold=new Promise(resolve=>release=resolve);now+=10000;r.onNativeSample(sample(1),20,20);await tick();
  now+=10000;r.onNativeSample(sample(2),20,20);now+=6000;release();await flush();scene=updates.at(-1);
  assert.equal(scene.keyThread,undefined);assert.equal(scene.currentFps,undefined);cases.push('queued native snapshot expires before dispatch');
  r.onDestroy();await flush();count=updates.length;now+=10000;r.onNativeSample(sample(9),20,20);r.onForeground();await flush();assert.equal(updates.length,count);cases.push('destroyed reporter rejects callbacks');
  r=await fresh();r.onNativeSample(sample(),undefined,undefined);await flush();scene=updates.at(-1);assert.equal(scene.maxFps,undefined);assert.equal(scene.recommendedFps,undefined);cases.push('unknown target and maximum are never invented');
  console.log(JSON.stringify({passed:cases.length,cases,scope:'actual ETS reporter with controlled APIs and uptime; no system scheduling or performance claim'}));
})().catch(error=>{console.error(error);process.exit(1);});
'''

with tempfile.TemporaryDirectory(prefix='gta-performance-native-') as tmp:
    harness = Path(tmp) / 'test.cjs'
    harness.write_text(HARNESS)
    subprocess.run(['/Applications/DevEco-Studio.app/Contents/tools/node/bin/node', str(harness),
                    str(typescript_path(None)), str(ROOT / 'harmony/entry/src/main/ets/entryability/GamePerformance.ets')], check=True)
