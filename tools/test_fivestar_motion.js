// Run after tsc of FiveStar{Input,Layout,Settings,MotionMath}.ts into the supplied directory.
// Synthetic sensor traces verify integration/calibration, not physical device orientation.
const assert = require('node:assert/strict');
const path = require('node:path');
const dir = path.resolve(process.argv[2] || '/private/tmp/fivestar-ts-controls');
const { FiveStarMotionCore } = require(path.join(dir, 'FiveStarMotionMath.js'));
const { emptyFiveStarOutput } = require(path.join(dir, 'FiveStarInput.js'));
const { defaultFiveStarSettings } = require(path.join(dir, 'FiveStarSettings.js'));
let checks = 0;
function ok(value, label) { checks++; assert.ok(value, label); }
function equal(actual, expected, label) { checks++; assert.deepEqual(actual, expected, label); }
function near(actual, expected, label) { checks++; assert.ok(Math.abs(actual - expected) < 1e-8, `${label}: ${actual} != ${expected}`); }
function feed(core, rate, begin = 0, count = 12, spacing = 1000 / 120) {
  for (let i = 0; i <= count; i++) {
    const at = begin + i * spacing;
    core.gravitySample({ x: 0, y: 9.80665, z: 0 }, at);
    core.gyroscopeSample(rate, Math.round(at * 1e6), at);
  }
}
{
  const core = new FiveStarMotionCore();
  feed(core, { x: 0, y: -1, z: 0 });
  near(core.rate(true, .1, 100).x, 1, 'all 120 Hz samples contribute to player-space yaw');
  near(core.rate(true, .1, 100).x, .25, 'a second poll has only the upstream smoothing tail, no replayed rotation');
  feed(core, { x: 0, y: -1, z: 0 }, 100, 12);
  equal(core.rate(false, .1, 200), { x: 0, y: 0 }, 'inactive aim discards accumulated travel');
  equal(core.rate(true, .1, 200), { x: 0, y: 0 }, 'enabling aim cannot replay old travel');
  feed(core, { x: 0, y: -1, z: 0 }, 200, 12);
  equal(core.rate(true, .1, 700), { x: 0, y: 0 }, 'stale sensor samples cannot move camera');
}
{
  const first = new FiveStarMotionCore(); first.setRotation(1);
  feed(first, { x: 0, y: 1, z: 0 });
  near(first.rate(true, .1, 100).y, 1, 'clockwise landscape maps device Y into pitch');
  const opposite = new FiveStarMotionCore(); opposite.setRotation(3);
  feed(opposite, { x: 0, y: 1, z: 0 });
  near(opposite.rate(true, .1, 100).y, -1, 'opposite landscape reverses pitch');
  opposite.setRotation(1);
  equal(opposite.rate(true, .1, 100), { x: 0, y: 0 }, 'orientation change resets motion');
}
{
  const core = new FiveStarMotionCore(); const settings = defaultFiveStarSettings(); settings.gyroAim = 2;
  feed(core, { x: 0, y: -1, z: 0 });
  core.apply(emptyFiveStarOutput(), 'onFoot', false, settings, 0, true, 100);
  ok(core.apply(emptyFiveStarOutput(), 'onFoot', false, settings, .1, true, 100).rightX > .5,
    'zero-elapsed immediate touch preserves sensor travel for the next frame');
  feed(core, { x: 0, y: -1, z: 0 }, 100, 12);
  const capped = emptyFiveStarOutput(); capped.rightX = .9;
  near(core.apply(capped, 'onFoot', false, settings, .1, true, 200).rightX, 1, 'gyro adds and clamps to thumb input');
  feed(core, { x: 0, y: -1, z: 0 }, 200, 12);
  equal(core.apply(emptyFiveStarOutput(), 'car', false, settings, .1, true, 300).rightX, 0,
    'always gyro mode applies only on foot unless aiming');
  feed(core, { x: 0, y: -1, z: 0 }, 300, 12);
  equal(core.apply(emptyFiveStarOutput(), 'menu', true, settings, .1, true, 400).rightX, 0, 'menu excludes gyro');
  feed(core, { x: 0, y: -1, z: 0 }, 400, 12);
  equal(core.apply(emptyFiveStarOutput(), 'plane', true, settings, .1, true, 500, true).rightX, 0,
    'aircraft throttle stick owns right-stick output');
}
{
  const core = new FiveStarMotionCore(); const settings = defaultFiveStarSettings(); settings.gyroAim = 1;
  feed(core, { x: -1, y: 0, z: 0 });
  const normal = core.apply(emptyFiveStarOutput(), 'onFoot', true, settings, .1, true, 100).rightY;
  feed(core, { x: -1, y: 0, z: 0 }, 100, 12); settings.gyroInvertY = true;
  const inverted = core.apply(emptyFiveStarOutput(), 'onFoot', true, settings, .1, true, 200).rightY;
  near(normal, -inverted, 'gyro invert Y reverses pitch only');
  core.reset(); feed(core, { x: 0, y: -.005, z: 0 });
  equal(core.apply(emptyFiveStarOutput(), 'onFoot', true, settings, .1, true, 100).rightX, 0, 'tiny tremor stays below look threshold');
}
function gravityAt(core, degrees, at) {
  const radians = degrees * Math.PI / 180;
  core.gravitySample({ x: -Math.sin(radians) * 9.80665, y: Math.cos(radians) * 9.80665, z: 0 }, at);
}
function calibrate(core, settings) {
  for (let now = 0; now <= 200; now += 20) {
    gravityAt(core, 10, now);
    near(core.apply(emptyFiveStarOutput(), 'car', false, settings, .02, true, now).leftX, 0, 'neutral captures held entry angle');
  }
}
{
  const core = new FiveStarMotionCore(); const settings = defaultFiveStarSettings(); settings.tiltSteer = 2;
  calibrate(core, settings);
  gravityAt(core, 11, 220);
  equal(core.apply(emptyFiveStarOutput(), 'car', false, settings, .02, true, 220).leftX, 0, 'tilt deadzone is 1.5 degrees');
  let turn = 0;
  for (let now = 240; now <= 800; now += 20) {
    gravityAt(core, 33.5, now); turn = core.apply(emptyFiveStarOutput(), 'car', false, settings, .02, true, now).leftX;
  }
  ok(turn > .99, 'calibrated full-lock tilt fades and smooths into full steering');
  const thumb = emptyFiveStarOutput(); thumb.leftX = -.4; gravityAt(core, 33.5, 820);
  near(core.apply(thumb, 'car', false, settings, .02, true, 820).leftX, -.4, 'full tilt respects an engaged touch steering stick');
  settings.tiltSteer = 1; const assist = emptyFiveStarOutput(); assist.leftX = -.4; gravityAt(core, 33.5, 840);
  ok(Math.abs(core.apply(assist, 'car', false, settings, .02, true, 840).leftX - .2) < .001, 'assist adds 60 percent tilt to thumb input');
  core.apply(emptyFiveStarOutput(), 'onFoot', false, settings, .02, true, 860);
  gravityAt(core, 33.5, 880);
  equal(core.apply(emptyFiveStarOutput(), 'car', false, settings, .02, true, 880).leftX, 0, 'leaving vehicle recalibrates steering');
}
{
  const core = new FiveStarMotionCore(); core.gravitySample({ x: 0, y: 0, z: 9.8 }, 0);
  equal(core.steeringAngle(0), undefined, 'flat phone cannot invent steering angle');
  gravityAt(core, 0, 1); core.gyroscopeSample({ x: NaN, y: 1, z: 0 }, 10, 1);
  equal(core.rate(true, .02, 1), { x: 0, y: 0 }, 'invalid sensor values are ignored');
  const settings = defaultFiveStarSettings(); feed(core, { x: 0, y: -1, z: 0 });
  const output = emptyFiveStarOutput(); output.rightX = .2; output.leftX = -.3;
  const unchanged = core.apply(output, 'onFoot', true, settings, .1, true, 100);
  equal([unchanged.leftX, unchanged.rightX], [-.3, .2], 'upstream gyro and tilt defaults remain off');
}
console.log(`Five Star motion: ${checks} assertions passed`);
