#!/usr/bin/env python3
"""Exercise actual EntryAbility callbacks and C++ NAPI exports with host fakes.

Rust lifecycle behavior is checked separately by test_app_lifecycle.py. This
contract verifies both previously missing language bridges, not target OS event
delivery or device pause/resume performance.
"""
from pathlib import Path
import re
import subprocess
import tempfile
import sys

sys.dont_write_bytecode = True
from test_game_quality import typescript_path

ROOT = Path(__file__).resolve().parents[1]


def function(source, marker):
    start = source.index(marker)
    brace = source.index('{', start)
    depth = 1
    end = brace + 1
    while depth:
        depth += (source[end] == '{') - (source[end] == '}')
        end += 1
    return source[start:end]


CPP = r'''
#include <algorithm>
#include <cassert>
#include <iostream>
#include <map>
#include <string>
#include <vector>
#include "muguet_game_runtime.h"
struct Value { bool boolean; bool value; };
using napi_value = Value*;
using napi_env = void*;
struct Info { std::vector<napi_value> args; };
using napi_callback_info = Info*;
using napi_callback = napi_value (*)(napi_env,napi_callback_info);
enum napi_status { napi_ok, napi_boolean_expected };
constexpr int napi_default = 0;
struct napi_property_descriptor {
    const char* utf8name; napi_value name; napi_callback method;
    napi_callback getter; napi_callback setter; napi_value value; int attributes; void* data;
};
std::map<std::string,napi_callback> exports;
std::vector<bool> visibility, focus;
int errors=0;
Value undefined{false,false};
napi_status napi_get_cb_info(napi_env,napi_callback_info info,size_t* count,napi_value* args,void*,void*) {
    for(size_t i=0;i<std::min(*count,info->args.size());i++)args[i]=info->args[i];
    *count=info->args.size();return napi_ok;
}
napi_status napi_get_value_bool(napi_env,napi_value arg,bool* value) {
    if(!arg||!arg->boolean)return napi_boolean_expected;*value=arg->value;return napi_ok;
}
void napi_get_undefined(napi_env,napi_value* result){*result=&undefined;}
void napi_throw_type_error(napi_env,const char*,const char*){errors++;}
void napi_define_properties(napi_env,napi_value,size_t count,const napi_property_descriptor* methods) {
    for(size_t i=0;i<count;i++)exports[methods[i].utf8name]=methods[i].method;
}
extern "C" void mg_game_app_foreground(bool value){visibility.push_back(value);}
extern "C" void mg_game_focus(bool value){focus.push_back(value);}
bool ResourceImportGate(bool){return true;}
void SetGameResourceImportGate(bool (*)(bool)){}
void SetGameSaveImportGate(bool (*)(bool)){}
'''

CPP_TEST = r'''
int main(){
    ExportGameMethods(nullptr,nullptr);
    assert(exports.count("setAppForeground")==1);
    assert(exports.count("setInputFocus")==1);
    assert(exports["setAppForeground"]!=exports["setInputFocus"]);
    Value yes{true,true}, no{true,false}, wrong{false,true};
    Info foreground{{&yes}}, background{{&no}}, invalid{{&wrong}}, missing{{}}, extra{{&yes,&no}};
    auto app=exports["setAppForeground"];
    assert(app(nullptr,&foreground)==&undefined);assert(app(nullptr,&background)==&undefined);
    assert(app(nullptr,&foreground)==&undefined);
#if MG_HAS_GAME_RUNTIME
    assert(visibility==std::vector<bool>({true,false,true}));
#else
    assert(visibility.empty());
#endif
    assert(focus.empty());
    const auto calls=visibility.size();
    assert(app(nullptr,&invalid)==nullptr);assert(app(nullptr,&missing)==nullptr);assert(app(nullptr,&extra)==nullptr);
    assert(errors==3);assert(visibility.size()==calls);
    exports["setInputFocus"](nullptr,&background);
    assert(visibility.size()==calls);
#if MG_HAS_GAME_RUNTIME
    assert(focus==std::vector<bool>({false}));
#else
    assert(focus.empty());
#endif
    std::cout<<"Actual C++ NAPI visibility export passed, MG_HAS_GAME_RUNTIME="<<MG_HAS_GAME_RUNTIME<<"\n";
}
'''

