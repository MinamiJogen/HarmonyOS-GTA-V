#!/usr/bin/env python3
"""Host contracts for actual ETS launch preflight; no runtime/HAP/device build."""
import argparse
import hashlib
import importlib.util
import json
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile

sys.dont_write_bytecode = True
ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location("quality_contracts", ROOT / "tools/test_game_quality.py")
quality = importlib.util.module_from_spec(spec)
spec.loader.exec_module(quality)

EXTRA = r'''
const rootDir = path.resolve(sourceDir, '../../../../../../../..');
const indexFile = path.join(rootDir, 'harmony/entry/src/main/ets/pages/Index.ets');
const indexSource = fs.readFileSync(indexFile, 'utf8');
function method(name) {
  const start = indexSource.indexOf('  private ' + name + '(');
  assert.ok(start >= 0, 'missing production method ' + name);
  const end = indexSource.indexOf('\n  private ', start + 3);
  return indexSource.slice(start, end);
}
let nativeCaps = { runtimeLinked: true, superResolutionAvailable: true, frameGenerationAvailable: true };
let writes = [], nativeStarts = [];
const game = {
  gameCapabilities() { if (nativeCaps instanceof Error) throw nativeCaps; return nativeCaps; },
  configureGame(...args) { nativeStarts.push(args); }, stopGame() { throw new Error('preflight must not stop the game'); }
};
const launchModule = { exports: {} };
const launchSource = 'export class Launch {\n' + ['refreshProcessingCapabilities', 'beginGame', 'selectQuality'].map(method).join('\n') + '\n}';
const output = ts.transpileModule(launchSource, { compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2020 } });
vm.runInNewContext(output.outputText, { module: launchModule, exports: launchModule.exports, game,
  requireGameQualityCapabilities: fresh().requireGameQualityCapabilities,
  applyGameDefaults() { writes.push('defaults'); }, applyGameQuality() { writes.push('quality'); },
  saveGameQuality(...args) { fresh().saveGameQuality(...args); writes.push('selection'); },
  inputDevice: { on() {} }, setInterval() { return 1; }, hilog: { error() {} }
});
function launch(request = true) {
  writes = []; nativeStarts = [];
  const x = new launchModule.exports.Launch();
  Object.assign(x, { importing: false, selectingPackage: false, resourceChecking: false,
    resourceReady: true, runtimeLinked: true, gamePageVisible: false, saveBusy: false,
    qualityPreset: 'medium', superResolution: true, frameGeneration: request, filesDirectory: app({
      graphics_preset: 'medium', spatial_upscale_enabled: true, frame_interpolation_enabled: request, untouched: 'sentinel'
    }), inputListener() {}, closeSavePicker() {}, readSafeArea() {}, refreshTouchState() {}, refreshStatus() {},
    getUIContext() { return { getPromptAction() { return { showToast() {} }; } }; }
  });
  return x;
}
for (const preset of ['low', 'medium', 'high']) for (const sr of [false, true]) for (const fg of [false, true]) {
  test('capable launch selection ' + [preset,sr,fg].join('/'), () => {
    fresh().requireGameQualityCapabilities(selection(preset,sr,fg), {runtimeLinked:true,superResolutionAvailable:true,frameGenerationAvailable:true});
  });
}
for (const action of ['demo', 'new', 'load']) test('unsupported FG ' + action + ' fails before any config write', () => {
  nativeCaps = {runtimeLinked:true,superResolutionAvailable:true,frameGenerationAvailable:false};
  const x=launch(true), configPath=path.join(x.filesDirectory,'config.json'), before=fs.readFileSync(configPath);
  x.beginGame(action, action==='load'?'userdata/Documents/Rockstar Games/GTA V/Default/SGTA50000':'');
  assert.deepEqual(writes,[]); assert.deepEqual(nativeStarts,[]); assert.deepEqual(fs.readFileSync(configPath),before);
  assert.equal(x.frameGeneration,true); assert.match(x.qualityMessage,/未编译/);
});
test('explicit FG off permits the same SR-only runtime', () => {
  nativeCaps={runtimeLinked:true,superResolutionAvailable:true,frameGenerationAvailable:false};
  const x=launch(false);x.beginGame('demo','');
  assert.deepEqual(writes,['defaults','quality']);assert.equal(nativeStarts.length,1);
  assert.equal(nativeStarts[0][3],false);assert.equal(x.gamePageVisible,true);
});
for (const caps of [new Error('missing bridge'),{runtimeLinked:true,superResolutionAvailable:true,frameGenerationAvailable:1}]) {
  test('unknown/malformed capability does not write config', () => {
    nativeCaps=caps;const x=launch(true);x.beginGame('demo','');
    assert.equal(x.capabilitiesKnown,false);assert.deepEqual(writes,[]);assert.deepEqual(nativeStarts,[]);
  });
}
test('unavailable request remains true until player explicitly switches it off', () => {
  nativeCaps={runtimeLinked:true,superResolutionAvailable:true,frameGenerationAvailable:false};
  const x=launch(true);x.refreshProcessingCapabilities();
  assert.equal(x.selectQuality('medium',true,false),true);
  assert.equal(read(x.filesDirectory).frame_interpolation_enabled,false);
  assert.equal(x.selectQuality('medium',true,true),false);
  assert.equal(read(x.filesDirectory).frame_interpolation_enabled,false);
});
test('missing runtime/SR capabilities reject requested processing', () => {
  assert.throws(()=>fresh().requireGameQualityCapabilities(selection(),{runtimeLinked:false,superResolutionAvailable:false,frameGenerationAvailable:false}));
  assert.throws(()=>fresh().requireGameQualityCapabilities(selection('medium',true,false),{runtimeLinked:true,superResolutionAvailable:false,frameGenerationAvailable:false}));
});
test('native producer/consumer ABI and live-state source contracts', () => {
  const ffi=fs.readFileSync(path.join(rootDir,'game-runtime/src/ffi.rs'),'utf8');
  const header=fs.readFileSync(path.join(rootDir,'game-runtime/include/muguet_game_runtime.h'),'utf8');
  const cpp=fs.readFileSync(path.join(rootDir,'harmony/entry/src/main/cpp/game_surface.cpp'),'utf8');
  const fg=fs.readFileSync(path.join(rootDir,'game-runtime/src/frame_interpolation.rs'),'utf8');
  assert.match(ffi,/fn mg_game_frame_generation_available\(\)->i32\s*\{\s*if cfg!\(feature="frame-interpolation"\) \{1\}else\{0\}/);
  assert.match(header,/int32_t mg_game_frame_generation_available\(void\);/);
  assert.match(cpp,/"gameCapabilities", nullptr, Capabilities/);
  assert.match(ffi,/value\["frameGenerationAvailable"\]/);assert.match(ffi,/value\["frameGenerationLive"\]/);
  assert.match(fg,/if success&&display\.kind=="ai"/);
  assert.match(fg,/let fresh=current&&now\.is_some_and/);
  assert.match(fg,/fn reset\([^]*?note_live\(if self.failed.is_some\(\)\{3\}else\{1\}\)/);
  assert.ok(!indexSource.includes('frame-interpolation-status.json'),'UI must not read a stale FG file');
});
'''


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--directory", type=Path)
    args = parser.parse_args()
    out = args.directory or Path(tempfile.mkdtemp(prefix="game-capabilities-contract-"))
    out.mkdir(parents=True, exist_ok=True)
    ts = quality.typescript_path(None)
    node = shutil.which("node") or "/Applications/DevEco-Studio.app/Contents/tools/node/bin/node"
    # Reuse the original 29 contracts as well as the new actual-launch contracts.
    harness = quality.HARNESS.replace("console.log(JSON.stringify({ ok: true", EXTRA + "\nconsole.log(JSON.stringify({ ok: true")
    # Pass the root explicitly; no environment application/device files are used.
    harness = harness.replace("const rootDir = path.resolve(sourceDir, '../../../../../../../..');", "const rootDir = " + json.dumps(str(ROOT)) + ";")
    (out / "test.cjs").write_text(harness)
    p = subprocess.run([node, str(out / "test.cjs"), str(ts), str(quality.CONFIG), str(out)], capture_output=True, text=True, timeout=60)
    if p.returncode:
        raise SystemExit(p.stderr or p.stdout)
    result = json.loads(p.stdout)
    result["scope"] = "Actual ETS quality + extracted current Index launch methods on isolated host mocks; source ABI/live-state assertions; no ArkUI compilation, Rust/native build, device or performance acceptance."
    result["sourceSha256"] = {str(path.relative_to(ROOT)): hashlib.sha256(path.read_bytes()).hexdigest() for path in [
        quality.CONFIG / "GameQuality.ets", ROOT / "harmony/entry/src/main/ets/pages/Index.ets",
        ROOT / "game-runtime/src/frame_interpolation.rs", ROOT / "game-runtime/src/ffi.rs"]}
    (out / "result.json").write_text(json.dumps(result, ensure_ascii=False, indent=2)+"\n")
    print(json.dumps(result, ensure_ascii=False, indent=2))


if __name__ == "__main__":
    main()
