#!/usr/bin/env python3
"""Canonical explicit-buffer interface layout comparison, ignoring SPIR-V IDs."""
import json,struct,sys
from pathlib import Path

def layouts(path):
 raw=Path(path).read_bytes();w=struct.unpack('<'+'I'*(len(raw)//4),raw);types={};constants={};decor={};members={};variables=[];i=5
 while i<len(w):
  n=w[i]>>16;op=w[i]&65535;a=w[i+1:i+n]
  if not n:raise ValueError('zero instruction')
  if op in [19,20,21,22,23,24,28,29,30,32]:types[a[0]]=(op,a[1:])
  elif op==43:constants[a[1]]=list(a[2:])
  elif op==59 and a[2] in [2,9,12,5343,5349]:variables.append(a)
  elif op==71:decor.setdefault(a[0],{})[a[1]]=list(a[2:])
  elif op==72:members.setdefault((a[0],a[1]),{})[a[2]]=list(a[3:])
  i+=n
 def sig(id):
  op,a=types[id]
  if op in [19,20,21,22]:return [op,list(a)]
  if op in [23,24]:return [op,sig(a[0]),a[1]]
  if op==28:return [op,sig(a[0]),constants.get(a[1]),decor.get(id,{}).get(6)]
  if op==29:return [op,sig(a[0]),decor.get(id,{}).get(6)]
  if op==30:return [op,{str(k):v for k,v in decor.get(id,{}).items()if k in [2,3]},[[sig(t),{str(k):v for k,v in members.get((id,j),{}).items()if k in [4,5,7,35]}]for j,t in enumerate(a)]]
  if op==32:return [op,a[0],sig(a[1])]
  raise ValueError(f'unhandled buffer type {op}')
 out=[]
 for a in variables:
  ty,var,storage=a[:3];d=decor.get(var,{})
  out.append({'storage':storage,'descriptorSet':d.get(34),'binding':d.get(33),'type':sig(ty)})
 return sorted(out,key=lambda r:json.dumps([r['storage'],r['descriptorSet'],r['binding']]))
if __name__=='__main__':
 a,b=map(layouts,sys.argv[1:3]);print(json.dumps({'sameExplicitBufferLayouts':a==b,'left':a,'right':b},indent=2));sys.exit(0 if a==b else 1)
