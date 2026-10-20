#!/usr/bin/env python3
"""Run the actual GameQuality ETS functions with a private host filesystem.

This checks configuration contracts, including failure preservation. It does
not compile ArkUI, execute the original engine, or establish device performance.
No application or device files are accessed. Node and an installed TypeScript
module are required; DevEco's bundled TypeScript is discovered without npm.
"""

import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]
CONFIG = ROOT / "harmony/entry/src/main/ets/config"

HARNESS = r'''
'use strict';
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');
const ts = require(process.argv[2]);
const sourceDir = process.argv[3];
const workDir = process.argv[4];
let fault = null;
let serial = 0;
const cases = [];
const operations = [];
const logs = [];

const fileIo = {
  OpenMode: { WRITE_ONLY: 1, CREATE: 2, TRUNC: 4 },
  accessSync(p) { return fs.existsSync(p); },
  readTextSync(p) { return fs.readFileSync(p, 'utf8'); },
  openSync(p) {
    operations.push(['open', p]);
    if (fault === 'open') throw new Error('injected open failure');
    return { fd: fs.openSync(p, 'w', 0o600) };
  },
  writeSync(fd, text) {
    if (fault === 'write') throw new Error('injected write failure');
    const bytes = Buffer.from(text);
    return fs.writeSync(fd, fault === 'short-write' ? bytes.subarray(0, 7) : bytes);
  },
  fsyncSync(fd) {
    if (fault === 'fsync') throw new Error('injected fsync failure');
    fs.fsyncSync(fd);
  },
  closeSync(file) { fs.closeSync(file.fd); },
  renameSync(from, to) {
    operations.push(['rename', from, to]);
    if (fault === 'rename') throw new Error('injected rename failure');
    fs.renameSync(from, to);
  }
};

function loadModule(name) {
  const filename = path.join(sourceDir, name + '.ets');
  const result = ts.transpileModule(fs.readFileSync(filename, 'utf8'), {
    fileName: name + '.ts', reportDiagnostics: true,
    compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2020 }
  });
  const errors = (result.diagnostics || []).filter(d => d.category === ts.DiagnosticCategory.Error);
  assert.equal(errors.length, 0, ts.formatDiagnosticsWithColorAndContext(errors, {
    getCanonicalFileName: f => f, getCurrentDirectory: () => sourceDir, getNewLine: () => '\n'
  }));
  const module = { exports: {} };
  vm.runInNewContext(result.outputText, {
    module, exports: module.exports,
    require(id) {
      if (id === '@kit.CoreFileKit') return { fileIo };
      if (id === '@kit.PerformanceAnalysisKit') return { hilog: { info(...args) { logs.push(args); } } };
      throw new Error('unexpected ETS dependency: ' + id);
    }
  }, { filename });
  return module.exports;
}

function fresh(name = 'GameQuality') { return loadModule(name); }
function plain(value) { return JSON.parse(JSON.stringify(value)); }
function test(name, run) { run(); cases.push(name); }
function app(original) {
  const dir = path.join(workDir, 'app-' + String(++serial).padStart(3, '0'));
  fs.mkdirSync(dir);
  if (original !== undefined) fs.writeFileSync(path.join(dir, 'config.json'),
    typeof original === 'string' ? original : JSON.stringify(original));
  return dir;
}
function read(dir) { return JSON.parse(fs.readFileSync(path.join(dir, 'config.json'), 'utf8')); }
function selection(preset = 'medium', superResolution = true, frameGeneration = true) {
  return { preset, superResolution, frameGeneration };
}
function argMap(args) {
  const entries = args.map(arg => {
    const separator = arg.indexOf('=');
    return [arg.slice(0, separator).toLowerCase(), arg.slice(separator + 1)];
  });
  assert.equal(new Set(entries.map(([key]) => key)).size, entries.length, 'duplicate managed argument');
  return Object.fromEntries(entries);
}
const expected = {
  low: { texturequality: '0', shaderquality: '1', shadowquality: '-1', shadowsoftness: '0',
    reflectionquality: '0', reflectionblur: '0', fxaa: '0', anisotropicqualitylevel: '0',
    ssao: '0', particlequality: '0', waterquality: '0', grassquality: '0', postfx: '0',
    lodscale: '0', pedlodbias: '0', vehiclelodbias: '0', pedvariety: '0', vehiclevariety: '0' },
  medium: { texturequality: '1', shaderquality: '1', shadowquality: '0', shadowsoftness: '1',
    reflectionquality: '0', reflectionblur: '1', fxaa: '1', anisotropicqualitylevel: '4',
    ssao: '0', particlequality: '0', waterquality: '0', grassquality: '1', postfx: '0',
    lodscale: '0.1', pedlodbias: '0', vehiclelodbias: '0', pedvariety: '1', vehiclevariety: '1' },
  high: { texturequality: '2', shaderquality: '2', shadowquality: '2', shadowsoftness: '1',
    reflectionquality: '1', reflectionblur: '1', fxaa: '1', anisotropicqualitylevel: '8',
    ssao: '1', particlequality: '1', waterquality: '1', grassquality: '2', postfx: '1',
    lodscale: '0.35', pedlodbias: '0.1', vehiclelodbias: '0.1', pedvariety: '1', vehiclevariety: '1' }
};
function assertPreset(config, preset) {
  const args = argMap(config.extra_args.filter(a => /^-[^=]+=/.test(a)));
  for (const [key, value] of Object.entries(expected[preset])) assert.equal(args['-' + key], value, preset + ' ' + key);
  for (const key of ['reflectionmsaa', 'multisample', 'multisamplequality', 'samplingmode', 'txaa',
    'tessellation', 'citydensity', 'maxlodscale', 'motionblurstrength', 'ultrashadows',
    'particleshadows', 'shadowlongshadows', 'shadowdisablescreensizecheck', 'fogvolumes', 'hdstreaminginflight', 'noingamedof']) {
    assert.equal(args['-' + key], '0', preset + ' fixed ' + key);
  }
  assert.equal(args['-shadowdistance'], '1');
  assert.equal(config.texture_quality, Number(expected[preset].texturequality));
}

test('missing config defaults to medium with independent SR and FG enabled without writing', () => {
  const dir = app();
  assert.deepEqual(plain(fresh().loadGameQuality(dir)), selection());
  assert.deepEqual(fs.readdirSync(dir), []);
});

test('unknown preset and non-boolean saved values use documented defaults', () => {
  const dir = app({ graphics_preset: 'ultra', spatial_upscale_enabled: 'false', frame_interpolation_enabled: 0 });
  assert.deepEqual(plain(fresh().loadGameQuality(dir)), selection());
});

for (const preset of ['low', 'medium', 'high']) {
  for (const sr of [false, true]) for (const fg of [false, true]) {
    test('persist/reload/apply ' + preset + ' SR=' + sr + ' FG=' + fg, () => {
      const original = { unknown: { nested: ['kept', 7] }, start_mode: 1, mute: true,
        extra_args: ['-savegamepc_tty=debug1', '-nonQuality=keep'], texture_quality: 99 };
      const dir = app(original);
      const value = selection(preset, sr, fg);
      fresh().saveGameQuality(dir, value);
      const saved = read(dir);
      assert.deepEqual(saved.extra_args, original.extra_args, 'selection must not apply engine options yet');
      assert.equal(saved.texture_quality, 99);
      assert.deepEqual(plain(fresh().loadGameQuality(dir)), value, 'fresh module models process restart');
      fresh().applyGameQuality(dir, value);
      const config = read(dir);
      assert.deepEqual(plain(fresh().loadGameQuality(dir)), value);
      assert.equal(config.spatial_upscale_enabled, sr);
      assert.equal(config.frame_interpolation_enabled, fg);
      assert.deepEqual(config.unknown, original.unknown);
      assert.equal(config.start_mode, 1);
      assert.equal(config.mute, true);
      assert.deepEqual(config.extra_args.slice(0, 2), original.extra_args);
      assert.equal(config.scale, 1);
      assert.equal(config.spatial_upscale_native_short_edge, 800);
      assert.equal(config.spatial_upscale_output_short_edge, 1080);
      assert.equal(config.spatial_upscale_match_display, true);
      assert.equal(config.spatial_upscale_sharpness, 0.1);
      assert.equal(config.frame_interpolation_output_fps, 40);
      assertPreset(config, preset);
      assert(!fs.existsSync(path.join(dir, 'config.json.quality.tmp')));
    });
  }
}

test('switching every preset pair removes stale mixed-case managed arguments and preserves unrelated data', () => {
  for (const from of ['low', 'medium', 'high']) for (const to of ['low', 'medium', 'high']) {
    const quality = fresh();
    const old = quality.gameQualityArguments(from).map(arg => arg.split('=')[0].toUpperCase() + '=999');
    const unrelated = ['-savegamepc_tty=debug1', '-loadGame=save/path', '-shadowQualityBonus=keep', '--untouched', ''];
    const dir = app({ extra_args: [...old, '-TeXtUrEqUaLiTy=999', ...unrelated, 123, null],
      unknown: { token: 'preserve' }, resolution: '1200x800', selected_save: 'unchanged' });
    quality.applyGameQuality(dir, selection(to, false, true));
    const config = read(dir);
    assert.deepEqual(config.extra_args.slice(0, unrelated.length), unrelated);
    assert.equal(config.extra_args.length, unrelated.length + quality.gameQualityArguments(to).length);
    assert.equal(config.unknown.token, 'preserve');
    assert.equal(config.resolution, '1200x800');
    assert.equal(config.selected_save, 'unchanged');
    assertPreset(config, to);
  }
});

test('legacy explicit SR sizes are removed before adding adaptive short-edge sizes', () => {
  const dir = app({ spatial_upscale_input_size: [960, 540], spatial_upscale_output_size: [1920, 1080], extra_args: [] });
  fresh().applyGameQuality(dir, selection());
  const config = read(dir);
  assert(!Object.hasOwn(config, 'spatial_upscale_input_size'));
  assert(!Object.hasOwn(config, 'spatial_upscale_output_size'));
  assert.equal(config.spatial_upscale_native_short_edge, 800);
});

test('one-time defaults followed by quality application retains persisted off switches and selection across restart', () => {
  const dir = app({ custom: 'kept', spatial_upscale_input_size: [960, 540], spatial_upscale_output_size: [1920, 1080] });
  fresh().saveGameQuality(dir, selection('low', false, false));
  const selected = plain(fresh().loadGameQuality(dir));
  fresh('GameDefaults').applyGameDefaults(dir);
  fresh().applyGameQuality(dir, selected);
  assertPreset(read(dir), 'low');
  assert.deepEqual(plain(fresh().loadGameQuality(dir)), selected);
  assert.equal(read(dir).custom, 'kept');
  const before = fs.readFileSync(path.join(dir, 'config.json'));
  fresh('GameDefaults').applyGameDefaults(dir);
  assert.deepEqual(fs.readFileSync(path.join(dir, 'config.json')), before, 'repeat defaults must not reset choices');
});

for (const invalid of ['{ broken JSON', 'null', '[]', 'true', '3', '"text"']) {
  test('invalid existing config stays byte-exact: ' + invalid, () => {
    const dir = app(invalid);
    const before = fs.readFileSync(path.join(dir, 'config.json'));
    for (const action of ['loadGameQuality', 'saveGameQuality', 'applyGameQuality']) {
      assert.throws(() => fresh()[action](dir, selection()));
      assert.deepEqual(fs.readFileSync(path.join(dir, 'config.json')), before);
    }
    assert.deepEqual(fs.readdirSync(dir), ['config.json']);
  });
}

test('unknown requested preset fails before opening a temporary file', () => {
  const dir = app({ original: true });
  const before = fs.readFileSync(path.join(dir, 'config.json'));
  for (const action of ['saveGameQuality', 'applyGameQuality']) assert.throws(() => fresh()[action](dir, selection('unknown')));
  assert.deepEqual(fs.readFileSync(path.join(dir, 'config.json')), before);
  assert.deepEqual(fs.readdirSync(dir), ['config.json']);
});

for (const failure of ['open', 'write', 'short-write', 'fsync', 'rename']) {
  test('failed ' + failure + ' preserves original configuration and previous selection', () => {
    for (const action of ['saveGameQuality', 'applyGameQuality']) {
      const dir = app({ graphics_preset: 'high', spatial_upscale_enabled: false, frame_interpolation_enabled: true,
        extra_args: ['-custom=keep'], original: ['exact bytes'] });
      const before = fs.readFileSync(path.join(dir, 'config.json'));
      fault = failure;
      try { assert.throws(() => fresh()[action](dir, selection('low', true, false))); }
      finally { fault = null; }
      assert.deepEqual(fs.readFileSync(path.join(dir, 'config.json')), before);
      assert.deepEqual(plain(fresh().loadGameQuality(dir)), selection('high', false, true));
      fresh()[action](dir, selection('low', true, false));
      assert.deepEqual(plain(fresh().loadGameQuality(dir)), selection('low', true, false), 'next attempt can replace stale tmp safely');
    }
  });
}

console.log(JSON.stringify({ ok: true, contracts: cases.length, cases,
  scope: 'Actual ETS functions transpiled for host execution with isolated filesystem and injected I/O failures; no ArkUI, engine, device or performance acceptance.',
  matrixCombinations: 12, presetTransitions: 9, sourceFiles: ['GameQuality.ets', 'GameDefaults.ets'],
  logEntries: logs.length, filesystemOperations: operations.length }, null, 2));
'''


