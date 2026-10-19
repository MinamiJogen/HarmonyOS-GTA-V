#!/usr/bin/env python3
"""Offline ArrayStride review; original trace bytes remain untouched."""
import argparse
import hashlib
import json
import struct
import subprocess
from pathlib import Path

# Numeric opcodes/enumerants from Khronos SPIR-V core grammar.
OP_TYPE_ARRAY, OP_TYPE_RUNTIME_ARRAY, OP_TYPE_STRUCT, OP_TYPE_POINTER = 28, 29, 30, 32
OP_DECORATE, DEC_ARRAY_STRIDE, DEC_BLOCK = 71, 6, 2
SC_NAMES = {0:'UniformConstant',1:'Input',2:'Uniform',3:'Output',4:'Workgroup',6:'Private',7:'Function',9:'PushConstant',12:'StorageBuffer',5343:'ShaderRecordBufferKHR',5349:'PhysicalStorageBuffer'}
EXPLICIT = {2,9,12,5343,5349}
NON_EXPLICIT = {6,7,4}

def review(path, output, validator):
    raw=path.read_bytes()
    if len(raw)%4: raise ValueError(f'{path}: non-word byte length')
    words=struct.unpack('<'+'I'*(len(raw)//4),raw)
    if len(words)<5 or words[0]!=0x07230203: raise ValueError(f'{path}: invalid header')
    instructions=[]; types={}; pointers=[]; strides={}; block=set(); i=5
    while i<len(words):
        n=words[i]>>16; op=words[i]&0xffff
        if not n or i+n>len(words): raise ValueError(f'{path}: invalid instruction at word {i}')
        w=words[i:i+n]; instructions.append((i,op,w))
        if op in (OP_TYPE_ARRAY,OP_TYPE_RUNTIME_ARRAY): types[w[1]]=[w[2]]
        elif op==OP_TYPE_STRUCT: types[w[1]]=list(w[2:])
        elif op==OP_TYPE_POINTER: pointers.append((w[1],w[2],w[3]))
        elif op==OP_DECORATE and w[2]==DEC_ARRAY_STRIDE: strides[w[1]]=w[3]
        elif op==OP_DECORATE and w[2]==DEC_BLOCK: block.add(w[1])
        i+=n
    storage={}; explicit_workgroup=set()
    def walk(t, seen):
        if t in seen: return
        seen.add(t)
        for child in types.get(t,[]): walk(child,seen)
    for _,sc,target in pointers:
        reached=set(); walk(target,reached)
        for t in reached: storage.setdefault(t,set()).add(sc)
        if sc==4 and target in block: explicit_workgroup.update(reached)
    removed=[]; kept=[]; removable=set()
    for t,stride in strides.items():
        uses=storage.get(t,set())
        info={'typeId':t,'stride':stride,'storageClasses':[SC_NAMES.get(s,str(s)) for s in sorted(uses)]}
        # Conservatively preserve unknown/mixed classes and explicit Workgroup blocks.
        if uses and uses<=NON_EXPLICIT and t not in explicit_workgroup:
            removable.add(t); removed.append(info)
        else: kept.append(info)
    fixed=list(words[:5])
    for _,op,w in instructions:
        if op==OP_DECORATE and w[2]==DEC_ARRAY_STRIDE and w[1] in removable: continue
        fixed.extend(w)
    fixed_raw=struct.pack('<'+'I'*len(fixed),*fixed)
    output.parent.mkdir(parents=True,exist_ok=True); output.write_bytes(fixed_raw)
    valid=subprocess.run([str(validator),'--target-env','vulkan1.3',str(output)],text=True,capture_output=True)
    return {'input':str(path),'inputSHA256':hashlib.sha256(raw).hexdigest(),'output':str(output),'outputSHA256':hashlib.sha256(fixed_raw).hexdigest(),'spirvVersion':f'{(words[1]>>16)&255}.{(words[1]>>8)&255}','removedArrayStride':removed,'preservedArrayStride':kept,'removedWords':len(words)-len(fixed),'validationExit':valid.returncode,'validationOutput':valid.stdout+valid.stderr}

def main():
    ap=argparse.ArgumentParser();ap.add_argument('trace',type=Path);ap.add_argument('output',type=Path);ap.add_argument('--validator',type=Path,default=Path('.tools/SPIRV-Tools/build/tools/spirv-val')); a=ap.parse_args()
    report={'scope':'Offline candidate repair only; no runtime or original trace modifications. Only ArrayStride annotations exclusive to Private, Function or non-Block Workgroup pointers removed; explicit/mixed/unknown storage annotations remain.','specification':'https://docs.vulkan.org/spec/latest/chapters/interfaces.html','files':[]}
    for p in sorted(a.trace.rglob('*.spv')):
        report['files'].append(review(p,a.output/p.relative_to(a.trace),a.validator))
    report['result']='pass' if all(f['validationExit']==0 for f in report['files']) else 'fail'
    (a.output/'review.json').write_text(json.dumps(report,indent=2)+'\n')
    print(json.dumps({'result':report['result'],'files':len(report['files']),'changed':[{'file':f['input'],'removed':f['removedArrayStride'],'validationExit':f['validationExit']} for f in report['files'] if f['removedWords']],'report':str(a.output/'review.json')},indent=2))
if __name__=='__main__':main()
