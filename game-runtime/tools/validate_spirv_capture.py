#!/usr/bin/env python3
"""Check completeness/hash ledger, then validate every captured final module."""
import argparse
from concurrent.futures import ThreadPoolExecutor
import hashlib
import json
from pathlib import Path
import re
import struct
import subprocess
import sys


def main():
    ap=argparse.ArgumentParser()
    ap.add_argument('capture_root',type=Path,help='Directory containing full-capture-status.json and its session folder')
    ap.add_argument('--report',type=Path,required=True)
    ap.add_argument('--expected-version',help='Require the actual header of every captured module to match, e.g. 1.4')
    ap.add_argument('--validator',type=Path,default=Path('.tools/SPIRV-Tools/build/tools/spirv-val'))
    ap.add_argument('--disassembler',type=Path,default=Path('.tools/SPIRV-Tools/build/tools/spirv-dis'))
    args=ap.parse_args();root=args.capture_root.resolve();errors=[]
    status=json.loads((root/'full-capture-status.json').read_text())
    session_name=Path(status['directory']).name
    session=root/session_name
    records=[json.loads(s) for s in (session/'manifest.jsonl').read_text().splitlines() if s.strip()]
    if len(records)!=status['uniqueFiles']:errors.append('manifest count differs from uniqueFiles')
    if [r['counter'] for r in records]!=list(range(1,len(records)+1)):errors.append('manifest counters are not contiguous and unique')
    if len({r['sha256'] for r in records})!=len(records):errors.append('duplicate unique-module SHA in manifest')
    if len({r['file'] for r in records})!=len(records):errors.append('duplicate module filename in manifest')
    if status['attempts']!=status['uniqueFiles']+status['duplicateModules']+status['missedModules']+status['failedWrites']:
        errors.append('attempt ledger does not reconcile with unique/duplicate/missed/failed counts')
    if status['missedModules'] or status['failedWrites'] or status['truncated']:
        errors.append('capture reported missed modules, failed writes or truncation')
    named={r['file'] for r in records};present={p.name for p in session.glob('*.spv')}
    if named!=present:errors.append('SPIR-V directory files differ from manifest filenames')
    if sum(r['bytes'] for r in records)>status['retainedBytes']:errors.append('module byte total exceeds retained-byte accounting')
    args.report.parent.mkdir(parents=True,exist_ok=True)
    def check(r):
        f={'file':r['file'],'counter':r['counter'],'stage':r['stage'],'entryPoint':r['entryPoint'],'pipelineLabel':r['pipelineLabel'],'firstPipelineSequence':r['firstPipelineSequence'],'retainedMixed':r['retainedMixed'],'sha256Expected':r['sha256']}
        if Path(r['file']).name!=r['file'] or not re.fullmatch(r'module-\d+-[0-9a-f]{64}\.spv',r['file']):
            return {**f,'integrityError':'invalid module filename','validationExit':-1}
        p=session/r['file'];raw=p.read_bytes();digest=hashlib.sha256(raw).hexdigest()
        f.update(bytes=len(raw),sha256Actual=digest)
        if digest!=r['sha256'] or len(raw)!=r['bytes'] or len(raw)!=r['wordCount']*4 or len(raw)<20 or struct.unpack_from('<I',raw)[0]!=0x07230203:
            return {**f,'integrityError':'SHA/length/word-count/header mismatch','validationExit':-1}
        version=struct.unpack_from('<I',raw,4)[0]
        f.update(spirvVersion=f'{(version>>16)&255}.{(version>>8)&255}',spirvVersionWord=version)
        meta=json.loads(p.with_suffix('.json').read_text())
        if meta!=r:return {**f,'integrityError':'module sidecar differs from manifest','validationExit':-1}
        v=subprocess.run([str(args.validator),'--target-env','vulkan1.3',str(p)],text=True,capture_output=True)
        f.update(validationExit=v.returncode,validationOutput=v.stdout+v.stderr)
        if v.returncode:
            asm=args.report.parent/(p.stem+'.spvasm')
            d=subprocess.run([str(args.disassembler),'--raw-id',str(p),'-o',str(asm)],text=True,capture_output=True)
            f.update(disassemblyExit=d.returncode,assembly=str(asm))
        return f
    with ThreadPoolExecutor(max_workers=4) as pool:files=list(pool.map(check,records))
    errors.extend(f"module {f['counter']} integrity: {f['integrityError']}" for f in files if 'integrityError' in f)
    complete=not errors
    valid=all(f['validationExit']==0 for f in files)
    versions=sorted({f.get('spirvVersion','unreadable') for f in files})
    version_match=args.expected_version is None or all(f.get('spirvVersion')==args.expected_version for f in files)
    report={'scope':'Exact final words captured immediately before vkCreateShaderModule. Capture completeness is checked before claiming coverage; validation does not establish vendor execution or gameplay.','session':str(session),'captureStatus':status,'completenessErrors':errors,'allActualModuleAttemptsCovered':complete,'allModuleIntegrityChecksPassed':all('integrityError'not in f for f in files),'modulesValidated':len(files),'allModulesValid':valid,'actualSpirvVersions':versions,'expectedSpirvVersion':args.expected_version,'expectedVersionMatches':version_match,'result':'pass' if complete and valid and version_match else 'fail','files':files}
    args.report.write_text(json.dumps(report,indent=2)+'\n')
    print(json.dumps({'result':report['result'],'allActualModuleAttemptsCovered':complete,'modulesValidated':len(files),'invalidModules':sum(f['validationExit']!=0 for f in files),'actualSpirvVersions':versions,'expectedVersionMatches':version_match,'completenessErrors':errors,'report':str(args.report)},indent=2))
    return 0 if report['result']=='pass' else 1
if __name__=='__main__':sys.exit(main())
