#!/usr/bin/env python3
"""Run one native-only flag trial using GTA V's own benchmark and preserve raw evidence.

The application must already contain the matching signed package. No game resources,
story saves, quality controls, source cap or AI algorithms are modified. A new engine
report plus closed/stable raw frame file is required; liveness is checked separately
from status-file freshness. A native-clock bracket includes startup/loading and is
never described as an exact benchmark-pass timestamp.
"""
import argparse
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import time

from collect_native_trial import HDC, FILES, AppFiles, add_device_file_options, parse_native_size, presentation_summary, summarize
from prepare_native_trial import nodes, launch_demo
from analyze_native_benchmark import parse_builtin, validate_fixed_native_features

BUNDLE = 'com.minami.harmonyruntimeprobe'
BENCHMARKS = FILES + 'saves/userdata/Documents/Rockstar Games/GTA V/Benchmarks/'
FLAGS = ('shared_memory_fastpath', 'cached_readback', 'occlusion_pool', 'subresource_proxies', 'pipeline_cache',
         'native_render_fastpath', 'io_handle_lru', 'pipeline_cache_read_only', 'inline_upload_borrow',
         'native_command_fastpath', 'skip_empty_init_transition', 'skip_empty_render_prepass',
         'pso_warmup', 'pso_warmup_read_only')
TRIAL_FEATURES = ('pipeline_cache', 'occlusion_pool', 'native_render_fastpath', 'io_handle_lru',
                  'inline_upload_borrow', 'native_command_fastpath', 'skip_empty_init_transition',
                  'skip_empty_render_prepass', 'pso_warmup')


def write_json(path, value):
    path.write_text(json.dumps(value, ensure_ascii=False, indent=2, allow_nan=False)+'\n')


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def checked_inventory(directory):
    """Hash every regular file; never suppress traversal/read errors or links."""
    if directory.is_symlink() or not directory.is_dir():
        raise RuntimeError('required inventory is not an accessible directory: '+str(directory))
    files, directories = [], []
    def walk(parent):
        with os.scandir(parent) as entries:
            for entry in sorted(entries, key=lambda item:item.name):
                path = Path(entry.path)
                if entry.is_symlink():
                    raise RuntimeError('inventory contains an unverified symbolic link: '+str(path))
                if entry.is_dir(follow_symlinks=False):
                    directories.append(path)
                    walk(path)
                elif entry.is_file(follow_symlinks=False):
                    files.append((str(path.relative_to(directory)),sha(path)))
                else:
                    raise RuntimeError('inventory contains an unsupported entry: '+str(path))
    walk(directory)
    return files, directories


def capture_directory_inventory(receive, benchmarks, inventory, parent, parent_manifest,
                                provenance_path, direct=False):
    provenance = {'required':True,'primaryRemote':benchmarks,'primaryLocal':str(inventory),
                  'directColdFallbackAllowed':direct,'status':'pending'}
    try:
        try:
            receive(benchmarks,inventory)
        except RuntimeError as error:
            provenance['primaryError']=str(error)
            if not direct:
                raise
            # A failed Benchmarks read never means empty. First require a
            # successful complete parent transfer into a separate fresh tree.
            parent_remote=benchmarks.rstrip('/').rsplit('/',1)[0]+'/'
            provenance.update(parentRemote=parent_remote,parentLocal=str(parent))
            receive(parent_remote,parent)
            parent_files, directories=checked_inventory(parent)
            write_json(parent_manifest,[{'path':name,'sha256':digest} for name,digest in parent_files])
            provenance['parentManifestSHA256']=sha(parent_manifest)
            candidates=[path for path in directories if path.name=='Benchmarks']
            if any(Path(name).name=='Benchmarks' for name,_ in parent_files):
                raise RuntimeError('parent inventory contains a non-directory Benchmarks entry')
            if len(candidates)>1 or (candidates and str(candidates[0].relative_to(parent)) not in ('Benchmarks','GTA V/Benchmarks')):
                raise RuntimeError('parent inventory has ambiguous/unexpected Benchmarks location')
            if candidates:
                before,_=checked_inventory(candidates[0])
                provenance.update(status='required_parent_existing_benchmarks',baselineLocal=str(candidates[0]))
            else:
                if inventory.exists() and (not inventory.is_dir() or any(inventory.iterdir())):
                    raise RuntimeError('failed primary inventory has entries absent from successful parent; refusing incomplete baseline')
                before=[]
                provenance.update(status='required_parent_confirms_benchmarks_absent',baselineLocal=None)
        else:
            before,_=checked_inventory(inventory)
            provenance.update(status='primary_inventory_complete',baselineLocal=str(inventory))
        provenance['baselineFiles']=len(before)
        return set(before), Path(provenance['baselineLocal']) if provenance['baselineLocal'] is not None else None
    except Exception as error:
        provenance.update(status='rejected',error=str(error))
        raise
    finally:
        write_json(provenance_path,provenance)


