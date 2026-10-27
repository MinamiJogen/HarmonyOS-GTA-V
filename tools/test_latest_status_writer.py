#!/usr/bin/env python3
"""Exercise actual ETS asynchronous exporter with controllable I/O failures."""
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
const code=ts.transpileModule(fs.readFileSync(process.argv[3],'utf8'),{compilerOptions:{module:ts.ModuleKind.CommonJS,target:ts.ScriptTarget.ES2020}}).outputText;
let gate,wait,renameGate,renameWait,renameEntered=0,fdSequence=0,fail='',active=0,maxActive=0,published=[],operations=[];
const paths=new Map(),contents=new Map();
const io={OpenMode:{WRITE_ONLY:1,CREATE:2,TRUNC:4},
  async open(path){active++;maxActive=Math.max(maxActive,active);operations.push(['open',path]);if(wait){await wait;wait=null;}const fd=++fdSequence;paths.set(fd,path);return {fd};},
  async write(fd,buffer){if(fail==='write')throw Error('write');contents.set(paths.get(fd),Buffer.from(buffer).toString('utf8'));return fail==='short'?1:buffer.byteLength;},
  async close(){active--;},async rename(path){renameEntered++;if(renameWait){const waiting=renameWait;renameWait=null;await waiting;}if(fail==='rename')throw Error('rename');published.push(contents.get(path));}};
const moduleRef={exports:{}};
vm.runInNewContext(code,{module:moduleRef,exports:moduleRef.exports,require(id){
  if(id==='@kit.CoreFileKit')return {fileIo:io};
  if(id==='@kit.ArkTS')return {util:{TextEncoder:class{encodeInto(s){return new Uint8Array(Buffer.from(s));}}}};
  if(id==='@kit.PerformanceAnalysisKit')return {hilog:{error(){}}};throw Error(id);
}});
const Writer=moduleRef.exports.LatestStatusWriter;
const tick=()=>new Promise(r=>setImmediate(r));
async function flush(){for(let i=0;i<8;i++)await tick();}
(async()=>{
  let w=new Writer();wait=new Promise(r=>gate=r);
  w.enqueue('/app','first');w.enqueue('/app','second');w.enqueue('/app','最新');
  await tick();assert.equal(active,1);gate();await flush();
  assert.deepEqual(published,['first','最新']);assert.equal(maxActive,1);
  for(const mode of ['write','short','rename']){
    fail=mode;w.enqueue('/app','failed');await flush();assert.equal(published.at(-1),'最新');assert.equal(active,0);
    fail='';w.enqueue('/app','recovered');await flush();assert.equal(published.at(-1),'recovered');
    w.enqueue('/app','最新');await flush();
  }
  wait=new Promise(r=>gate=r);w.enqueue('/app','retired');w.enqueue('/app','pending');await tick();w.stop();gate();await flush();
  assert.equal(active,0);assert.equal(published.at(-1),'最新');w.enqueue('/app','after-stop');await flush();assert.equal(published.at(-1),'最新');
  assert.equal(maxActive,1);
  // Retire an instance only after its OS rename has started. Its successor can
  // write its own temporary file immediately, but must publish after that old
  // rename completes. Without the cross-instance chain the old file wins.
  let retired=new Writer();renameWait=new Promise(r=>renameGate=r);
  const before=renameEntered;retired.enqueue('/app','old-before-retirement');
  while(renameEntered===before)await tick();retired.stop();
  let successor=new Writer();successor.enqueue('/app','current-owner');await flush();
  assert.equal(renameEntered,before+1,'successor rename raced retired publication');
  assert.equal(published.at(-1),'最新');renameGate();await flush();
  assert.deepEqual(published.slice(-2),['old-before-retirement','current-owner']);
  assert.notEqual(operations.at(-1)[1],operations.at(-2)[1],'instances must use distinct temporary files');
  assert.equal(active,0);

  // stop() also prevents a queued-but-not-yet-issued publication, and a failed
  // shared commit cannot poison the chain for later status snapshots.
  let blocker=new Writer();renameWait=new Promise(r=>renameGate=r);
  const started=renameEntered;blocker.enqueue('/app','blocker');
  while(renameEntered===started)await tick();
  let queued=new Writer();queued.enqueue('/app','queued-retired');await flush();queued.stop();
  renameGate();await flush();assert.equal(published.at(-1),'blocker');
  assert(!published.includes('queued-retired'));
  fail='rename';successor.enqueue('/app','shared-tail-failed');await flush();fail='';
  let afterFailure=new Writer();afterFailure.enqueue('/app','new-after-shared-failure');await flush();
  assert.equal(published.at(-1),'new-after-shared-failure');assert.equal(active,0);
  console.log(JSON.stringify({passed:11,scope:'actual ETS writer; slow I/O coalescing, bounded concurrency, UTF-8, short write/failure retention, close, lifecycle retirement, cross-instance delayed rename ordering and failure recovery; no device performance claim'}));
})().catch(e=>{console.error(e);process.exit(1);});
'''

with tempfile.TemporaryDirectory(prefix='gta-status-writer-') as tmp:
    harness = Path(tmp) / 'test.cjs'
    harness.write_text(HARNESS)
    subprocess.run(['/Applications/DevEco-Studio.app/Contents/tools/node/bin/node', str(harness),
                    str(typescript_path(None)), str(ROOT / 'harmony/entry/src/main/ets/telemetry/LatestStatusWriter.ets')], check=True)
