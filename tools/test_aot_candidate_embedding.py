#!/usr/bin/env python3
"""Verify independent AOT identity, mismatch rejection and no clobber; no device."""
from pathlib import Path
import copy
import hashlib
import json
import os
import struct
import subprocess
import tempfile

ROOT=Path(__file__).resolve().parents[1]
BASE=ROOT/'evidence/native-30fps-800p-20261021/aot-lse'
TOOL=ROOT/'game-runtime/scripts/embed-aot-candidate.py'


def sections(path):
    raw=path.read_bytes()
    off=struct.unpack_from('<Q',raw,40)[0]
    size,count,index=struct.unpack_from('<HHH',raw,58)
    headers=[struct.unpack_from('<IIQQQQIIQQ',raw,off+i*size) for i in range(count)]
    string=headers[index];names=raw[string[4]:string[4]+string[5]]
    return {names[h[0]:names.find(b'\0',h[0])].decode():raw[h[4]:h[4]+h[5]] for h in headers}


def main():
    original=json.loads((BASE/'game-ohos-lse.metadata.json').read_text())
    cases=[]
    with tempfile.TemporaryDirectory(prefix='candidate-identity-') as temporary:
        directory=Path(temporary)
        for profile in ['generic','lse']:
            artifact=BASE/'atomic-fixture-v2'/f'atomic-{profile}.cwasm'
            raw=artifact.read_bytes();metadata=copy.deepcopy(original)
            metadata.update(isaProfile=profile,requestedIsaFlags={'has_lse':profile=='lse'},requiredCpuFeatures=['FEAT_LSE'] if profile=='lse' else [],artifactBytes=len(raw),artifactSha256=hashlib.sha256(raw).hexdigest())
            sidecar=directory/f'{profile}.json';sidecar.write_text(json.dumps(metadata))
            output=directory/f'{profile}.o'
            def run(meta,target):
                return subprocess.run(['python3',str(TOOL),str(artifact),str(meta),str(target)],capture_output=True,text=True)
            result=run(sidecar,output);assert result.returncode==0,result.stderr
            sec=sections(output);assert sec['.text.mg_game_aot']==raw
            identity=sec['.rodata.mg_game_aot_identity'].split(b'\0')
            assert identity[:3]==[profile.encode(),metadata['artifactSha256'].encode(),hashlib.sha256(sidecar.read_bytes()).hexdigest().encode()]
            before=output.read_bytes();assert run(sidecar,output).returncode!=0;assert output.read_bytes()==before
            cases.extend([f'{profile}: exact payload and read-only profile/hashes',f'{profile}: existing object rejected unchanged'])
            for name,change in [('sha_mismatch',{'artifactSha256':'0'*64}),('size_mismatch',{'artifactBytes':1}),('unknown_profile',{'isaProfile':'all'}),('isa_mismatch',{'requestedIsaFlags':{'has_lse':profile!='lse'}}),('memory_mismatch',{'memoryGuardBytes':0})]:
                bad=dict(metadata);bad.update(change);path=directory/f'{profile}-{name}.json';path.write_text(json.dumps(bad));target=directory/f'{profile}-{name}.o'
                result=run(path,target);assert result.returncode!=0,(name,result.stdout,result.stderr);assert not target.exists();assert not target.with_suffix('.S').exists()
                cases.append(f'{profile}: {name} rejected before embedding')
    report={'status':'pass','cases':cases,'count':len(cases),'scope':'real small OHOS AOT fixture objects and SDK assembler; no HAP/native archive or device execution'}
    (BASE/'embedding-tests.json').write_text(json.dumps(report,indent=2)+'\n')
    print(json.dumps({'status':'pass','cases':len(cases)}))

if __name__=='__main__':main()