def capture_before_inventory(receive, benchmarks, out, direct=False):
    before,_=capture_directory_inventory(receive,benchmarks,out/'benchmark-inventory-before',
        out/'inventory-parent',out/'inventory-parent-manifest.json',out/'benchmark-inventory-provenance.json',direct)
    return before


def fixed_feature_options(values, feature):
    fixed = {}
    for item in values:
        name, separator, value = item.partition('=')
        if not separator or value not in ('true', 'false'):
            raise ValueError('fixed feature requires NAME=true|false')
        if name in fixed:
            raise ValueError('fixed feature declared more than once: ' + name)
        fixed[name] = value == 'true'
    return validate_fixed_native_features(fixed, feature)


def benchmark_configuration(base, feature, enabled, trace, benchmark_pass, baseline_mode='legacy20',
                            fixed_flags=None, cpu_profile=False, render_prepass_diagnostics=False):
    if feature not in TRIAL_FEATURES:
        raise ValueError('unsupported independently controlled native feature')
    if not isinstance(enabled, bool) or not isinstance(trace, bool):
        raise ValueError('enabled and trace must be explicit booleans')
    fixed_flags = validate_fixed_native_features(fixed_flags, feature)
    if not isinstance(cpu_profile, bool):
        raise ValueError('cpu_profile must be an explicit boolean diagnostic')
    if not isinstance(render_prepass_diagnostics, bool):
        raise ValueError('render_prepass_diagnostics must be an explicit boolean diagnostic')
    if base.get('graphics_preset') != 'medium' or base.get('spatial_upscale_enabled') is not True or base.get('frame_interpolation_enabled') is not True:
        raise ValueError('require current medium/SR/FG baseline')
    if baseline_mode not in ('legacy20', 'unlimited-native-800p'):
        raise ValueError('explicit supported baseline mode required')
    if base.get('frame_interpolation_output_fps') != 40:
        raise ValueError('existing output40 configuration changed')
    if baseline_mode == 'legacy20' and base.get('fps_limit') != 24:
        raise ValueError('existing fps24/source20-output40 policy changed')
    if baseline_mode == 'unlimited-native-800p' and (type(base.get('fps_limit')) is not int or base.get('fps_limit') != 0 or
            base.get('spatial_upscale_native_short_edge') != 800):
        raise ValueError('unlimited baseline requires explicit fps0 and native short edge800')
    extra=base.get('extra_args')
    if not isinstance(extra,list) or not all(isinstance(x,str) for x in extra):
        raise ValueError('extra_args must be a string list')
    if any(x.lower().startswith('-benchmark') for x in extra):
        raise ValueError('baseline already contains benchmark options; use preserved pre-benchmark config')
    if baseline_mode == 'unlimited-native-800p' and any(
            x.lower().split('=', 1)[0] in ('-framelimit', '-fps', '-fpslimit') for x in extra):
        raise ValueError('unlimited baseline must not include a hidden command-line FPS limit')
    result={**base, **{flag:False for flag in FLAGS}, 'shared_memory_fastpath':True, **fixed_flags,
            feature:enabled, 'cpu_profile':cpu_profile,
            'render_prepass_diagnostics':render_prepass_diagnostics, 'frame_interval_trace':trace,
            'native_benchmark_monitor':True}
    result['extra_args']=['-benchmark','-benchmarkFrameTimes','-benchmarkIterations=1',f'-benchmarkPass={benchmark_pass}',*extra]
    return result