ETS = r'''
const fs=require('fs'),vm=require('vm'),assert=require('assert/strict');
const ts=require(process.argv[2]);
const source=fs.readFileSync(process.argv[3],'utf8');
const compiled=ts.transpileModule(source,{reportDiagnostics:true,compilerOptions:{module:ts.ModuleKind.CommonJS,target:ts.ScriptTarget.ES2020}});
assert(!(compiled.diagnostics||[]).some(d=>d.category===ts.DiagnosticCategory.Error),'EntryAbility transpilation failed');
const calls=[],storage=new Map();let performanceFailure=false;
const game={setAppForeground(value){calls.push(['nativeVisibility',value]);},setInputFocus(value){calls.push(['inputFocus',value]);}};
const performance={gamePerformanceForeground(){calls.push(['performance',true]);if(performanceFailure)throw Error('report failure');},
  gamePerformanceBackground(){calls.push(['performance',false]);if(performanceFailure)throw Error('report failure');}};
const moduleRef={exports:{}};
vm.runInNewContext(compiled.outputText,{module:moduleRef,exports:moduleRef.exports,
  AppStorage:{setOrCreate(key,value){storage.set(key,value);calls.push(['storage',key,value]);}},
  require(name){
    if(name==='@kit.AbilityKit')return {UIAbility:class {}};
    if(name==='@kit.ArkUI')return {window:{}};
    if(name==='@kit.PerformanceAnalysisKit')return {hilog:{warn(){},info(){},error(){}}};
    if(name==='@kit.BasicServicesKit')return {systemDateTime:{}};
    if(name==='libharmony_probe.so')return {default:game};
    if(name==='@ohos.multimodalInput.pointer')return {default:{setPointerVisibleSync(value){calls.push(['pointerVisible',value]);}}};
    if(name==='./CrashEvidence')return {};
    if(name==='./GamePerformance')return performance;
    throw Error(name);
  }});
const Ability=moduleRef.exports.default;
const ability=new Ability();
// Direct visibility delivery must work before any game configure/start request.
ability.onBackground();assert.deepEqual(calls[0],['nativeVisibility',false]);
assert.equal(storage.get('gameAppForeground'),false);assert(calls.some(x=>x[0]==='inputFocus'&&x[1]===false));
calls.length=0;ability.onForeground();assert.deepEqual(calls[0],['nativeVisibility',true]);
assert.equal(storage.get('gameAppForeground'),true);assert(calls.some(x=>x[0]==='inputFocus'&&x[1]===true));
// UI focus release alone must never masquerade as application backgrounding.
calls.length=0;game.setInputFocus(false);assert.deepEqual(calls,[['inputFocus',false]]);
// Reporting failure cannot prevent the earlier native lifecycle transition.
calls.length=0;performanceFailure=true;assert.throws(()=>ability.onBackground(),/report failure/);
assert.deepEqual(calls[0],['nativeVisibility',false]);assert.equal(storage.get('gameAppForeground'),false);
console.log('Actual EntryAbility visibility-first lifecycle callbacks passed; native focus remains independent.');
'''


def main():
    cpp = (ROOT / 'harmony/entry/src/main/cpp/game_surface.cpp').read_text()
    declaration = (ROOT / 'harmony/entry/src/main/cpp/types/libharmony_probe/Index.d.ts').read_text()
    assert re.search(r'export const setAppForeground:\s*\(foreground: boolean\) => void;', declaration)
    exported = function(cpp, 'void ExportGameMethods(')
    methods = re.findall(r'\{"[^"]+", nullptr, (\w+),', exported)
    stubs = '\n'.join(f'napi_value {name}(napi_env,napi_callback_info){{return &undefined;}}' for name in set(methods) - {'AppForeground','InputFocus'})
    actual = '\n'.join(function(cpp, marker) for marker in ['napi_value Undefined(', 'napi_value InputFocus(', 'napi_value AppForeground('])
    # This change must preserve the existing terminal surface-destruction contract.
    destroyed = function(cpp, 'void OnDestroyed(')
    assert 'if (startDispatched) mg_game_stop();' in destroyed
    assert 'mg_game_app_foreground' not in destroyed and 'mg_game_surface_lost' not in destroyed
    with tempfile.TemporaryDirectory(prefix='app-lifecycle-bridge-', dir='/private/tmp') as directory:
        root = Path(directory)
        source = root / 'bridge.cpp'
        source.write_text(CPP + stubs + '\n' + actual + '\n' + exported + CPP_TEST)
        for linked in [0,1]:
            binary = root / f'bridge-{linked}'
            subprocess.run(['/usr/bin/clang++','-std=c++17',f'-DMG_HAS_GAME_RUNTIME={linked}',
                            '-I',str(ROOT/'game-runtime/include'),str(source),'-o',str(binary)],check=True)
            subprocess.run([str(binary)],check=True)
        harness = root / 'ability.cjs'
        harness.write_text(ETS)
        subprocess.run(['/Applications/DevEco-Studio.app/Contents/tools/node/bin/node',str(harness),str(typescript_path(None)),
                        str(ROOT/'harmony/entry/src/main/ets/entryability/EntryAbility.ets')],check=True)
    print('C++/ETS lifecycle bridge host contracts passed; target event delivery and actual background/resume remain device acceptance tasks.')


if __name__ == '__main__':
    main()
