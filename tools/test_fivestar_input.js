/* Run after compiling controls with tsc --target ES2020 --module commonjs --strict --outDir DIR:
 * node tools/test_fivestar_input.js DIR
 * Checks policy and complete transport snapshots; this does not establish device gameplay acceptance. */
const assert = require('node:assert/strict');
const path = require('node:path');
const dir = path.resolve(process.argv[2] || '/private/tmp/fivestar-ts-controls');
const { FiveStarInput, FIVE_STAR_PAD: P, defaultFiveStarSnapshot, defaultFiveStarHints, parseFiveStarSnapshot } = require(path.join(dir, 'FiveStarInput.js'));
const { defaultFiveStarSettings, sanitizeFiveStarSettings } = require(path.join(dir, 'FiveStarSettings.js'));
const { fiveStarContext, createFiveStarButtons } = require(path.join(dir, 'FiveStarLayout.js'));
let checks = 0;
function ok(value, message) { checks++; assert.ok(value, message); }
function equal(actual, expected, message) { checks++; assert.deepEqual(Array.isArray(actual) ? actual.map(v => v === 0 ? 0 : v) : actual, expected, message); }
function fixture(vehicle = 0, settings = defaultFiveStarSettings()) {
  const input = new FiveStarInput(settings);
  input.setViewport(832, 407);
  const snapshot = { ...defaultFiveStarSnapshot(), hookReady: true, known: true, vehicle,
    status: 0, characterSwitchEnabled: true };
  input.update(snapshot, 1000);
  let now = 1000;
  return { input, snapshot,
    frame(at = now, elapsed = 1 / 60) { now = at; input.update(snapshot, now); return input.sample(now, elapsed); },
    press(name, id = 1, at = now) {
      const b = input.visibleButtons().find(b => b.name === name);
      ok(!!b, `${name} is visible`);
      input.touchDown(id, b.frame.x + b.frame.width / 2, b.frame.y + b.frame.height / 2, at);
      return input.sample(at, 0);
    }, release(id = 1, at = now) { input.touchUp(id, at); return input.sample(at, 0); },
    visible() { return input.visibleButtons().map(b => b.name); }
  };
}

equal(createFiveStarButtons().length, 72, 'all upstream button definitions retained');
const contexts = [[0, 'onFoot'], [1, 'car'], [2, 'plane'], [9, 'heli'], [10, 'heli'], [11, 'heli'],
  [12, 'bike'], [13, 'bicycle'], [14, 'boat'], [16, 'sub']];
for (const [vehicle, expected] of contexts) { equal(fiveStarContext(vehicle, false), expected, 'upstream vehicle type'); }
equal(fiveStarContext(0, true), 'menu', 'pause owns context');
equal(defaultFiveStarSettings().lookSensitivity, 2, 'look default');
equal(defaultFiveStarSettings().aimSensitivity, 1.3, 'aim default');
equal(sanitizeFiveStarSettings({ ...defaultFiveStarSettings(), cameraTouchMode: 1 }).cameraTouchMode, 2, 'pinned getter maps saved swipe variant to mouse');
equal(sanitizeFiveStarSettings({ ...defaultFiveStarSettings(), mouseLookSensitivity: 100 }).mouseLookSensitivity, 15, 'mouse sensitivity limit');
ok(!parseFiveStarSnapshot('broken').hookReady, 'malformed snapshot cannot activate input');

