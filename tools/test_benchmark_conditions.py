#!/usr/bin/env python3
"""Run the actual ETS monitor with gated taskpool and device API fakes.

This checks asynchronous ownership and units, not real IPC/device readings.
The actual SDK compiler remains responsible for ArkTS and concurrent imports.
"""
from pathlib import Path
import subprocess
import sys
import tempfile

sys.dont_write_bytecode = True
from test_game_quality import typescript_path

ROOT = Path(__file__).resolve().parents[1]
HARNESS = r'''
const fs=require('fs'),vm=require('vm'),assert=require('assert/strict');
const ts=require(process.argv[2]);
const code=ts.transpileModule(fs.readFileSync(process.argv[3],'utf8'),{compilerOptions:{module:ts.ModuleKind.CommonJS,target:ts.ScriptTarget.ES2020}}).outputText;
let now=1000,worker=false,capability=true,thermalValue=3,temp=426,fail='',submitThrows=false;
let executed=0,ipc=0,timerId=0,active=0,maxActive=0,pending=[];
const timers=new Map();
function hardware(name,value){assert(worker,`${name} called outside worker`);ipc++;if(fail===name)throw Error(name);return value;}
const battery={};for(const [name,value] of [['batterySOC',64],['pluggedType',1],['chargingStatus',2]]){
  Object.defineProperty(battery,name,{get(){return hardware(name,value);}});
}
Object.defineProperty(battery,'batteryTemperature',{get(){return hardware('batteryTemperature',temp);}});
const moduleRef={exports:{}};
vm.runInNewContext(code,{module:moduleRef,exports:moduleRef.exports,
  canIUse(){assert(worker,'canIUse outside worker');return capability;},
  setInterval(fn,ms){assert.equal(ms,5000);timers.set(++timerId,fn);return timerId;},
  clearInterval(id){timers.delete(id);},
  require(id){
    if(id==='@kit.ArkTS')return {taskpool:{execute(fn){
      if(submitThrows)throw Error('submit');executed++;active++;maxActive=Math.max(maxActive,active);
      return new Promise((resolve,reject)=>pending.push({fn,resolve,reject}));
    }}};
    if(id==='@kit.BasicServicesKit')return {systemDateTime:{TimeType:{STARTUP:0},getUptime(){return now;}}};
    if(id==='@ohos.thermal')return {default:{getLevel(){return hardware('thermal.getLevel',thermalValue);}}};
    if(id==='@ohos.batteryInfo')return {default:battery};throw Error(id);
  }
});
const Monitor=moduleRef.exports.BenchmarkConditions,enabled=moduleRef.exports.benchmarkMonitorEnabled;
const tick=()=>new Promise(r=>setImmediate(r));
async function flush(){for(let i=0;i<5;i++)await tick();}
async function complete(reject=false){
  const item=pending.shift();assert(item);active--;
  if(reject){item.reject(Error('taskpool failure'));}else{
    worker=true;try{item.resolve(item.fn());}catch(e){item.reject(e);}finally{worker=false;}
  }await flush();
}
function pulse(){for(const fn of [...timers.values()])fn();}
(async()=>{
  for(const text of ['{}','{"native_benchmark_monitor":false}','{"native_benchmark_monitor":1}',
    '{"native_benchmark_monitor":"true"}','null','[]'])assert.equal(enabled(text),false);
  assert.equal(enabled('{"native_benchmark_monitor":true}'),true);
  assert.throws(()=>enabled('{broken'));
  const m=new Monitor();m.setActive(true);pulse();assert.equal(executed,0);assert.equal(timers.size,0);
  assert.equal(m.snapshot(now).enabled,false);assert.equal(m.snapshot(now).sample,null);
  m.configure(true);m.setActive(true);assert.equal(pending.length,1);assert.equal(ipc,0);
  pulse();pulse();assert.equal(executed,1);assert.equal(active,1);
  await complete();let s=m.snapshot(now);assert.equal(s.fresh,true);assert.equal(s.sample.batteryTemperatureC,42.6);
  assert.equal(s.sample.batteryTemperatureDeciC,426);assert.equal(s.sample.batterySOC,64);
  assert.equal(s.sample.thermalLevel,3);assert.equal(s.sample.apiFailures.length,0);
  assert.match(s.temperatureBoundary,/neither is GPU/);now+=15001;assert.equal(m.snapshot(now).fresh,false);

  // A background transition retires the task. An immediate resume cannot
  // launch a second task until the old IPC completes, and discards its result.
  pulse();const before=m.snapshot(now).sample;
  m.setActive(false);assert.equal(timers.size,0);assert.equal(m.snapshot(now).fresh,false);
  m.setActive(true);assert.equal(active,1);await complete();assert.equal(m.snapshot(now).sample,before);
  assert.equal(active,1);assert.equal(pending.length,1);await complete();assert.equal(m.snapshot(now).fresh,true);
  assert.equal(maxActive,1);

  // Every getter fails independently; successful fields retain their value.
  fail='batteryTemperature';pulse();await complete();s=m.snapshot(now);
  assert.equal(s.sample.batteryTemperatureC,null);assert.equal(s.sample.batterySOC,64);
  assert(s.sample.apiFailures.some(x=>x.startsWith('batteryTemperature:')));
  fail='thermal.getLevel';pulse();await complete();s=m.snapshot(now);
  assert.equal(s.sample.thermalLevel,null);assert.equal(s.sample.batterySOC,64);
  fail='';capability=false;const count=ipc;pulse();await complete();s=m.snapshot(now);
  assert.equal(ipc,count);assert.equal(s.sample.batterySOC,null);assert.equal(s.sample.apiFailures.length,2);
  capability=true;thermalValue=7;pulse();await complete();assert.equal(m.snapshot(now).sample.thermalLevel,7);
  thermalValue=99;temp=NaN;pulse();await complete();s=m.snapshot(now);
  assert.equal(s.sample.thermalLevel,null);assert.equal(s.sample.batteryTemperatureC,null);

  // Worker rejection and synchronous task submission rejection are exported;
  // neither poisons single-flight state or prevents later retries.
  pulse();await complete(true);assert.match(m.snapshot(now).sample.apiFailures[0],/^taskpool:/);
  submitThrows=true;pulse();await flush();assert.equal(m.snapshot(now).inFlight,false);
  assert.match(m.snapshot(now).sample.apiFailures[0],/submit/);submitThrows=false;
  thermalValue=1;temp=400;pulse();await complete();assert.equal(m.snapshot(now).sample.batteryTemperatureC,40);

  pulse();const retired=m.snapshot(now).sample;m.stop();assert.equal(timers.size,0);
  await complete();assert.equal(m.snapshot(now).sample,retired);assert.equal(m.snapshot(now).enabled,false);
  pulse();assert.equal(active,0);m.configure(true);m.setActive(true);assert.equal(active,1);
  await complete();assert.equal(m.snapshot(now).fresh,true);m.stop();assert.equal(maxActive,1);
  console.log(JSON.stringify({passed:10,scope:'actual ETS; strict opt-in; worker-only IPC/syscap; 5s single-flight; stale freshness; pause/resume stale result rejection; independent API errors/unknowns/units; rejection retry; stop/restart retirement',boundary:'taskpool and device APIs mocked; no real temperature or device performance acceptance'}));
})().catch(e=>{console.error(e);process.exit(1);});
'''


def main():
    index = (ROOT / 'harmony/entry/src/main/ets/pages/Index.ets').read_text()
    assert index.count('benchmarkMonitorEnabled(') == 1
    assert 'status.uiBenchmarkConditions = this.benchmarkConditions.snapshot(processingNow)' in index
    assert index.count('this.benchmarkConditions.stop()') == 2
    with tempfile.TemporaryDirectory(prefix='gta-benchmark-conditions-') as tmp:
        harness = Path(tmp) / 'test.cjs'
        harness.write_text(HARNESS)
        subprocess.run(['/Applications/DevEco-Studio.app/Contents/tools/node/bin/node', str(harness),
                        str(typescript_path(None)),
                        str(ROOT / 'harmony/entry/src/main/ets/telemetry/BenchmarkConditions.ets')], check=True)


if __name__ == '__main__':
    main()
