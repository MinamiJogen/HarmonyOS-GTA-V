#!/usr/bin/env python3
"""Independent executable contracts for discrete touch buttons and native ownership.

Source game-runtime/scripts/env.sh first for the offline Rust tests. This does
not operate a device or establish driving/mission effects. Production TS and
Rust modules are executed; no control algorithm is copied into the tests.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tempfile

sys.dont_write_bytecode = True
from test_game_quality import typescript_path

ROOT = Path(__file__).resolve().parents[1]
CONTROLS = ROOT / 'harmony/entry/src/main/ets/controls'

NODE_TESTS = r'''
const fs=require('fs'),path=require('path'),vm=require('vm'),assert=require('assert/strict');
const ts=require(process.argv[2]),base=process.argv[3];
require.extensions['.ts']=(module,filename)=>module._compile(ts.transpileModule(fs.readFileSync(filename,'utf8'),{compilerOptions:{module:ts.ModuleKind.CommonJS,target:ts.ScriptTarget.ES2020}}).outputText,filename);
const {TouchInputController,TouchButtonTracker,touchEventPoints,clampTouch}=require(path.join(base,'TouchInput.ts'));
const {directionButtons}=require(path.join(base,'DirectionButtons.ts'));
const {rawTouchBinding}=require(path.join(base,'RawTouchBindings.ts'));
const {TouchStateResolver}=require(path.join(base,'TouchState.ts'));
const {touchProfile}=require(path.join(base,'TouchLayout.ts'));
const {TASK_WIDGETS}=require(path.join(base,'TaskPadLayout.ts'));
const {isDirectTouchAction}=require(path.join(base,'TouchBindingPolicy.ts'));
const cases=[];function test(name,fn){fn();cases.push(name);}
function setup(reject=false){const keys=[],mouse=[],controls=[],wheel=[],look=[];const c=new TouchInputController({control:(...v)=>{controls.push(v);return !reject;},rawKey:(...v)=>{keys.push(v);return !reject;},mouse:(...v)=>{mouse.push(v);return !reject;},wheel:n=>wheel.push(n),look:(...v)=>look.push(v)});return {c,keys,mouse,controls,wheel,look};}
function observed(value,g=3,t=1000){return {known:true,value,source:'independent-fixture',generation:g,sampledAtMonoMs:t};}
function snapshot(seq=1,extra={}){return {generation:3,seq,sampledAtMonoMs:1000,nowMonoMs:1000,playerValid:observed(true),ui:observed('gameplay'),uiOwnsInput:observed(false),transport:observed('foot'),weaponClass:observed('unarmed'),...extra};}
function resolved(seq=1,extra={}){const r=new TouchStateResolver();r.accept(JSON.stringify(snapshot(seq,extra)),5000);r.resolve(5000);r.accept(JSON.stringify(snapshot(seq+1,extra)),5000);return [r,r.resolve(5000)];}
function fromProfile(p){return directionButtons(p.axisX,p.axisY,p.directionControls);}
test('foot and driver buttons execute exact raw WASD and AD without semantic injection',()=>{
 const foot=fromProfile(touchProfile('FOOT_UNARMED')),car=fromProfile(touchProfile('VEHICLE_CAR'));
 assert.deepEqual(foot.map(b=>[b.id,b.mapping.keys[0]]),[['left','KeyA'],['right','KeyD'],['up','KeyW'],['down','KeyS']]);
 assert.deepEqual(car.map(b=>[b.id,b.mapping.keys[0]]),[['left','KeyA'],['right','KeyD']]);
 const {c,keys,controls}=setup();for(const b of foot)c.pressMapped('direction:'+b.id,b.binding,b.mapping);
 assert.deepEqual(keys.map(v=>v.slice(0,2)),[['KeyA',true],['KeyD',true],['KeyW',true],['KeyS',true]]);assert.equal(controls.length,0);c.releaseAll();assert.equal(keys.length,8);
});
test('simultaneous forward left and run remain separate owners through independent release',()=>{
 const {c,keys}=setup();c.pressMapped('forward','up',rawTouchBinding('INPUT_MOVE_UP_ONLY'));c.pressMapped('left','left',rawTouchBinding('INPUT_MOVE_LEFT_ONLY'));c.pressRaw('run','run','ShiftLeft');
 c.release('left');assert(c.held('up'));assert(c.held('run'));assert(!c.held('left'));c.release('forward');assert(c.held('run'));c.releaseAll();
 for(const key of ['KeyW','KeyA','ShiftLeft']){const entries=keys.filter(v=>v[0]===key);assert.deepEqual(entries.map(v=>v[1]),[true,false]);assert.equal(entries[0][2],entries[1][2]);}
});
test('two fingers on same key release with their own exact native source',()=>{
 const {c,keys}=setup();const map=rawTouchBinding('INPUT_MOVE_UP_ONLY');c.pressMapped('finger:0','up',map);c.pressMapped('finger:1','up',map);c.release('finger:0');assert(c.held('up'));c.release('finger:1');assert(!c.held('up'));
 assert.notEqual(keys[0][2],keys[1][2]);assert.equal(keys[0][2],keys[2][2]);assert.equal(keys[1][2],keys[3][2]);
});
test('all direction mappings preserve holds across unchanged binding refresh',()=>{
 const {c,keys}=setup();const buttons=fromProfile(touchProfile('FOOT_UNARMED'));for(const b of buttons)c.pressMapped(b.id,b.binding,b.mapping);
 c.retainBindings(buttons.map(b=>b.binding));assert.equal(keys.length,4);c.retainBindings([]);assert.equal(keys.length,8);assert(!buttons.some(b=>c.held(b.binding)));
});
test('tracker protects active other fingers and repairs Up with empty changed list',()=>{
 const tracker=new TouchButtonTracker();const a=tracker.begin('up',0,0,100),b=tracker.begin('left',1,0,101);assert(a&&b);assert.equal(tracker.begin('right',0,0,102),undefined);
 assert.deepEqual(tracker.end('up',true,false,[{id:1}],[{id:0}],103),[]);assert(tracker.current('up',0,0));
 assert.deepEqual(tracker.end('up',true,false,[],[{id:1}],104).map(c=>c.id),[0]);assert(tracker.current('left',1,0));
 assert.deepEqual(tracker.end('left',false,true,[],[],105).map(c=>c.id),[1]);assert.equal(tracker.contains(1),false);
});
test('tracker ignores delayed old Up, invalid epoch Move and duplicate Down',()=>{
 const tracker=new TouchButtonTracker();const old=tracker.begin('up',0,0,100);assert(old);tracker.reset();const current=tracker.begin('up',0,1,200);assert(current);
 assert.equal(tracker.current('up',0,0),undefined);assert.equal(tracker.begin('up',0,1,201),undefined);assert.deepEqual(tracker.end('up',true,false,[{id:0}],[],150),[]);assert(tracker.current('up',0,1));
 assert.deepEqual(tracker.end('up',true,false,[{id:0}],[],202).map(c=>c.epoch),[1]);
});
test('TaskPad has eight discrete axis buttons and nineteen SCRIPT-only channels',()=>{
 const axes=TASK_WIDGETS.filter(w=>w.kind==='axis_button');assert.equal(axes.length,8);assert(!TASK_WIDGETS.some(w=>w.kind==='stick'));
 const channels=new Set(TASK_WIDGETS.flatMap(w=>w.channels));assert.equal(channels.size,19);for(const ch of channels){assert(ch.startsWith('INPUT_SCRIPT_'));assert.equal(rawTouchBinding(ch),undefined);}
 for(const ch of ['INPUT_SCRIPT_LEFT_AXIS_X','INPUT_SCRIPT_LEFT_AXIS_Y','INPUT_SCRIPT_RIGHT_AXIS_X','INPUT_SCRIPT_RIGHT_AXIS_Y'])assert.deepEqual(axes.filter(w=>w.channels[0]===ch).map(w=>w.value).sort(),[-1,1]);
});
test('TaskPad opposite axes aggregate zero then recover remaining owner in group2',()=>{
 const {c,controls,keys}=setup();c.setControl('l','script:left','INPUT_SCRIPT_LEFT_AXIS_X',-1,2);c.setControl('r','script:right','INPUT_SCRIPT_LEFT_AXIS_X',1,2);c.release('l');c.release('r');
 assert.deepEqual(controls,[['INPUT_SCRIPT_LEFT_AXIS_X',-1,2],['INPUT_SCRIPT_LEFT_AXIS_X',0,2],['INPUT_SCRIPT_LEFT_AXIS_X',1,2],['INPUT_SCRIPT_LEFT_AXIS_X',0,2]]);assert.equal(keys.length,0);
});
test('rejected SCRIPT input cannot fall back to raw or remain logically held',()=>{
 const {c,controls,keys,mouse}=setup(true);assert.equal(c.setControl('script','script','INPUT_SCRIPT_LEFT_AXIS_X',1,2),false);assert(!c.held('script'));c.releaseAll();assert.equal(controls.length,1);assert.equal(keys.length,0);assert.equal(mouse.length,0);
});
test('UNKNOWN and stale snapshots cannot create automatic WASD or driver buttons',()=>{
 const r=new TouchStateResolver();const empty=r.resolve(0);assert.equal(empty.profile.id,'UNKNOWN');assert.deepEqual(directionButtons(empty.axisX,empty.axisY,empty.directions),[]);
 const [fresh,foot]=resolved();assert.equal(foot.profile.id,'FOOT_UNARMED');assert.equal(directionButtons(foot.axisX,foot.axisY,foot.directions).length,4);const stale=fresh.resolve(5501);assert.equal(stale.profile.id,'UNKNOWN');assert.equal(directionButtons(stale.axisX,stale.axisY,stale.directions).length,0);
 const [,unknown]=resolved(1,{transport:{known:false},driverSeat:observed(true)});assert.equal(unknown.profile.id,'UNKNOWN');
});
test('fresh driver requires real car observations and manual layout is not vehicle evidence',()=>{
 const [,car]=resolved(1,{transport:observed('car'),inVehicle:observed(true),driverSeat:observed(true)});assert.equal(car.profile.id,'VEHICLE_CAR');assert.equal(car.manual,false);
 const r=new TouchStateResolver();r.manualProfile='VEHICLE_CAR';const manual=r.resolve(0);assert.equal(manual.profile.id,'VEHICLE_CAR');assert.equal(manual.manual,true);assert.equal(manual.transport,'');
 const [,pause]=resolved(1,{transport:observed('car'),inVehicle:observed(true),driverSeat:observed(true),ui:observed('pause'),uiOwnsInput:observed(true)});assert.equal(pause.profile.id,'GAME_PAUSE');assert.notEqual(pause.profile.id,'VEHICLE_CAR');
});
test('missing mapping remains unavailable rather than inventing input',()=>{
 const unknown=directionButtons('UNRECOGNIZED_AXIS','',[]);assert.deepEqual(unknown,[]);
 const [,car]=resolved(1,{transport:observed('car'),inVehicle:observed(true),driverSeat:{known:false}});assert.equal(car.profile.id,'UNKNOWN');
});
// Extract only actual input/state methods. ArkUI builders are not replaced by
// simulated handlers: the same production method bodies execute below.
const ets=fs.readFileSync(path.join(base,'GameTouchControls.ets'),'utf8');
function method(name){const start=ets.indexOf('  private '+name+'(');assert(start>=0,name);const open=ets.indexOf('{',start);let depth=1,end=open+1;for(;end<ets.length&&depth;end++){if(ets[end]==='{')depth++;if(ets[end]==='}')depth--;}assert.equal(depth,0);return ets.slice(start,end);}
let mockNow=5000,semanticReleases=0,focusCalls=0;
const context={TouchType:{Down:0,Up:1,Move:2,Cancel:3},Date:{now:()=>mockNow},touchEventPoints,clampTouch,directionButtons,TASK_WIDGETS,isDirectTouchAction,
 game:{setInputFocus:()=>focusCalls++,releaseSemanticInputs:()=>semanticReleases++},AppStorage:{setOrCreate:()=>{}}};
const names=['endButtonContacts','directionTouch','directionKeys','taskTouch','releaseTouches','updatePressed','findPointer','eventPoints','canPlay','refresh','stateFamily','sameActions','binding'];
const code=ts.transpileModule('class ActualControls {'+names.map(method).join('\n')+'}',{compilerOptions:{target:ts.ScriptTarget.ES2020}}).outputText;
const ActualControls=vm.runInNewContext(code+'; ActualControls;',context);
function harness(task=false,reject=false){const s=setup(reject),h=new ActualControls();Object.assign(h,{controller:s.c,buttonTracker:new TouchButtonTracker(),active:true,visible:true,inputEnabled:true,taskVisible:task,drawerVisible:false,pointers:[],lookPointer:-1,lookSensitivity:1,pressed:[],actions:[],runActive:false,axisX:'INPUT_MOVE_LR',axisY:'INPUT_MOVE_UD',directions:[],profileId:'FOOT_UNARMED',snapshotGeneration:3,lastFresh:true,lastTransport:'foot',manualId:'',pinnedDetonate:false,statusPublished:false,traceTouch:()=>{},applySettingsFamily:()=>{},onStatusChanged:()=>{}});return {...s,h};}
function event(type,changed=[],active=[],timestamp=100){return {type,changedTouches:changed.map(id=>({id,x:10,y:10})),touches:active.map(id=>({id,x:10,y:10})),timestamp,stopPropagation(){}};}
const footButtons=fromProfile(touchProfile('FOOT_UNARMED')),up=footButtons.find(b=>b.id==='up'),left=footButtons.find(b=>b.id==='left');
test('actual directionTouch executes W+A and missing-Up without releasing active other button',()=>{
 const {h,c,keys}=harness();h.directionTouch(up,event(0,[0],[0],100));h.directionTouch(left,event(0,[1],[0,1],101));assert(c.held(up.binding)&&c.held(left.binding));
 h.directionTouch(left,event(1,[1],[0],102));assert(c.held(up.binding));assert(!c.held(left.binding));h.directionTouch(up,event(1,[],[],103));assert(!c.held(up.binding));assert.equal(keys.length,4);
});
test('actual directionTouch ignores Move-only and duplicate Down and handles target Cancel',()=>{
 const {h,c,keys}=harness();h.directionTouch(up,event(2,[],[0],100));assert.equal(keys.length,0);h.directionTouch(up,event(0,[0],[0],101));h.directionTouch(up,event(0,[0],[0],102));assert.equal(keys.length,1);
 h.directionTouch(left,event(0,[1],[0,1],103));h.directionTouch(up,event(3,[],[1],104));assert(!c.held(up.binding));assert(c.held(left.binding));h.releaseTouches();assert(!c.held(left.binding));
});
test('actual taskTouch uses declared group2 axis even when supplied widget channels are spoofed',()=>{
 const {h,c,keys,controls}=harness(true);const axis=TASK_WIDGETS.find(w=>w.id==='left_axis_left');h.taskTouch({...axis,channels:['INPUT_ATTACK']},event(0,[0],[0],100));assert.deepEqual(controls[0],['INPUT_SCRIPT_LEFT_AXIS_X',-1,2]);assert.equal(keys.length,0);
 h.taskTouch(axis,event(3,[],[],101));assert(!c.held('task:'+axis.id));h.taskTouch({id:'forged',channels:['INPUT_SCRIPT_RDOWN']},event(0,[1],[1],102));assert.equal(controls.length,2);
});
test('actual taskTouch opposites recover other owner on Up and Move never creates a hold',()=>{
 const {h,controls,keys}=harness(true);const a=TASK_WIDGETS.find(w=>w.id==='left_axis_left'),b=TASK_WIDGETS.find(w=>w.id==='left_axis_right');h.taskTouch(a,event(2,[],[0],100));assert.equal(controls.length,0);
 h.taskTouch(a,event(0,[0],[0],101));h.taskTouch(b,event(0,[1],[0,1],102));h.taskTouch(a,event(1,[],[1],103));h.taskTouch(b,event(3,[],[],104));assert.deepEqual(controls.map(v=>v[1]),[-1,0,1,0]);assert(controls.every(v=>v[2]===2));assert.equal(keys.length,0);
});
test('actual rejected taskTouch has no raw fallback and global release clears contact and queued holds',()=>{
 const {h,c,keys,controls}=harness(true,true);const a=TASK_WIDGETS.find(w=>w.id==='left_axis_left');h.taskTouch(a,event(0,[0],[0],100));assert(!c.held('task:'+a.id));assert.equal(keys.length,0);assert.equal(controls.length,1);
 const before=semanticReleases;h.runActive=true;h.lookPointer=4;h.releaseTouches();assert.equal(semanticReleases,before+1);assert.equal(h.lookPointer,-1);assert.equal(h.runActive,false);assert.equal(h.buttonTracker.contains(0),false);h.taskTouch(a,event(2,[],[0],101));assert.equal(controls.length,1);
});
test('actual refresh preserves held directions on same family and releases them on UNKNOWN expiry',()=>{
 const {h,c,keys}=harness();const [r]=resolved();h.resolver=r;mockNow=5000;h.directionTouch(up,event(0,[0],[0],100));h.refresh();assert(c.held(up.binding));assert.equal(keys.length,1);
 mockNow=5501;h.refresh();assert.equal(h.profileId,'UNKNOWN');assert(!c.held(up.binding));assert.equal(h.buttonTracker.contains(0),false);assert.equal(keys.length,2);h.directionTouch(up,event(2,[],[0],101));assert.equal(keys.length,2);mockNow=5000;
});
test('turret planner exposes only declared mouse axes and rejects unknown axis plans',()=>{
 const buttons=directionButtons('INPUT_VEH_GUN_LR','INPUT_VEH_GUN_UD',[]);assert.deepEqual(buttons.map(b=>[b.id,b.mouseAxis,b.mouseSign]),[['left','x',-1],['right','x',1],['up','y',-1],['down','y',1]]);assert(buttons.every(b=>b.mapping===undefined));assert.deepEqual(directionButtons('not_known','also_unknown',[]),[]);
});
test('mouse direction repeat is bounded without delayed-tick catchup or multiple-finger speedup',()=>{
 const {c,look}=setup();assert(c.pressLookDirection('left','left','x',-1,6,0));assert.equal(look.length,1);c.tick(99);assert.equal(look.length,1);c.tick(100);assert.equal(look.length,2);c.tick(1000);assert.equal(look.length,3);
 c.pressLookDirection('left2','left','x',-1,6,1000);c.tick(1100);assert.deepEqual(look.at(-1),[0,0,-6,0,true]);c.release('left');c.tick(1200);assert.deepEqual(look.at(-1),[0,0,-6,0,true]);c.releaseAll();const before=look.length;c.tick(10000);assert.equal(look.length,before);assert.equal(c.pressLookDirection('bad','bad','z',1,6,0),false);assert.equal(c.pressLookDirection('bad','bad','x',0,6,0),false);
});
test('opposite mouse directions cancel repeat and release restores the remaining owner',()=>{
 const {c,look}=setup();c.pressLookDirection('l','left','x',-1,6,0);c.pressLookDirection('r','right','x',1,6,0);c.tick(100);assert.equal(look.length,1);c.release('l');c.tick(200);assert.deepEqual(look.at(-1),[0,0,6,0,true]);c.retainBindings([]);const before=look.length;c.tick(500);assert.equal(look.length,before);
});
test('actual mouse direction Down tick Cancel stays on relative input and leaves no repeat owner',()=>{
 const {h,c,look,keys,controls}=harness();h.axisX='INPUT_VEH_GUN_LR';h.axisY='INPUT_VEH_GUN_UD';const button=h.directionKeys().find(b=>b.id==='left');const saved=Date.now;Date.now=()=>mockNow;
 try{mockNow=5000;h.directionTouch(button,event(0,[0],[0],100));assert.deepEqual(look[0],[0,0,-6,0,true]);c.tick(5100);assert.equal(look.length,2);h.directionTouch(button,event(3,[],[],101));c.tick(10000);assert.equal(look.length,2);assert.equal(keys.length,0);assert.equal(controls.length,0);assert(!c.held(button.binding));}finally{Date.now=saved;mockNow=5000;}
});
test('queued old direction Down after UNKNOWN and spoofed mouse plan cannot rearm input',()=>{
 const {h,keys,look}=harness();const [r]=resolved();h.resolver=r;mockNow=5501;h.refresh();assert.equal(h.profileId,'UNKNOWN');h.directionTouch(up,event(0,[4],[4],200));assert.equal(keys.length,0);
 h.axisX='INPUT_VEH_GUN_LR';h.axisY='';const actual=h.directionKeys()[0],forged={...actual,mouseAxis:'y',mouseSign:1};h.directionTouch(forged,event(0,[5],[5],201));assert.deepEqual(look[0],[0,0,-6,0,true]);h.releaseTouches();mockNow=5000;
});
console.log(JSON.stringify({passed:cases.length,cases,scope:'actual production TS planner/controller/tracker/resolver and extracted ETS input/state method execution with injected transport/UI services; no ArkUI rendering or device effects'}));
'''

RUST_TESTS = r'''
#[test] fn combined_raw_sources_and_tick_release_do_not_cross_release() {
    let mut keys=input_ownership::Keys::default();
    keys.legacy("KeyW",true);keys.raw_event("KeyW",true,"touch:0:finger1",10);
    keys.raw_event("KeyW",true,"touch:0:finger2",10);keys.raw_event("KeyA",true,"touch:0:left",10);
    keys.raw_event("KeyW",false,"touch:0:finger1",10);keys.guest_tick(12);
    assert!(keys.contains("KeyW"));assert!(keys.contains("KeyA"));
    keys.legacy("KeyW",false);assert!(keys.contains("KeyW"));
    keys.raw_event("KeyW",false,"touch:0:finger2",12);assert!(!keys.contains("KeyW"));
    keys.clear();keys.guest_tick(100);assert!(!keys.contains("KeyA"));
}
#[test] fn task_axis_transport_preserves_group_and_release_all_clears_queued_pulse() {
    touch_controls::release();
    assert!(touch_controls::send("INPUT_SCRIPT_LEFT_AXIS_X",-1.0,2));
    assert!(touch_controls::send("INPUT_SCRIPT_LEFT_AXIS_X",0.0,2));
    // Existing native short-tap policy preserves the unsampled edge. This is
    // recorded explicitly, not mislabeled as an already observed mission effect.
    assert_eq!(touch_controls::snapshot(),vec![(2,218,-1.0)]);
    assert!(touch_controls::snapshot().is_empty());
    assert!(touch_controls::send("INPUT_SCRIPT_LEFT_AXIS_X",1.0,2));
    assert!(touch_controls::send("INPUT_SCRIPT_LEFT_AXIS_X",0.0,2));
    touch_controls::release();assert!(touch_controls::snapshot().is_empty());
    assert!(!touch_controls::send("INPUT_GUESSED_TASK",1.0,2));
    assert!(!touch_controls::send("INPUT_SCRIPT_LEFT_AXIS_X",f32::NAN,2));
}
#[test] fn successful_group2_dispatch_is_not_verified_mission_effect() {
    touch_controls::release();
    touch_controls::record_dispatch(2,218,1.0,touch_controls::Dispatch::Returned(1));
    let health=touch_controls::health_json();
    assert_eq!(health["lastGroup"],2);assert_eq!(health["lastControl"],218);
    assert_eq!(health["lastOutcome"],"nativeReturned");assert_eq!(health["semanticEffectVerified"],false);
}
'''


def identity(path):
    return {'path': str(path), 'sha256': hashlib.sha256(path.read_bytes()).hexdigest()}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--out', type=Path)
    args = parser.parse_args()
    cargo = shutil.which('cargo')
    if cargo is None:
        raise SystemExit('Source game-runtime/scripts/env.sh before running this test')
    node = shutil.which('node') or '/Applications/DevEco-Studio.app/Contents/tools/node/bin/node'
    with tempfile.TemporaryDirectory(prefix='gta-button-acceptance-') as directory:
        work = Path(directory)
        (work / 'test.cjs').write_text(NODE_TESTS)
        run = subprocess.run([node, str(work / 'test.cjs'), str(typescript_path(None)), str(CONTROLS)], text=True, capture_output=True)
        if run.returncode:
            raise RuntimeError(run.stdout + run.stderr)
        js = json.loads(run.stdout.splitlines()[-1])
        (work / 'Cargo.toml').write_text('[package]\nname="gta-button-acceptance"\nversion="0.0.0"\nedition="2021"\n[dependencies]\nserde_json="1"\n[workspace]\n')
        (work / 'src').mkdir()
        modules = '\n'.join(f'#[path={json.dumps(str(ROOT / "game-runtime/src" / (name + ".rs")), ensure_ascii=False)}] mod {name};' for name in ['input_ownership', 'touch_controls'])
        (work / 'src/lib.rs').write_text('#![allow(dead_code)]\n' + modules + RUST_TESTS)
        env = os.environ.copy()
        env['CARGO_TARGET_DIR'] = '/private/tmp/gta-button-acceptance-target'
        run = subprocess.run([cargo, 'test', '--offline', '--manifest-path', str(work / 'Cargo.toml'), '--', '--test-threads=1'], text=True, capture_output=True, env=env)
        if run.returncode:
            raise RuntimeError(run.stdout + run.stderr)
        passed = re.search(r'test result: ok\. (\d+) passed;', run.stdout)
        if not passed:
            raise RuntimeError(run.stdout + run.stderr)
        result = {'schemaVersion': 1, 'ts': js, 'rustPassed': int(passed[1]),
                  'rustScope': 'actual input_ownership and touch_controls modules including their existing unit tests and three independent cross-layer boundary tests; native queries and device are not executed',
                  'sourceEvidence': [identity(CONTROLS / name) for name in ['DirectionButtons.ts', 'TouchInput.ts', 'RawTouchBindings.ts', 'TouchState.ts', 'TaskPadLayout.ts', 'GameTouchControls.ets']] + [identity(ROOT / 'game-runtime/src' / name) for name in ['input_ownership.rs', 'touch_controls.rs']]}
        if args.out:
            args.out.parent.mkdir(parents=True, exist_ok=True)
            args.out.write_text(json.dumps(result, ensure_ascii=False, indent=2) + '\n')
        print(json.dumps(result, ensure_ascii=False))


if __name__ == '__main__':
    main()