def typescript_path(explicit):
    candidates = [explicit, os.environ.get("GAME_QUALITY_TYPESCRIPT"),
                  "/Applications/DevEco-Studio.app/Contents/tools/hvigor/hvigor-ohos-plugin/node_modules/typescript/lib/typescript.js",
                  "/Applications/DevEco-Studio.app/Contents/tools/ohpm/node_modules/typescript/lib/typescript.js",
                  "/opt/homebrew/lib/node_modules/typescript/lib/typescript.js",
                  "/usr/local/lib/node_modules/typescript/lib/typescript.js"]
    for value in candidates:
        if value and Path(value).is_file():
            return Path(value).resolve()
    raise SystemExit("TypeScript not found; pass --typescript /path/to/typescript.js (no dependencies are downloaded).")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--directory", type=Path, help="New private output directory; defaults to a temporary directory")
    parser.add_argument("--node", default=shutil.which("node"))
    parser.add_argument("--typescript", help="Installed TypeScript JS module path")
    args = parser.parse_args()
    if not args.node:
        raise SystemExit("Node.js not found; pass --node /path/to/node.")
    compiler = typescript_path(args.typescript)
    directory = args.directory or Path(tempfile.mkdtemp(prefix="game-quality-contract-"))
    if args.directory:
        directory.mkdir(parents=True, exist_ok=False)
    directory = directory.resolve()
    harness = directory / "test.cjs"
    harness.write_text(HARNESS)
    process = subprocess.run([str(args.node), str(harness), str(compiler), str(CONFIG), str(directory)],
                             capture_output=True, text=True, timeout=60)
    if process.returncode:
        raise SystemExit(process.stderr or process.stdout or f"Node exited {process.returncode}")
    result = json.loads(process.stdout)
    result["sourceSha256"] = {name: hashlib.sha256((CONFIG / name).read_bytes()).hexdigest()
                              for name in ("GameQuality.ets", "GameDefaults.ets")}
    result["testSha256"] = hashlib.sha256(Path(__file__).read_bytes()).hexdigest()
    result["typescript"] = str(compiler)
    result["directory"] = str(directory)
    (directory / "result.json").write_text(json.dumps(result, ensure_ascii=False, indent=2) + "\n")
    print(json.dumps(result, ensure_ascii=False, indent=2))


if __name__ == "__main__":
    main()
