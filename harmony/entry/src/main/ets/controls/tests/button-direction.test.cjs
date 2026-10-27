// Actual TypeScript direction bindings, pointer ownership and task-pad layout.
const fs = require('node:fs');
const path = require('node:path');
const assert = require('node:assert/strict');
const ts = require('/Applications/DevEco-Studio.app/Contents/tools/arktsdoc/node_modules/typescript/lib/typescript.js');
require.extensions['.ts'] = (m, file) => m._compile(ts.transpileModule(fs.readFileSync(file, 'utf8'), {
  compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2020 }
}).outputText, file);
const base = path.resolve(__dirname, '..');
const { directionButtons } = require(path.join(base, 'DirectionButtons.ts'));
const { TouchButtonTracker, TouchInputController } = require(path.join(base, 'TouchInput.ts'));
const { TOUCH_PROFILES } = require(path.join(base, 'TouchLayout.ts'));
const { RAW_TOUCH_AXES, rawTouchBinding } = require(path.join(base, 'RawTouchBindings.ts'));
const { TASK_WIDGETS } = require(path.join(base, 'TaskPadLayout.ts'));
const { directionButtonFrame } = require(path.join(base, 'TouchGeometry.ts'));
const { defaultTouchFamilySettings, validateTouchSettings, touchSettingsFamily } = require(path.join(base, 'TouchSettings.ts'));
let passed = 0;
function test(name, fn) { fn(); passed++; console.log('PASS ' + name); }
const p = id => ({ id });
function recording() {
  const events = [], rawSources = new Map();
  const c = new TouchInputController({
    control: (...args) => { events.push(['control', ...args]); return true; },
    rawKey: (key, down, source) => {
      events.push(['key', key, down, source]);
      if (down) rawSources.set(source, key); else rawSources.delete(source);
      return true;
    }, mouse: () => true, wheel: () => {}, look: () => {}
  });
  return { c, events, held: key => [...rawSources.values()].includes(key) };
}

