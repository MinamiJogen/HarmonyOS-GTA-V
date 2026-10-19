#!/usr/bin/env python3
"""Validate actual offscreen batch probe evidence; not game execution evidence."""
import argparse
from collections import defaultdict
import hashlib,json,re
from pathlib import Path


def main():
    ap=argparse.ArgumentParser();ap.add_argument('directory',type=Path);ap.add_argument('--individual-reset',action='store_true');ap.add_argument('--report',required=True,type=Path);args=ap.parse_args()
    root=args.directory;result=json.loads((root/'result.json').read_text());trace=[json.loads(s)for s in(root/'trace.jsonl').read_text().splitlines()];n=result['iterationsRequested']
    assert result['status']=='pass' and result['batched'] is True and 3<=n<=256
    for stage in ['batched-open-encoder-before-glyph-upload','batched-before-submit','batched-after-submit','batched-round-verified']:
        assert[x['detail']['iteration']for x in trace if x['stage']==stage]==list(range(n)),stage
    assert all(x['detail']['commandBuffers']==3 for x in trace if x['stage']=='batched-before-submit')
    polls=[x['detail']['submittedBeforePoll']for x in trace if x['stage']=='batched-before-poll'];expected_polls=[min(3,n-i)for i in range(0,n,3)];assert polls==expected_polls
    assert [x['detail']['submittedBeforePoll']for x in trace if x['stage']=='batched-after-poll']==expected_polls
    assert trace[-1]['stage']=='pass' and result['iterationsCompleted']==n
    assert result['pixelBytesChecked']==trace[-1]['pixelBytesChecked']==n*16*16*4
    assert trace[-1]['iterationsCompleted']==n
    stages=[x['stage']for x in trace];assert stages.index('after-full-upload-poll')<stages.index('batched-open-encoder-before-glyph-upload')
    assert stages.index('before-batched-resource-drop')<stages.index('after-batched-resource-drop')<stages.index('before-resource-drop')<stages.index('after-resource-drop-poll')<stages.index('pass')
    tile=bytearray()
    for y in range(16):
        for x in range(16):
            xx,yy=x+32,y+64;tile.extend([(xx*13+yy*7)&255,(xx*3+yy*17)&255,(xx^yy)&255,255])
    checksum=0xcbf29ce484222325
    for i in range(n):
        gx,gy=(i*3)%8,(i*5)%8;gw,gh=3+i%5,3+(i//3)%5
        for y in range(gh):
            for x in range(gw):
                off=((gy+y)*16+gx+x)*4;tile[off:off+4]=bytes([(i*37+x*11+y*17)&255,(i*53+x*19+y*7)&255,(i*29+x*3+y*23)&255,255])
        for byte in tile:checksum=((checksum^byte)*0x100000001b3)&((1<<64)-1)
    assert result['checksumFNV1a64']==trace[-1]['detail']['checksumFNV1a64']==f'{checksum:016x}'
    atlas=(root/'atlas-sync.txt').read_text();assert 'TRUNCATED' not in atlas
    epochs=defaultdict(set);generations=set()
    for cmd,epoch,generation in re.findall(r'command-begin cmd=(0x[0-9a-f]+) epoch=(\d+) poolGeneration=(\d+)',atlas):
        epochs[cmd].add(int(epoch));generations.add(int(generation))
    reused={cmd:sorted(values)for cmd,values in epochs.items()if len(values)>1}
    pool=(root/'pool-last.txt').read_text();final='phase=after-final-command-encoder-drop' in pool
    if args.individual_reset:
        assert final and 'generation=1' in pool and 'freeBuffers=16' in pool and 'action=final-command-encoder-drop' in pool
        assert reused and generations=={1}
    log=(root/'hilog.txt').read_text(errors='replace');errors=[line for line in log.splitlines()if'Validation Error'in line or'SYNC-HAZARD'in line or'VUID-'in line]
    report={'scope':'Independent validation of actual device result, full probe stage ledger and bounded HAL atlas trace. Does not establish full game safety.','result':'pass','evidenceDirectory':str(root),'iterations':n,'verifiedPixelBytes':n*1024,'commandBuffersPerSubmission':3,'totalUserCommandBuffers':n*3,'pollGroups':polls,'independentPythonChecksum':f'{checksum:016x}','teardownTraceBeforePass':True,'elapsedToPassMs':trace[-1]['elapsedMs'],'nativeVvlErrorLinesInRetainedLog':len(errors),'retainedHilogBytes':len(log.encode()),'nativeLayerActivation':'Not inferred from absent errors; parent owns separate activation evidence.','individualReset':{'requiredByValidation':args.individual_reset,'allObservedPoolGenerations':sorted(generations),'distinctObservedCommandBuffers':len(epochs),'rawCommandBuffersReusedAcrossEpochs':len(reused),'maxEpochUsesOfOneRawBuffer':max(map(len,epochs.values()),default=0),'reusedBuffers':reused,'finalPoolDropCheckpointReturned':final,'poolLastRecord':pool,'scope':'Reuse inferred from actual repeated raw command buffers with generation1 plus the reviewed source. Last-call file shows the last final destructor; completed whole-probe teardown requires all destructors in that path to return.'},'traceSHA256':hashlib.sha256((root/'trace.jsonl').read_bytes()).hexdigest(),'gameplayVerified':False}
    args.report.write_text(json.dumps(report,indent=2)+'\n');print(json.dumps({k:report[k]for k in ['result','iterations','verifiedPixelBytes','independentPythonChecksum','teardownTraceBeforePass','nativeVvlErrorLinesInRetainedLog']},indent=2));print(json.dumps({k:v for k,v in report['individualReset'].items()if k not in ['reusedBuffers','poolLastRecord','scope']},indent=2))


if __name__=='__main__':main()