def complete_report(path, expected=None):
    """Bind one observed report pass; CLI selection and report IDs may differ."""
    text=path.read_text(encoding='utf-8-sig')
    if '=== SETTINGS ===' not in text or not re.search(r'MotionBlurStrength:\s*[\d.+-]+\s*$',text):
        return None
    table=parse_builtin(path)
    report_passes=[row['pass'] for row in table.get('passes',[])]
    if not table.get('recognized') or len(report_passes)!=1:
        return None
    if expected is not None and set(report_passes)!={str(p) for p in expected}:
        return None
    denominators={}
    for p,count in re.findall(r'Pass\s+(\d+):\s*\d+/(\d+)\s+frames',text):
        if p in denominators and denominators[p]!=int(count):raise ValueError('conflicting benchmark frame count')
        denominators[p]=int(count)
    if set(denominators)!=set(report_passes) or any(n<2 for n in denominators.values()):return None
    raw=[]
    # FrameTimes independently obtains its date, so do not require identical
    # timestamp suffixes. The caller supplies only fresh files; reject mixed
    # pass files as ambiguous rather than silently selecting one matching file.
    matches=list(path.parent.glob('FrameTimes-Pass*.txt'))
    if len(matches)!=1:return None
    for p in report_passes:
        if not matches[0].name.startswith(f'FrameTimes-Pass{p}-'):return None
        lines=matches[0].read_text().splitlines()
        if not lines or lines[0].strip()!='Frame\tTime(ms)':return None
        rows=[line.split() for line in lines[1:] if line.strip()]
        if len(rows)!=denominators[p]:return None
        if any(len(row)!=2 or int(row[0])!=i or not (0<float(row[1])<1e8) for i,row in enumerate(rows)):
            raise ValueError('invalid/non-contiguous raw frame times')
        raw.append((p,matches[0]))
    table['completePassTable']=True
    return {'table':table,'reportPasses':report_passes,'frameFiles':raw,'settings':text.split('=== SETTINGS ===',1)[1].strip()}


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--device',required=True);parser.add_argument('--hdc',default=HDC)
    parser.add_argument('--bundle',default=BUNDLE);parser.add_argument('--baseline-config',type=Path,required=True)
    parser.add_argument('--directory',type=Path,required=True);parser.add_argument('--package',type=Path,required=True)
    parser.add_argument('--feature',choices=TRIAL_FEATURES,required=True)
    parser.add_argument('--enabled',choices=('true','false'),required=True)
    parser.add_argument('--trace',choices=('true','false'),default='true')
    parser.add_argument('--fixed-feature',action='append',default=[],metavar='NAME=true|false',
                        help='repeat to fix another native boolean flag; cannot select graphics/diagnostics or the tested feature')
    parser.add_argument('--cpu-profile',choices=('true','false'),default='false',
                        help='explicit CPU diagnostic instrumentation; match within a pair')
    parser.add_argument('--render-prepass-diagnostics',choices=('true','false'),default='false',
                        help='explicit render-prepass diagnostic instrumentation; match within a pair')
    parser.add_argument('--baseline-mode',choices=('legacy20','unlimited-native-800p'),default='legacy20',
                        help='legacy20 preserves historical fps24/source20; unlimited requires explicit fps0/native800p')
    parser.add_argument('--benchmark-pass',type=int,default=0);parser.add_argument('--pair-id',required=True)
    parser.add_argument('--trial-id',required=True);parser.add_argument('--order',type=int,default=0)
    parser.add_argument('--workload-cache-state',default='same-pass-prior-processes; OS/driver cache state not reset')
    parser.add_argument('--scene-warmup-passes',type=int,default=0);parser.add_argument('--timeout',type=float,default=600)
    parser.add_argument('--hold-until-seconds',type=float,default=0,help='After report completion, keep process alive until this run age for asynchronous cache checkpoints; excluded from benchmark bracket')
    add_device_file_options(parser)
    args=parser.parse_args()
    if args.benchmark_pass<0 or args.timeout<30:parser.error('use a nonnegative single pass and timeout >=30s')
    try:
        fixed_flags=fixed_feature_options(args.fixed_feature,args.feature)
        files=AppFiles(args.bundle,args.file_transfer_mode,args.files_root)
        expected_size=parse_native_size(args.expected_native_size) if args.expected_native_size is not None else [1200,800]
        if args.baseline_mode=='legacy20' and expected_size != [1200,800]:
            raise ValueError('legacy20 requires its original1200x800 baseline')
    except ValueError as error:
        parser.error(str(error))
    out=args.directory.resolve();out.mkdir(parents=True,exist_ok=True)
    if list(out.iterdir()):raise ValueError('preserve prior evidence: use an empty trial directory')
    capture=out/'capture';capture.mkdir();prefix=[args.hdc,'-t',args.device]
    def call(*command,optional=False):
        r=subprocess.run(prefix+list(command),capture_output=True,text=True,timeout=30)
        if not optional and (r.returncode or (command[:2]==('file','recv') and '[Fail]' in r.stdout+r.stderr)):
            raise RuntimeError(r.stdout+r.stderr)
        return r.stdout
    def receive(remote,dest,optional=False):
        result=call(*files.command('recv',remote,dest,compressed=True),optional=optional)
        if not optional and (not dest.exists() or '[Fail]' in result):
            raise RuntimeError('required file inventory/readback failed: '+remote)
        return result
    def get_json(name,dest):
        receive(files.path(name),dest);return json.loads(dest.read_text())
    def pid_now():
        value=call('shell','pidof',args.bundle).strip()
        return int(value) if value.isdigit() else None
    base=json.loads(args.baseline_config.read_text());config=benchmark_configuration(base,args.feature,args.enabled=='true',args.trace=='true',args.benchmark_pass,args.baseline_mode,
                                                                                   fixed_flags=fixed_flags,cpu_profile=args.cpu_profile=='true',
                                                                                   render_prepass_diagnostics=args.render_prepass_diagnostics=='true')
    write_json(out/'requested-config.json',config)
    get_json('config.json',out/'config-preserved-before-run.json')
    call('shell','aa','force-stop',args.bundle)
    if pid_now() is not None:raise RuntimeError('prior application process still exists')
    inventory=out/'benchmark-inventory-before'
    benchmarks=files.path('saves/userdata/Documents/Rockstar Games/GTA V/Benchmarks/')
    before=capture_before_inventory(receive,benchmarks,out,direct=files.mode=='direct')
    write_json(out/'benchmark-inventory-before.json',[{'path':n,'sha256':h} for n,h in sorted(before)])
    call(*files.command('send',out/'requested-config.json',files.path('config.json')))
    call('shell','aa','start','-a','EntryAbility','-b',args.bundle)
    remote='/data/local/tmp/gta-builtin-home.json'
    launch_demo(lambda command:call(*command),out/'launcher-layout.json',remote,timeout=35,bundle=args.bundle)
    pid=pid_now()
    if pid is None:raise RuntimeError('application did not start')
    started=time.monotonic();deadline=started+args.timeout;last_clock=None;last_advance=started;index=0;samples=[];fgs=[];conditions=[];stable=None;stable_clock=None;selected=None;failure=None;first_clock=None;final_clock=None
    write_json(out/'run-start.json',{'pid':pid,'startedAtUtc':datetime.now(timezone.utc).isoformat(),'benchmarkPass':args.benchmark_pass,'scope':'CLI startup; not exact benchmark phase start'})
    try:
        while time.monotonic()<deadline:
            current=pid_now()
            if current!=pid:raise RuntimeError(f'benchmark process disappeared or changed: {pid} -> {current}')
            w=get_json('game-status.json',capture/f'status-{index:04d}.json');r=json.loads(w['runtimeJson'])
            if r.get('processId')!=pid:
                time.sleep(.5);continue
            native=r.get('nativeTelemetry',{});clock=native.get('monotonicNs')
            if args.baseline_mode=='unlimited-native-800p' and r.get('guestPresents',0)>0:
                if 'sourceTargetFps' not in native or native['sourceTargetFps'] is not None:
                    raise RuntimeError('unlimited run requires explicit native sourceTargetFps:null; missing/zero/positive is not equivalent')
            if isinstance(clock,int) and clock!=last_clock:
                if last_clock is not None and clock<last_clock:raise RuntimeError('native clock rewound')
                last_clock=clock;last_advance=time.monotonic();first_clock=first_clock or clock;final_clock=clock
                if r.get('guestPresents',0)>0 and r.get('renderSize') != expected_size:
                    raise RuntimeError('actual native size differs from expected native size')
                if r.get('renderSize')==expected_size:
                    samples.append({'pid':pid,'monotonicNs':clock,'guestPresents':r['guestPresents'],'presents':r['presents'],'renderSize':r['renderSize'],'nativeTelemetry':native})
                observed=w.get('uiBenchmarkConditions');conditions.append({'pid':pid,'monotonicNs':clock,'conditions':observed})
                if r.get('guestPresents',0)>0:
                    fg=get_json('frame-interpolation-status.json',capture/f'fg-{index:04d}.json')
                    if fg.get('pid')==pid:
                        source=fg.get('guestFinalSource') or {}
                        if [source.get('width'),source.get('height')] != expected_size:
                            raise RuntimeError('FG actual native texture differs from expected native size')
                        fgs.append(fg)
                    else:(capture/f'fg-{index:04d}.json').rename(out/f'startup-fg-other-pid-{index:04d}.json')
                    # A prior process's async export may remain during startup.
                    # Keep it on disk as evidence; never use it for statistics.
            if time.monotonic()-last_advance>20:raise RuntimeError('native status stale for >20s; refusing old-file progress')
            if r.get('error'):raise RuntimeError(r['error'])
            if r.get('appForeground') is False or r.get('presentationPaused'):raise RuntimeError('benchmark left foreground or paused')
            if index%3==0:
                stem=f'report-poll-{index:04d}'
                entries,inventory_root=capture_directory_inventory(receive,benchmarks,out/stem,
                    out/(stem+'-parent'),out/(stem+'-parent-manifest.json'),out/(stem+'-provenance.json'),
                    direct=files.mode=='direct')
                fresh=[inventory_root/name for name,digest in sorted(entries) if (name,digest) not in before]
                reports=[f for f in fresh if re.fullmatch(r'Benchmark-\d{2}(?:-\d{2}){5}\.txt',f.name)]
                if len(reports)>1:raise RuntimeError('multiple new benchmark reports; run identity ambiguous')
                if reports:
                    # Only this run's newly generated frame files enter matching.
                    selected_dir=out/'selected-results';selected_dir.mkdir(exist_ok=True)
                    import shutil
                    for f in selected_dir.iterdir():f.unlink()
                    for f in fresh:shutil.copyfile(f,selected_dir/f.name)
                    candidate=selected_dir/reports[0].name;detail=complete_report(candidate)
                    if detail:
                        fingerprint=(sha(candidate),tuple((p,sha(f)) for p,f in detail['frameFiles']))
                        if fingerprint==stable and isinstance(clock,int) and isinstance(stable_clock,int) and clock>stable_clock:
                            # Close the bracket with a new native snapshot acquired
                            # AFTER the stable artifact read, never the earlier poll.
                            time.sleep(1.2)
                            closing=get_json('game-status.json',out/'closing-status.json')
                            closing_runtime=json.loads(closing['runtimeJson'])
                            closing_clock=closing_runtime.get('nativeTelemetry',{}).get('monotonicNs')
                            if (closing_runtime.get('processId')==pid and isinstance(closing_clock,int)
                                    and closing_clock>clock and pid_now()==pid
                                    and closing_runtime.get('appForeground') is True
                                    and not closing_runtime.get('presentationPaused')
                                    and not closing_runtime.get('error')):
                                final_clock=closing_clock;selected=(candidate,detail);break
                        stable=fingerprint;stable_clock=clock
                    else:stable=None;stable_clock=None
                else:stable=None;stable_clock=None
            if index%10==0:print(json.dumps({'trial':args.trial_id,'pid':pid,'elapsedSeconds':round(time.monotonic()-started,1),'sourceFrames':r.get('guestPresents'),'sourceFps':native.get('currentSourceFps'),'stableReportObserved':stable is not None}),flush=True)
            index+=1;time.sleep(2)
        if selected is None:raise TimeoutError('no complete stable builtin report before timeout')
        if args.hold_until_seconds>0:
            while time.monotonic()-started<args.hold_until_seconds:
                if pid_now()!=pid:raise RuntimeError('process disappeared during post-benchmark cache hold')
                print(json.dumps({'trial':args.trial_id,'benchmarkComplete':True,'postCompletionCacheHoldSeconds':round(args.hold_until_seconds-(time.monotonic()-started),1)}),flush=True)
                time.sleep(min(10,max(0,args.hold_until_seconds-(time.monotonic()-started))))
    except Exception as error:
        failure=str(error)
    finally:
        get_json('config.json',capture/'config-after.json')
        write_json(capture/'config-before.json',config);write_json(capture/'samples.json',samples);write_json(out/'conditions.jsonl.json',conditions)
        with (out/'conditions.jsonl').open('w') as f:
            for row in conditions:f.write(json.dumps(row,ensure_ascii=False)+'\n')
        if len(samples)>=2:
            summary=summarize(samples);summary.update({'device':args.device,'bundle':args.bundle})
            if files.mode!='sandbox':summary['fileTransport']=files.conditions()
            if args.expected_native_size is not None:summary['nativeRenderSize']=expected_size
            if len(fgs)>=2:
                try:summary['presentations']=presentation_summary(fgs)
                except ValueError as error:summary['presentationCoverageError']=str(error)
            write_json(capture/'summary.json',summary)
        receive(files.path('game-runtime.log'),out/'runtime-all.log',optional=True)
        all_log=out/'runtime-all.log'
        if all_log.exists():
            data=all_log.read_bytes();begin=data.rfind(b'[config] effective -FrameQueueLimit=');(out/'runtime-session.log').write_bytes(data[begin:] if begin>=0 else data);all_log.unlink()
        if args.trace=='true':receive(files.path(f'native-frame-intervals-{pid}.jsonl'),out/'frame-intervals.jsonl',optional=True)
        if failure:write_json(capture/'error.json',{'error':failure,'pid':pid,'scope':'failed run; never accepted as a completed benchmark'})
    if failure:raise RuntimeError(failure)
    if json.loads((capture/'config-after.json').read_text())!=config:raise RuntimeError('application changed trial configuration')
    result,detail=selected;window={'clock':'CLOCK_MONOTONIC_ns','pid':pid,'startMonotonicNs':first_clock,'endMonotonicNs':final_clock}
    binding={'resultSha256':sha(result),'frameTimesSha256':{p:sha(f) for p,f in detail['frameFiles']},'window':window,'completed':True,'completionEvidence':'new report inventory, expected single pass, SETTINGS terminator, contiguous raw frame count matching report, and two identical file inventories six seconds apart; native bracket encloses startup/loading','exactPhaseBoundary':False,'postBenchmarkCacheHoldUntilRunSeconds':args.hold_until_seconds}
    write_json(out/'artifact-binding.json',binding)
    conditions={'packageSha256':sha(args.package),'gameSettingsSha256':hashlib.sha256(detail['settings'].encode()).hexdigest(),
                'queueDepth':3,'sourceTargetFps':None if args.baseline_mode=='unlimited-native-800p' else 20,
                'sceneWarmupPasses':args.scene_warmup_passes,'workloadCacheState':args.workload_cache_state,
                'instrumentation':{'frameIntervalTrace':args.trace=='true','cpuProfile':args.cpu_profile=='true',
                                   'renderPrepassDiagnostics':args.render_prepass_diagnostics=='true','conditionMonitor':True}}
    if files.mode!='sandbox':conditions['fileTransport']=files.conditions()
    if args.expected_native_size is not None:conditions['nativeRenderSize']=expected_size
    # Preserve historical/default manifests, including an in-flight old pair.
    if fixed_flags:
        conditions['fixedNativeFeatures']=dict(sorted(fixed_flags.items()))
    if args.baseline_mode=='unlimited-native-800p':
        conditions.update({'baselineMode':args.baseline_mode,'performanceGoal':{'averageFps':30,'onePercentLowFps':20}})
    manifest={'schemaVersion':1,'trialId':args.trial_id,'pairId':args.pair_id,'feature':args.feature,'enabled':args.enabled=='true','order':args.order,'captureDirectory':'capture','benchmark':{'kind':'gta_v_builtin','sceneId':f'commandline-benchmarkPass={args.benchmark_pass}','resultPath':str(result.relative_to(out)),'expectedPasses':detail['reportPasses'],'window':window,'windowScope':'enclosing_native_monotonic_bracket','bindingPath':'artifact-binding.json','frameTimes':[{'pass':p,'path':str(f.relative_to(out))} for p,f in detail['frameFiles']]},'conditions':conditions,'runtimeLogPath':'runtime-session.log','frameIntervalsPath':'frame-intervals.jsonl' if args.trace=='true' else None,'thermalPath':'conditions.jsonl','scope':'builtin raw frametimes primary; source cadence and host stage ledger are distinct; native-clock bracket includes startup/loading, not exact benchmark pass'}
    write_json(out/'manifest.json',manifest)
    print(json.dumps({'trial':args.trial_id,'completed':True,'pid':pid,'report':str(result),'rawFiles':[str(f) for _,f in detail['frameFiles']],'manifest':str(out/'manifest.json')},ensure_ascii=False),flush=True)


if __name__=='__main__':main()