test('FOOT buttons send exact W/A/S/D and car steering is only A/D', () => {
  const foot = TOUCH_PROFILES.find(p => p.id === 'FOOT_FIREARM');
  const buttons = directionButtons(foot.axisX, foot.axisY, foot.directionControls);
  assert.deepEqual(buttons.map(b => [b.id, b.label, b.mapping.keys[0]]),
    [['left', 'A', 'KeyA'], ['right', 'D', 'KeyD'], ['up', 'W', 'KeyW'], ['down', 'S', 'KeyS']]);
  assert.equal(new Set(buttons.map(b => b.binding)).size, 4);
  const car = TOUCH_PROFILES.find(p => p.id === 'VEHICLE_CAR');
  assert.deepEqual(directionButtons(car.axisX, car.axisY, []).map(b => b.mapping.keys[0]), ['KeyA', 'KeyD']);
  assert(car.slots.some(a => a.control === 'INPUT_VEH_ACCELERATE'));
  assert(car.slots.some(a => a.control === 'INPUT_VEH_BRAKE'));
});
test('every mapped axis preserves both original PC directions and unique bindings', () => {
  for (const axis of RAW_TOUCH_AXES) {
    const buttons = directionButtons(axis.control, '', []);
    assert.equal(buttons.length, 2);
    assert.deepEqual(buttons.map(b => b.mapping), [rawTouchBinding(axis.negative), rawTouchBinding(axis.positive)]);
    assert.notEqual(buttons[0].binding, buttons[1].binding);
  }
  for (const id of ['SWIM_SURFACE', 'SWIM_UNDERWATER', 'FREEFALL', 'PARACHUTE', 'VEHICLE_MOTORBIKE',
    'VEHICLE_BICYCLE', 'VEHICLE_BOAT', 'VEHICLE_SUBMARINE', 'VEHICLE_HELI', 'VEHICLE_PLANE']) {
    const profile = TOUCH_PROFILES.find(p => p.id === id);
    assert.equal(directionButtons(profile.axisX, profile.axisY, []).filter(b => b.mapping).length, 4, id);
  }
});
test('phone directions retain Arrow inputs; unknown/restricted profiles gain no fabricated mapping', () => {
  const phone = TOUCH_PROFILES.find(p => p.id === 'GAME_PHONE');
  assert.deepEqual(directionButtons('', '', phone.directionControls).map(b => b.mapping.keys[0]),
    ['ArrowLeft', 'ArrowRight', 'ArrowUp', 'ArrowDown']);
  assert.deepEqual(directionButtons('', '', []), []);
  assert.deepEqual(directionButtons('INPUT_UNKNOWN_AXIS', '', []), []);
  const turret = directionButtons('INPUT_VEH_GUN_LR', 'INPUT_VEH_GUN_UD', []);
  assert(turret.every(b => b.mapping === undefined));
  assert.deepEqual(turret.map(b => [b.mouseAxis, b.mouseSign]), [['x', -1], ['x', 1], ['y', -1], ['y', 1]]);
});
test('two fingers share W while an independent A survives one missing-Up', () => {
  const tracker = new TouchButtonTracker(), r = recording();
  const w = directionButtons('INPUT_MOVE_LR', 'INPUT_MOVE_UD', []).find(b => b.id === 'up');
  const a = directionButtons('INPUT_MOVE_LR', 'INPUT_MOVE_UD', []).find(b => b.id === 'left');
  for (const [button, id] of [[w, 1], [w, 2], [a, 3]]) {
    const contact = tracker.begin(button.id, id, r.c.getEpoch());
    r.c.pressMapped(contact.owner, button.binding, button.mapping);
  }
  for (const contact of tracker.end('up', true, false, [], [p(2), p(3)])) r.c.release(contact.owner);
  assert.equal(tracker.contains(1), false); assert(r.held('KeyW')); assert(r.held('KeyA'));
  for (const contact of tracker.end('up', true, false, [p(2)], [p(3)])) r.c.release(contact.owner);
  assert(!r.held('KeyW')); assert(r.held('KeyA'));
  r.c.releaseAll(); tracker.reset(); assert(!r.held('KeyA'));
});
test('target Cancel with empty lists clears all its contacts but preserves other buttons', () => {
  const tracker = new TouchButtonTracker();
  tracker.begin('left', 1, 0); tracker.begin('left', 2, 0); tracker.begin('right', 3, 0);
  assert.equal(tracker.end('left', false, true, [], []).length, 2);
  assert(!tracker.contains(1)); assert(!tracker.contains(2)); assert(tracker.contains(3));
  assert.equal(tracker.end('right', true, false, [p(4)], [p(3)]).length, 0);
  assert(tracker.contains(3));
});
test('duplicate Down, a different target and stale epoch cannot inherit a contact', () => {
  const tracker = new TouchButtonTracker();
  assert(tracker.begin('up', 7, 0, 100));
  assert.equal(tracker.begin('up', 7, 0, 101), undefined);
  assert.equal(tracker.begin('left', 7, 0, 101), undefined);
  assert.equal(tracker.current('up', 7, 1), undefined);
  tracker.reset(); assert.equal(tracker.current('up', 7, 1), undefined);
  assert(tracker.begin('up', 7, 1, 200));
  assert.equal(tracker.end('up', true, false, [p(7)], [], 150).length, 0);
  assert(tracker.current('up', 7, 1));
  assert.equal(tracker.end('up', true, false, [p(7)], [], 201).length, 1);
});
test('unchanged direction bindings retain holds and context reset releases original sources', () => {
  const r = recording(), tracker = new TouchButtonTracker();
  const buttons = directionButtons('INPUT_MOVE_LR', 'INPUT_MOVE_UD', []), w = buttons.find(b => b.id === 'up');
  const contact = tracker.begin('up', 1, r.c.getEpoch());
  r.c.pressMapped(contact.owner, w.binding, w.mapping);
  for (let i = 0; i < 20; i++) r.c.retainBindings(buttons.map(b => b.binding));
  assert(r.held('KeyW')); assert.equal(r.events.length, 1);
  r.c.releaseAll(); tracker.reset(); r.c.releaseAll();
  assert(!r.held('KeyW')); assert.equal(r.events.length, 2);
  assert.equal(r.events[0][3], r.events[1][3]);
  assert.equal(tracker.current('up', 1, r.c.getEpoch()), undefined);
});
test('task axis direction owners cancel algebraically and releasing one restores the other', () => {
  const r = recording(), tracker = new TouchButtonTracker();
  const left = tracker.begin('task:left', 1, 0), right = tracker.begin('task:right', 2, 0);
  r.c.setControl(left.owner, left.target, 'INPUT_SCRIPT_LEFT_AXIS_X', -1, 2);
  r.c.setControl(right.owner, right.target, 'INPUT_SCRIPT_LEFT_AXIS_X', 1, 2);
  assert.deepEqual(r.events.at(-1), ['control', 'INPUT_SCRIPT_LEFT_AXIS_X', 0, 2]);
  for (const c of tracker.end('task:left', true, false, [p(1)], [p(2)])) r.c.release(c.owner);
  assert.deepEqual(r.events.at(-1), ['control', 'INPUT_SCRIPT_LEFT_AXIS_X', 1, 2]);
  for (const c of tracker.end('task:right', false, true, [], [])) r.c.release(c.owner);
  assert.deepEqual(r.events.at(-1), ['control', 'INPUT_SCRIPT_LEFT_AXIS_X', 0, 2]);
});
test('rejected task input leaves no held owner or raw-key fallback', () => {
  const calls = [];
  const c = new TouchInputController({ control: (...v) => { calls.push(v); return false; },
    rawKey: () => { throw Error('forbidden SCRIPT raw fallback'); }, mouse: () => true, wheel: () => {}, look: () => {} });
  assert.equal(c.setControl('bad', 'task:left', 'INPUT_SCRIPT_LEFT_AXIS_X', -1, 2), false);
  assert.equal(c.held('task:left'), false); c.releaseAll(); assert.equal(calls.length, 1);
});
test('turret mouse holds repeat at bounded100ms with no catch-up burst or keyboard/control fallback', () => {
  const events = [];
  const c = new TouchInputController({ control: () => { throw Error('no control fallback'); },
    rawKey: () => { throw Error('no invented keyboard mapping'); }, mouse: () => true, wheel: () => {},
    look: (...args) => events.push(args) });
  assert(c.pressLookDirection('left', 'mouse:left', 'x', -1, 6, 1000));
  assert.deepEqual(events, [[0, 0, -6, 0, true]]);
  c.tick(1099); assert.equal(events.length, 1); c.tick(1100); assert.equal(events.length, 2);
  c.tick(90000); assert.equal(events.length, 3); c.tick(90000); assert.equal(events.length, 3);
  c.retainBindings(['mouse:left']); c.tick(90100); assert.equal(events.length, 4);
  c.release('left'); c.tick(90200); assert.equal(events.length, 4);
  assert.equal(c.held('mouse:left'), false);
});
test('mouse opposite directions cancel independent of duplicate finger counts and resume remaining side', () => {
  const events = [];
  const c = new TouchInputController({ control: () => true, rawKey: () => true, mouse: () => true,
    wheel: () => {}, look: (...args) => events.push(args) });
  c.pressLookDirection('left:1', 'left', 'x', -1, 6, 1000);
  c.pressLookDirection('left:2', 'left', 'x', -1, 6, 1001);
  c.pressLookDirection('right', 'right', 'x', 1, 6, 1002);
  c.tick(1100); assert.equal(events.length, 1);
  c.release('left:1'); c.tick(1200); assert.equal(events.length, 1);
  c.release('left:2'); c.tick(1300); assert.deepEqual(events.at(-1), [0, 0, 6, 0, true]);
  c.pressLookDirection('down', 'down', 'y', 1, 6, 1301);
  c.tick(1400); assert.deepEqual(events.at(-1), [0, 0, 6, 6, true]);
  c.releaseAll(); const count = events.length; c.tick(1500); assert.equal(events.length, count);
});
test('mouse plans reject unknown axes/nonfinite values and clamp one step', () => {
  const events = [];
  const c = new TouchInputController({ control: () => true, rawKey: () => true, mouse: () => true,
    wheel: () => {}, look: (...args) => events.push(args) });
  assert.equal(c.pressLookDirection('bad', 'bad', 'z', 1, 6, 0), false);
  assert.equal(c.pressLookDirection('bad', 'bad', 'x', 0.5, 6, 0), false);
  assert.equal(c.pressLookDirection('bad', 'bad', 'x', 1, NaN, 0), false);
  assert(c.pressLookDirection('ok', 'ok', 'y', -1, 100000, 0));
  assert.deepEqual(events, [[0, 0, 0, -24, true]]);
  c.retainBindings([]); c.tick(1000); assert.equal(events.length, 1);
});
test('task pad has two four-key sets covering original 19 channels with no overlaps', () => {
  const axes = TASK_WIDGETS.filter(w => w.kind === 'axis_button');
  assert.equal(axes.length, 8); assert(!TASK_WIDGETS.some(w => w.kind === 'stick'));
  const channels = new Set(TASK_WIDGETS.flatMap(w => w.channels)); assert.equal(channels.size, 19);
  assert([...channels].every(c => c.startsWith('INPUT_SCRIPT_')));
  for (const channel of ['INPUT_SCRIPT_LEFT_AXIS_X', 'INPUT_SCRIPT_LEFT_AXIS_Y', 'INPUT_SCRIPT_RIGHT_AXIS_X', 'INPUT_SCRIPT_RIGHT_AXIS_Y']) {
    assert.deepEqual(axes.filter(w => w.channels[0] === channel).map(w => w.value).sort(), [-1, 1]);
  }
  for (const w of TASK_WIDGETS) {
    assert(w.width >= 44 && w.height >= 44 && w.x >= 0 && w.y >= 0 && w.x + w.width <= 640 && w.y + w.height <= 360, w.id);
    for (const other of TASK_WIDGETS) if (other !== w) {
      assert(!(w.x < other.x + other.width && w.x + w.width > other.x && w.y < other.y + other.height && w.y + w.height > other.y), `${w.id}/${other.id}`);
    }
  }
});
test('all direction targets are visible44vp and custom layouts cannot overlap them', () => {
  for (const [w, h] of [[640, 360], [764, 368.333333], [800, 600], [1200, 800]]) {
    assert.equal(validateTouchSettings(w, h, defaultTouchFamilySettings()), '');
    for (const direction of ['left', 'right', 'up', 'down']) assert.equal(directionButtonFrame(direction, w, h).diameter, 44);
  }
  const settings = defaultTouchFamilySettings();
  settings.slots.S4 = { x: 32, y: 60, diameter: 44 };
  assert.match(validateTouchSettings(800, 360, settings), /方向按键/);
  assert.equal(touchSettingsFamily('FOOT_AIM', 800, 360), touchSettingsFamily('FOOT_FIREARM', 800, 360));
});
console.log(JSON.stringify({ passed, scope: 'actual TS bindings/controller/tracker/geometry/task inventory; device delivery, multi-touch routing and game effects remain separate acceptance' }));
