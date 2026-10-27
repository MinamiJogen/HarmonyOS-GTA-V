#!/usr/bin/env python3
"""Slow/failing I/O tests against the actual bounded asynchronous ETS writer."""
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
let held=null,release,mode='',active=0,peak=0,serial=0,files=new Map(),published=[],journal=[],errors=[],removed=[];
const io={OpenMode:{WRITE_ONLY:1,CREATE:2,TRUNC:4,APPEND:8},
  async open(path){active++;peak=Math.max(peak,active);if(held){const wait=held;held=null;await wait;}const fd=++serial;files.set(fd,path);return {fd};},
  async write(fd,buffer){if(mode==='write')throw Error('injected write');const text=Buffer.from(buffer).toString('utf8');
    if(mode==='short')return 1;files.set(files.get(fd),text);if(files.get(fd).endsWith('.jsonl'))journal.push(text);return buffer.byteLength;},
  async close(){active--;},async rename(from,to){if(mode==='rename')throw Error('injected rename');published.push(files.get(from));},
  async unlink(path){removed.push(path);}};
const moduleRef={exports:{}};
vm.runInNewContext(code,{module:moduleRef,exports:moduleRef.exports,require(id){
  if(id==='@kit.CoreFileKit')return {fileIo:io};
  if(id==='@kit.ArkTS')return {util:{TextEncoder:class{encodeInto(s){return new Uint8Array(Buffer.from(s));}}}};
  if(id==='@kit.PerformanceAnalysisKit')return {hilog:{error(...args){errors.push(args);}}};throw Error(id);
}});
const Writer=moduleRef.exports.GamePerformanceEvidenceWriter;
const tick=()=>new Promise(r=>setImmediate(r));async function flush(){for(let i=0;i<12;i++)await tick();}
(async()=>{
  const writer=new Writer();held=new Promise(resolve=>release=resolve);writer.enqueue('/app','{"event":"first"}');
  for(let n=0;n<40;n++)writer.enqueue('/app',JSON.stringify({event:'queued',n}));
  await tick();assert.equal(active,1);assert.equal(writer.pending.length,32);assert.equal(writer.dropped,8);release();await flush();
  assert.equal(published.length,33);assert.equal(journal.length,33);assert.equal(peak,1);assert.equal(active,0);
  writer.enqueue('/app','{"event":"中文"}');await flush();assert.equal(published.at(-1),'{"event":"中文"}');
  for(const fault of ['write','short','rename']){mode=fault;writer.enqueue('/app','{"event":"failed"}');await flush();
    assert.equal(published.at(-1),'{"event":"中文"}');assert.equal(active,0);mode='';writer.enqueue('/app','{"event":"中文"}');await flush();}
  assert.ok(removed.length>0);assert.ok(errors.length>=11);
  console.log(JSON.stringify({passed:7,scope:'actual ETS writer: bounded backlog, single I/O, UTF-8, short/failed writes and rename preserve last status, handle close, recovery; no device timing claim'}));
})().catch(error=>{console.error(error);process.exit(1);});
'''

with tempfile.TemporaryDirectory(prefix='gta-performance-writer-') as tmp:
    harness = Path(tmp) / 'test.cjs'
    harness.write_text(HARNESS)
    subprocess.run(['/Applications/DevEco-Studio.app/Contents/tools/node/bin/node', str(harness),
                    str(typescript_path(None)), str(ROOT / 'harmony/entry/src/main/ets/telemetry/GamePerformanceEvidenceWriter.ets')], check=True)
