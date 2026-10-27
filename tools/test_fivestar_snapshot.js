// Run after tsc of the FiveStar control modules into the supplied directory.
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const dir = path.resolve(process.argv[2] || '/private/tmp/fivestar-ts-controls');
const { parseFiveStarSnapshot, defaultFiveStarHints, FiveStarInput } = require(path.join(dir, 'FiveStarInput.js'));
let checks = 0;
function equal(actual, expected, label) { checks++; assert.deepEqual(actual, expected, label); }
function ok(value, label) { checks++; assert.ok(value, label); }
const wire = { hookReady: true, known: true, generation: 12, seq: 40, ageMs: 14,
  vehicle: 1, pauseMenu: false, status: 0, characterSwitchEnabled: true, missionInputs: [228, 229],
  hints: { ...defaultFiveStarHints(), specialShown: true, specialReady: true, specialFill: .7,
    canDetonate: true, helpShown: true, helpContext: true, helpInputs: 1 << 11,
    radioWheelOpen: true, vehicleModel: 0xB12314E0, pedModel: 0x1234, trailerAttached: true,
    passenger: true, taxiMenu: true, phoneBrowser: true } };
equal(parseFiveStarSnapshot(JSON.stringify(wire)), wire, 'every Rust wire field needed by the input model round trips');
// Independently compare the Serde camelCase field inventory with all TS default hint fields.
const rust = fs.readFileSync(path.join(__dirname, '../game-runtime/src/fivestar_state.rs'), 'utf8');
const body = rust.match(/struct Hints\s*\{([\s\S]*?)\n\}/)[1];
const snakeToCamel = name => name.replace(/_([a-z])/g, (_, c) => c.toUpperCase());
const fields = [...body.matchAll(/\b([a-z_]+):(?:bool|f32|u32)/g)].map(match => snakeToCamel(match[1])).sort();
equal(fields, Object.keys(defaultFiveStarHints()).sort(), 'backend Serde hint inventory exactly matches TS model');
const startup = parseFiveStarSnapshot(JSON.stringify({ ...wire, known: false, vehicle: 0, status: 16, hints: {} }));
equal(startup.known, false, 'missing ped remains unknown');
equal(startup.hints, defaultFiveStarHints(), 'partial hints close unavailable situational controls');
const engine = new FiveStarInput(); engine.update(startup, 1000);
ok(engine.visibleButtons().some(button => button.name === 'accept'), 'live hook without player has startup Accept');
const button = engine.visibleButtons().find(button => button.name === 'accept');
engine.touchDown(1, button.frame.x + button.frame.width / 2, button.frame.y + button.frame.height / 2, 1000);
ok(engine.sample(1000, 0).buttons !== 0, 'startup frontend navigation sends real pad input');
const missingAge = parseFiveStarSnapshot(JSON.stringify({ ...wire, ageMs: null }));
equal(missingAge.ageMs, Infinity, 'nullable unpublished age cannot become fresh zero');
engine.update(missingAge, 1001); equal(engine.sample(1001, 0).active, false, 'nullable age cannot sustain a held pad');
const malformed = parseFiveStarSnapshot(JSON.stringify({ ...wire, hints: { specialShown: 'yes', specialFill: 3,
  helpInputs: -1, vehicleModel: 'taxi', phoneBrowser: 1 }, missionInputs: [228, 228, 400, -1, 220.5, '229', 229] }));
equal(malformed.hints.specialShown, false, 'string values cannot invent situational availability');
equal(malformed.hints.specialFill, 1, 'ability progress is bounded');
equal([malformed.hints.helpInputs, malformed.hints.vehicleModel, malformed.hints.phoneBrowser], [0, 0, false], 'malformed hint values fail closed');
equal(malformed.missionInputs, [228, 229], 'mission poll IDs are bounded unique integers');
for (const value of ['null', '{}', 'broken', JSON.stringify({ ...wire, vehicle: -1 }), JSON.stringify({ ...wire, status: '0' })]) {
  equal(parseFiveStarSnapshot(value).hookReady, false, 'malformed snapshot cannot activate input');
}
console.log(`Five Star snapshot: ${checks} assertions passed`);
