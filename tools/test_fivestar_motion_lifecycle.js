// Execute the actual plain-class .ets adapter with mocked SDK boundaries.
// The separate ArkTS build checks real SDK signatures; this checks subscription ownership.
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');
const ts = require(process.env.FIVESTAR_TYPESCRIPT_PATH || '/Applications/DevEco-Studio.app/Contents/tools/ohpm/node_modules/typescript/lib/typescript.js');
const dir = path.resolve(process.argv[2] || '/private/tmp/fivestar-ts-controls');
const { defaultFiveStarSettings } = require(path.join(dir, 'FiveStarSettings.js'));
const { emptyFiveStarOutput } = require(path.join(dir, 'FiveStarInput.js'));
let checks = 0, clock = 0, additions = 0, removals = 0;
const subscribed = new Map();
const externalCallback = () => {};
subscribed.set(2, new Set([externalCallback]));
const sensor = { SensorId: { GRAVITY: 257, GYROSCOPE: 2 },
  on(type, callback, options) {
    assert.equal(options.interval, 8333333); additions++;
    if (!subscribed.has(type)) subscribed.set(type, new Set());
    subscribed.get(type).add(callback);
  },
  off(type, callback) { assert.equal(typeof callback, 'function'); removals++; subscribed.get(type).delete(callback); }
};
const displayCallbacks = new Set();
const display = { getDefaultDisplaySync: () => ({ rotation: 0 }),
  on(name, callback) { assert.equal(name, 'change'); displayCallbacks.add(callback); },
  off(name, callback) { assert.equal(name, 'change'); displayCallbacks.delete(callback); }
};
const source = fs.readFileSync(path.join(__dirname, '../harmony/entry/src/main/ets/controls/FiveStarMotion.ets'), 'utf8');
const compiled = ts.transpileModule(source, { fileName: 'FiveStarMotion.ts', compilerOptions: { target: ts.ScriptTarget.ES2020, module: ts.ModuleKind.CommonJS } });
const moduleObject = { exports: {} };
vm.runInNewContext(compiled.outputText, { exports: moduleObject.exports, module: moduleObject,
  require(name) {
    if (name === '@kit.SensorServiceKit') return { sensor };
    if (name === '@kit.ArkUI') return { display };
    if (name === '@kit.BasicServicesKit') return { systemDateTime: { TimeType: { STARTUP: 0 }, getUptime: () => clock } };
    return require(path.join(dir, name + '.js'));
  }
});
const { FiveStarMotion } = moduleObject.exports;
const motion = new FiveStarMotion(); const settings = defaultFiveStarSettings();
function equal(actual, expected, label) { checks++; assert.deepEqual(actual, expected, label); }
motion.apply(emptyFiveStarOutput(), 'onFoot', false, settings, .016, true);
equal(additions, 0, 'default off modes never subscribe sensors');
settings.gyroAim = 2;
motion.apply(emptyFiveStarOutput(), 'onFoot', false, settings, .016, true);
equal([additions, displayCallbacks.size], [2, 1], 'enabling motion subscribes gravity/gyro/display once');
motion.apply(emptyFiveStarOutput(), 'onFoot', false, settings, .016, true);
equal(additions, 2, 'frame sampling does not duplicate subscriptions');
const gyro = [...subscribed.get(2)].find(callback => callback !== externalCallback);
const gravity = [...subscribed.get(257)][0];
for (let index = 0; index <= 12; index++) {
  clock = index * 1000 / 120;
  gravity({ x: 0, y: 9.8, z: 0, timestamp: Math.round(clock * 1e6) });
  gyro({ x: 0, y: -1, z: 0, timestamp: Math.round(clock * 1e6) });
}
checks++; assert.ok(motion.apply(emptyFiveStarOutput(), 'onFoot', false, settings, .1, true).rightX > .5,
  'real adapter forwards SDK samples into frame output');
motion.stop();
equal([removals, displayCallbacks.size], [2, 0], 'stop destroys owned subscriptions');
equal([...subscribed.get(2)], [externalCallback], 'stop preserves other features sensor callbacks');
clock = 150;
gravity({ x: 9.8, y: 0, z: 0, timestamp: 150e6 });
gyro({ x: 0, y: -2, z: 0, timestamp: 150e6 });
motion.apply(emptyFiveStarOutput(), 'onFoot', false, settings, .016, true);
clock = 160;
gravity({ x: 0, y: 9.8, z: 0, timestamp: 160e6 });
gyro({ x: 0, y: -2, z: 0, timestamp: 160e6 });
clock = 170;
gyro({ x: 0, y: -2, z: 0, timestamp: 170e6 });
equal(motion.apply(emptyFiveStarOutput(), 'onFoot', false, settings, .016, true).rightX, 0,
  'late callbacks from a previous subscription cannot reintroduce old rotation after restart');
motion.apply(emptyFiveStarOutput(), 'menu', false, settings, .016, true);
equal([...subscribed.get(2)], [externalCallback], 'opening menus unsubscribes motion');
settings.tiltSteer = 1;
motion.apply(emptyFiveStarOutput(), 'car', false, settings, .016, true);
motion.apply(emptyFiveStarOutput(), 'car', false, settings, .016, false);
equal([...subscribed.get(2)], [externalCallback], 'inactive input destroys tilt subscriptions too');
motion.stop(); motion.stop();
equal(removals, additions, 'repeated stop is idempotent');
console.log(`Five Star motion lifecycle: ${checks} assertions passed`);