{
  const f = fixture();
  ok(f.visible().includes('jump') && !f.visible().includes('gas'), 'foot controls');
  const frame = f.input.button('fire').frame;
  equal(frame.width, 78, '832x407 upstream fire diameter');
  equal(frame.x + frame.width / 2, 832 * .885, 'fire center');
  f.input.touchDown(1, 180, 290, 1000);
  f.input.touchMove(1, 180, 242, 1001);
  let out = f.frame(1100);
  equal(out.leftY, 1, 'left float stick saturates');
  equal(out.buttons & P.a, 0, 'edge sprint waits 250ms');
  out = f.frame(1400, .02);
  ok((out.buttons & P.a) !== 0, 'edge sprint repeats A');
  out = f.release(1, 1500);
  equal(out.leftY, 0, 'lift recenters move stick');
  f.input.touchDown(2, 650, 240, 1510);
  f.input.touchMove(2, 698, 240, 1511);
  out = f.frame(1512);
  equal(out.rightX, 1, 'right float stick reaches full deflection');
  f.release(2, 1600);
  out = f.press('jump', 3, 1610);
  ok((out.buttons & P.x) !== 0, 'jump is X');
  out = f.release(3, 1620);
  ok((out.buttons & P.x) !== 0, 'short press retained for 70ms');
  out = f.frame(1681);
  equal(out.buttons & P.x, 0, 'minimum press expires');
}
{
  const f = fixture();
  let out = f.press('aim');
  equal(out.leftTrigger, 1, 'aim toggles LT');
  f.release();
  ok(f.visible().includes('leftFire') && !f.visible().includes('jump'), 'aim layout');
  out = f.press('leftFire', 2, 1010);
  equal(out.rightTrigger, 1, 'left fire immediate');
  f.release(2, 1020);
  f.press('aim', 3, 1100); f.release(3, 1110);
  out = f.press('fire', 4, 1200);
  equal([out.leftTrigger, out.rightTrigger], [1, 0], 'fire first locks on for 90ms');
  out = f.frame(1290);
  equal(out.rightTrigger, 1, 'fire after lock delay');
  f.release(4, 1300);
}
{
  const f = fixture();
  f.snapshot.hints.specialShown = true; f.snapshot.hints.specialFill = .7;
  f.frame();
  let out = f.press('special');
  equal(out.buttons & (P.l3 | P.r3), P.l3 | P.r3, 'ability simultaneous stick clicks');
  f.release(1, 1010);
  out = f.frame(1120);
  equal(out.buttons & (P.l3 | P.r3), P.l3 | P.r3, 'ability pulse lasts 130ms');
  out = f.frame(1140);
  equal(out.buttons & (P.l3 | P.r3), 0, 'ability pulse expires');
  f.snapshot.hints.canDetonate = true; f.snapshot.hints.helpContext = true; f.frame(1200);
  ok(f.visible().includes('detonate') && f.visible().includes('interact'), 'situational hints');
  out = f.press('interact', 2, 1210);
  equal(out.buttons & P.right, P.right, 'interact is dpad right');
}
{
  const f = fixture();
  let out = f.press('weapons');
  equal(out.buttons & P.l1, P.l1, 'weapon wheel holds LB');
  const b = f.input.button('weapons').frame;
  f.input.touchMove(1, b.x + b.width / 2 + 40, b.y + b.height / 2, 1010);
  out = f.frame(1010);
  equal([out.rightX, out.rightY], [1, 0], 'same finger points weapon slot');
  ok(f.visible().includes('weaponPrev') && f.visible().includes('weaponNext'), 'wheel slot cycling buttons');
  f.release(1, 1100);
  out = f.press('switchCharacter', 2, 1200);
  equal(out.buttons & P.down, P.down, 'character wheel holds dpad down');
  const s = f.input.button('switchCharacter').frame;
  f.input.touchMove(2, s.x + s.width / 2, s.y + s.height / 2 + 50, 1210);
  out = f.frame(1210);
  equal(out.rightY, -1, 'character wheel direction');
}
{
  const f = fixture(1);
  ok(f.visible().includes('gas') && f.visible().includes('driveAim') && !f.visible().includes('jump'), 'car layout');
  let out = f.press('gas');
  equal(out.rightTrigger, 1, 'gas is RT');
  const brake = f.input.button('brake').frame;
  f.input.touchMove(1, brake.x + brake.width / 2, brake.y + brake.height / 2, 1010);
  out = f.frame(1010);
  equal([out.leftTrigger, out.rightTrigger], [1, 0], 'pedal roll releases gas and presses brake');
  f.release(1, 1100);
  out = f.press('driveBy', 2, 1200);
  equal(out.buttons & (P.l1 | P.r1), P.l1, 'drive-by aims first');
  out = f.frame(1321);
  equal(out.buttons & (P.l1 | P.r1), P.l1 | P.r1, 'drive-by shoots after 120ms');
  f.release(2, 1400);
  f.press('radio', 3, 1500);
  out = f.release(3, 1600);
  equal(out.buttons & P.left, P.left, 'radio short tap station pulse');
  out = f.press('radio', 4, 1800);
  const radio = f.input.button('radio').frame;
  f.input.touchMove(4, 832 / 2 + 80, 407 / 2, 1810);
  out = f.frame(2060);
  equal([out.radioActive, out.radioX, out.radioY, out.leftX, out.rightX], [true, 1, 0, 0, 0], 'radio wheel owns axes');
  f.release(4, 2100);
}
{
  const f = fixture(13);
  let out = f.press('gas');
  equal(out.buttons & P.a, P.a, 'bicycle pedals with A');
  equal(out.rightTrigger, 0, 'bicycle gas is not RT');
  f.release(1, 1200);
  out = f.press('brake', 2, 1300); equal(out.leftTrigger, 1, 'bicycle LT brake');
  f.release(2, 1400);
  out = f.press('handbrake', 3, 1500); equal(out.rightTrigger, 1, 'bicycle RT rear brake');
}
{
  const settings = defaultFiveStarSettings(); settings.airplaneRightStick = true;
  const f = fixture(2, settings);
  ok(!f.visible().includes('throttleUp') && f.visible().includes('gear'), 'plane right-stick alternative');
  f.input.touchDown(1, 832 * .8, 407 * .6, 1000);
  const travel = f.input.sticks().find(s => s.id === 'plane').travel;
  f.input.touchMove(1, 832 * .8, 407 * .6 - travel, 1010);
  let out = f.frame(1010);
  equal(out.rightTrigger, .5, 'plane throttle integrated');
  f.release(1, 1100);
  out = f.frame(1200);
  equal(out.rightTrigger, .5, 'plane throttle retained after lift');
  out = f.press('enter', 2, 1300);
  equal(out.buttons & P.y, 0, 'aircraft exit guard');
  out = f.frame(1699); equal(out.buttons & P.y, 0, 'guard before 400ms');
  out = f.frame(1700); equal(out.buttons & P.y, P.y, 'aircraft exit at 400ms');
  f.snapshot.vehicle = 9; f.frame(1800);
  ok(f.visible().includes('throttleUp') && f.visible().includes('yawLeft'), 'helicopter keeps original buttons');
  out = f.press('throttleUp', 3, 1900); equal(out.rightTrigger, 1, 'helicopter throttle');
}
{
  const f = fixture();
  // Entering a car releases old action while carrying a live move finger.
  f.input.touchDown(1, 170, 300, 1000); f.input.touchMove(1, 194, 300, 1010);
  f.press('jump', 2, 1020);
  f.snapshot.vehicle = 1;
  let out = f.frame(1030);
  ok(out.leftX > 0, 'vehicle transition retains stick finger');
  // The pinned upstream minimum press can bridge a just-pressed X for 70ms.
  out = f.frame(1110); equal(out.buttons & P.x, 0, 'old action does not remain held in new context');
  f.release(1, 1200); equal(f.frame(1210).leftX, 0, 'carried finger can lift');
}
{
  const f = fixture();
  f.snapshot.status = 1; f.frame(1000); f.frame(1601);
  ok(f.input.showingPrompt && f.visible().includes('accept') && !f.visible().includes('fire'), 'control-off prompt debounce');
  f.snapshot.hints.helpShown = true;
  f.snapshot.hints.helpInputs = (1 << (220 - 218)) | (1 << (229 - 218));
  f.snapshot.missionInputs = [220, 229]; f.frame(1610);
  ok(f.visible().includes('script_rt') && !f.visible().includes('up'), 'right-stick mission trigger and empty stick zone');
  let out = f.press('script_rt', 1, 1620);
  equal(out.rightTrigger, 1, 'mission RT pressed');
  f.release(1, 1630);
  out = f.frame(1640); equal(out.rightTrigger, 1, 'RT latch leaves thumb free');
  f.press('script_rt', 2, 1650); out = f.release(2, 1660);
  equal(out.rightTrigger, 0, 'second tap unlatches');
  f.snapshot.hints.helpInputs = 0; f.snapshot.missionInputs = [228, 229]; f.frame(1700);
  ok(f.visible().includes('script_ltrt') && !f.visible().includes('script_lt'), 'combined trigger mission control');
  out = f.press('script_ltrt', 3, 1710); f.release(3, 1720);
  equal([out.leftTrigger, out.rightTrigger], [1, 1], 'combined trigger latch');
  f.snapshot.missionInputs = []; f.frame(1800);
  out = f.frame(1810); equal([out.leftTrigger, out.rightTrigger], [0, 0], 'latch clears when mission stops asking');
}
{
  const f = fixture();
  f.snapshot.missionInputs = [187]; f.frame();
  ok(f.visible().includes('fire'), 'one frontend direction does not imply script menu');
  f.snapshot.missionInputs = [187, 188]; f.frame(1010);
  ok(f.visible().includes('up') && f.visible().includes('accept'), 'both frontend directions imply shop navigation');
  f.snapshot.missionInputs = []; f.snapshot.hints.helpShown = true; f.snapshot.hints.helpInputs = 1; f.frame(1100);
  ok(f.visible().includes('left') && !f.visible().includes('fire'), 'TV couch arrows');
  let out = f.press('right', 1, 1110);
  equal([out.leftX, out.buttons & P.right], [1, 0], 'TV right changes left-stick axis');
  f.release(1, 1200);
  f.snapshot.hints = { ...defaultFiveStarHints(), passenger: true, taxiMenu: true, vehicleModel: 0xC703DB5F };
  f.snapshot.vehicle = 1; f.frame(1300);
  ok(f.visible().includes('taxi_hurry') && !f.visible().includes('gas'), 'taxi passenger layout');
  out = f.press('right', 2, 1310);
  equal([out.leftX, out.buttons & P.right], [1, 0], 'taxi destination uses left stick');
}
{
  const f = fixture(1);
  f.snapshot.hints.pedModel = 0; f.snapshot.hints.vehicleModel = 0xB12314E0; f.snapshot.hints.trailerAttached = true; f.frame();
  ok(f.visible().includes('towHook') && f.visible().includes('towArmDown') && f.visible().includes('detachTrailer'), 'vehicle model and trailer controls');
  f.snapshot.hints.vehicleModel = 0xFCFCB68B; f.snapshot.vehicle = 9; f.frame(1100);
  ok(f.visible().includes('towHook'), 'Cargobob hook');
  let out = f.press('vehicleWeapon', 1, 1200);
  equal([out.nextAircraftWeapon, out.buttons & P.left], [true, 0], 'aircraft weapon action does not open radio');
}
{
  const f = fixture();
  let out = f.press('phone');
  ok(f.input.phoneDockOpen, 'phone dock opens'); equal(out.buttons & P.up, P.up, 'phone request is dpad up');
  f.release(1, 1010);
  f.snapshot.status = 2; f.frame(1100);
  equal(f.input.button('accept').icon, 'script_a', 'phone buttons use game A/B/X hints');
  f.snapshot.status = 0; f.frame(1200); ok(!f.input.phoneDockOpen, 'dock follows game phone closing');
  f.snapshot.status = 2; f.frame(1300);
  ok(!f.input.phoneDockOpen && f.visible().includes('fire'), 'incoming call leaves play controls');
  out = f.press('phone', 2, 1400);
  ok(f.input.phoneDockOpen, 'phone tap opens incoming dock'); equal(out.buttons & P.b, 0, 'incoming phone tap does not hang up');
}
{
  const settings = defaultFiveStarSettings(); settings.cameraTouchMode = 2;
  const f = fixture(1, settings); f.frame();
  f.input.touchDown(1, 500, 250, 1010); f.input.sample(1010, 0);
  f.input.touchMove(1, 510, 252, 1020);
  let out = f.frame(1020);
  equal([out.mouseDx, out.mouseDy, out.mouseActive], [50, 10, true], 'mouse swipe gain and pixels');
  out = f.frame(1030); equal([out.mouseDx, out.mouseDy], [0, 0], 'motion consumed once');
  out = f.press('driveAim', 2, 1040); equal(out.mouseButtons & 2, 2, 'swipe vehicle aim also right mouse');
  f.release(2, 1050);
  f.press('radio', 3, 1100);
  f.input.touchMove(1, 530, 260, 1110);
  out = f.frame(1400);
  equal([out.mouseDx, out.mouseDy, out.mouseActive], [0, 0, false], 'selection wheel suppresses camera swipe');
}
{
  const f = fixture();
  f.input.setMinimapRect({ x: 10, y: 10, width: 100, height: 80 });
  f.input.touchDown(1, 40, 40, 1000); f.input.sample(1000, 0);
  let out = f.release(1, 1100); equal(out.buttons & P.start, P.start, 'tap relocated minimap requests map');
  f.snapshot.pauseMenu = true; f.frame(1200);
  out = f.frame(2701); equal(out.buttons & P.a, P.a, 'after map opens A requests fullscreen');
  f.input.beginMapPinch(); f.input.mapPinch(1.4);
  out = f.frame(2710); equal(out.mouseWheel, 2, 'map pinch 15 percent notches');
}
{
  const f = fixture();
  f.input.editing = true; f.input.setEditorContext('plane');
  f.input.update(f.snapshot, 1100); equal(f.input.context, 'plane', 'live snapshot cannot change editor preview');
  f.input.setLayoutEntry('gear', { x: .5, y: .5, scale: 1.6, hidden: true });
  equal(f.input.getLayoutEntry('gear').scale, 1.6, 'customized aircraft vehicle entry');
  ok(f.input.visibleButtons().find(b => b.name === 'gear').hidden, 'editor retains hidden controls');
  f.input.setLayoutEntry('enter', { x: .5, y: .5, scale: 1, hidden: true });
  equal(f.input.getLayoutEntry('enter').hidden, false, 'essential control cannot hide');
  equal(f.input.sample(1100, .1).active, false, 'editor does not play buttons');
  f.input.resetLayoutGroup(); equal(f.input.getLayoutEntry('gear'), undefined, 'reset current group');
}
{
  const f = fixture();
  f.press('jump');
  let out = f.input.sample(2600, .1); equal([out.active, out.buttons], [false, 0], 'stale snapshot cancels held and minimum press');
  f.snapshot.known = false; f.snapshot.status = 16; f.frame(2700);
  equal(f.input.context, 'menu', 'hook-ready without ped exposes frontend navigation');
  out = f.press('accept', 2, 2710); equal(out.buttons & P.a, P.a, 'frontend accepts input before world ready');
  f.input.reset(); out = f.frame(2720); equal(out.buttons, 0, 'public reset clears all output');
  f.snapshot.hookReady = false; f.frame(2800); out = f.input.sample(2800, 0); equal(out.active, false, 'missing hook deactivates pad');
}
console.log(`Five Star input checks passed: ${checks}`);
