#!/usr/bin/env python3
"""Host contract tests for the save-only importer, using synthetic header bytes.

These bytes are NOT fabricated game progress and never go to a device. Tests
prove ZIP/filesystem/rollback behavior, not original-engine payload compatibility.
"""
import argparse
import hashlib
import io
import json
import os
from pathlib import Path
import shutil
import stat
import struct
import subprocess
import tempfile
import zipfile

ROOT = Path(__file__).resolve().parents[1]
CPP = ROOT / "harmony/entry/src/main/cpp"
NATIVE = "userdata/Documents/Rockstar Games/GTA V/Default/"
SANDBOX = "sandbox/Documents/Rockstar Games/GTA V/Default/"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--directory", type=Path)
    parser.add_argument("--real-fixture", type=Path, help="Read-only original-engine save; only copies into the private test directory")
    args = parser.parse_args()
    directory = args.directory or Path(tempfile.mkdtemp(prefix="game-save-import-contract-"))
    directory.mkdir(parents=True, exist_ok=True)
    harness = directory / "main.cpp"
    harness.write_text(r'''
#include "game_save_import_core.h"
#include <fcntl.h>
#include <iostream>
#include <unistd.h>
int main(int argc,char** argv) {
    if(argc<3)return 2;
    std::string mode=argv[1]; game_saves::Result r;
    if(mode=="list")r=game_saves::List(argv[2]);
    else {
        if(argc!=4)return 2;
        int fd=open(argv[3],O_RDONLY|O_CLOEXEC); if(fd<0)return 3;
        if(mode=="fail1")game_saves::FailAfterCommitsForTesting(1);
        if(mode=="fail2")game_saves::FailAfterCommitsForTesting(2);
        game_saves::Limits limits;
        if(mode=="small")limits.maxFileBytes=300;
        r=game_saves::ImportZip(fd,argv[2],limits); close(fd);
    }
    std::cout<<game_saves::ToJson(r)<<"\n";
}
''')
    binary = directory / "contract"
    command = [shutil.which("clang++") or "c++", "-std=c++17", "-O1", "-g", "-Wall", "-Wextra", "-Werror",
               "-DGAME_SAVE_IMPORT_TESTING", "-fsanitize=address,undefined", "-fno-omit-frame-pointer",
               "-I", str(CPP), str(harness), str(CPP / "game_save_import_core.cpp"), "-lz", "-pthread", "-o", str(binary)]
    subprocess.run(command, check=True)
    passed = []
    sequence = 0

    def app():
        nonlocal sequence
        sequence += 1
        p = directory / f"app-{sequence:03}"
        p.mkdir()
        return p

    def blob(tag=b"contract-only", size=600):
        return b"\x00\x00\x00\x01" + bytes(260) + (tag * (size // len(tag) + 1))[:size-264]

    def archive(rows, compression=zipfile.ZIP_DEFLATED):
        nonlocal sequence
        sequence += 1
        p = directory / f"input-{sequence:03}.zip"
        with zipfile.ZipFile(p, "w", compression=compression) as z:
            for name, value in rows:
                z.writestr(name, value)
        return p

    def run(a, z=None, mode="import"):
        cmd = [str(binary), "list" if z is None else mode, str(a)]
        if z is not None:
            cmd.append(str(z))
        proc = subprocess.run(cmd, check=True, capture_output=True, text=True)
        return json.loads(proc.stdout)

    def expect(name, condition):
        assert condition, name
        passed.append(name)

    a = app()
    expect("empty list is read-only", run(a)["ok"] and not (a / "saves").exists())
    payload = blob()
    z = archive([("SGTA50000", payload), ("SGTA50000.bak", blob(b"backup")), ("settings.xml", b"do not import")])
    r = run(a, z)
    expect("raw store imports main and bak, excludes settings", r["ok"] and r["imported"] == 2 and r["ignored"] == 1)
    expect("no-conflict message does not claim an old-slot backup",r["conflicts"]==0 and "已保留同名旧 slot 备份" not in r["warning"])
    expect("list emits only loadable main ID with mode and unix seconds", len(r["saves"]) == 1 and r["saves"][0]["id"] == NATIVE+"SGTA50000" and r["saves"][0]["mode"] == 1 and 1000000000 < r["saves"][0]["mtime"] < 2000000000)
    expect("actual bytes and private permissions", (a/"saves"/(NATIVE+"SGTA50000")).read_bytes() == payload and stat.S_IMODE((a/"saves"/(NATIVE+"SGTA50000")).stat().st_mode) == 0o600)
    for wrapper in ("Profiles/ABCDEF12/", "Rockstar Games/GTA V/Profiles/ABCDEF12/", "Default/", "Rockstar Games/GTA V/Default/"):
        b = app(); r = run(b, archive([(wrapper, b""), (wrapper+"SGTA50001", payload)]))
        expect("container "+wrapper, r["ok"] and r["saves"][0]["id"] == NATIVE+"SGTA50001")
    b = app()
    r = run(b, archive([("SGTA50000.bak", payload)]))
    expect("backup-only ZIP rejected without inventing a main save", not r["ok"] and not (b/"saves").exists())
    r = run(b, archive([("saves/"+NATIVE+"SGTA50002", payload), ("saves/"+SANDBOX+"SGTA50002", payload)]))
    expect("own-backup preserves two namespaces", r["ok"] and {x["mode"] for x in r["saves"]} == {1, 2} and len(r["saves"]) == 2)
    old = (a/"saves"/(NATIVE+"SGTA50000")).read_bytes()
    new = blob(b"replacement")
    r = run(a, archive([("SGTA50000", new)], zipfile.ZIP_STORED))
    expect("conflict preserves original backup then replaces atomically", r["ok"] and r["conflicts"] == 1 and (a/r["backupDirectory"]/"original-0").read_bytes() == old and (a/"saves"/(NATIVE+"SGTA50000")).read_bytes() == new and (a/r["backupDirectory"]/"COMMITTED").is_file())
    expect("conflict message correctly names retained old-slot backup","已保留同名旧 slot 备份" in r["warning"])
    for mode in ("fail1", "fail2"):
        os.utime(a/"saves"/(NATIVE+"SGTA50000"), (1600000000,1600000000))
        r = run(a, archive([("SGTA50000", payload), ("SGTA50003", payload)]), mode)
        expect(mode+" rollback old and newly-created slots", not r["ok"] and (a/"saves"/(NATIVE+"SGTA50000")).read_bytes() == new and not (a/"saves"/(NATIVE+"SGTA50003")).exists() and (a/r["backupDirectory"]/"original-0").read_bytes() == new)
        expect(mode+" rollback preserves prior mtime", int((a/"saves"/(NATIVE+"SGTA50000")).stat().st_mtime) == 1600000000)
    unreadable=a/"saves"/(NATIVE+"SGTA50005")
    unreadable.write_bytes(payload);unreadable.chmod(0)
    listing=run(a)
    expect("unreadable last slot does not poison readdir EOF",listing["ok"] and (os.geteuid()==0 or all(x["id"]!=NATIVE+"SGTA50005" for x in listing["saves"])))
    unreadable.chmod(0o600);unreadable.unlink()
    for name in ("../SGTA50000", "/SGTA50000", "x/../SGTA50000", "x\\SGTA50000", "x:SGTA50000", "x//SGTA50000", "SGTA50016", "REPLAY0000", "game.wasm", "data/foo.rpf"):
        b = app(); r = run(b, archive([(name, payload)]))
        expect("reject unsafe/unsupported "+name, not r["ok"] and not (b/"saves").exists())
    b = app()
    r = run(b, archive([("SGTA50000", payload), ("sgta50000", payload)]))
    expect("case collision rejected", not r["ok"] and not (b/"saves").exists())
    r = run(b, archive([("Profiles/AAA/SGTA50000", payload), ("Profiles/BBB/SGTA50001", payload)]))
    expect("multiple profiles rejected without mutation", not r["ok"] and not (b/"saves").exists())
    link = zipfile.ZipInfo("SGTA50000"); link.create_system = 3; link.external_attr = (stat.S_IFLNK | 0o777) << 16
    expect("ZIP symlink rejected", not run(b, archive([(link, b"../../victim")]))["ok"])
    (b/"saves").symlink_to(a/"saves", target_is_directory=True)
    before = (a/"saves"/(NATIVE+"SGTA50000")).read_bytes()
    r = run(b, archive([("SGTA50000", payload)]))
    expect("destination symlink cannot redirect writes", not r["ok"] and (a/"saves"/(NATIVE+"SGTA50000")).read_bytes() == before)
    c = app(); target = c/"saves"/NATIVE; target.mkdir(parents=True)
    (target/"SGTA50000").symlink_to(a/"saves"/(NATIVE+"SGTA50000"))
    expect("existing slot symlink rejected", not run(c, archive([("SGTA50000", payload)]))["ok"] and (a/"saves"/(NATIVE+"SGTA50000")).read_bytes() == before)
    for value in (b"tiny", bytes(600), b"\x01\x00\x00\x00"+bytes(596)):
        c = app(); r = run(c, archive([("SGTA50000", value)]))
        expect("minimum outer header rejects "+str(len(value))+"/"+value[:4].hex(), not r["ok"] and not (c/"saves").exists())
    c = app(); z = archive([("SGTA50000", payload)], zipfile.ZIP_STORED)
    raw = bytearray(z.read_bytes()); offset = 30+struct.unpack_from("<H",raw,26)[0]+struct.unpack_from("<H",raw,28)[0]
    raw[offset+300] ^= 1; z.write_bytes(raw)
    expect("payload CRC mismatch rejected before mutation", not run(c,z)["ok"] and not (c/"saves").exists())
    z = archive([("SGTA50000", payload)], zipfile.ZIP_STORED)
    raw = bytearray(z.read_bytes()); raw[30] = ord("X"); z.write_bytes(raw)
    expect("local/central filename mismatch rejected", not run(c,z)["ok"] and not (c/"saves").exists())
    expect("single-file cap enforced", not run(c,archive([("SGTA50000",payload)]),"small")["ok"])
    expect("compression bomb ratio rejected", not run(c,archive([("SGTA50000",b"\x00\x00\x00\x01"+bytes(2<<20))]))["ok"])
    z=directory/"zip64.zip"
    with zipfile.ZipFile(z,"w") as f:
        with f.open("SGTA50000","w",force_zip64=True) as out: out.write(payload)
    expect("ZIP64 explicitly rejected", not run(c,z)["ok"])
    expect("failed temporary payloads cleaned", all(not any(x.iterdir()) for x in (c/"save-backups",) if x.exists()))
    class Pipe(io.BytesIO):
        def seek(self,*args):
            raise OSError("not seekable")
    pipe=Pipe()
    with zipfile.ZipFile(pipe,"w",compression=zipfile.ZIP_DEFLATED) as f:f.writestr("SGTA50004",payload)
    z=directory/"descriptor.zip";z.write_bytes(pipe.getvalue())
    expect("bounded signed data descriptor ZIP accepted",run(app(),z)["ok"])
    raw=bytearray(z.read_bytes());raw[raw.find(b"PK\x07\x08")+4]^=1;z.write_bytes(raw)
    expect("data descriptor CRC conflict rejected",not run(app(),z)["ok"])
    z=archive([("SGTA50000",payload)])
    z.write_bytes(z.read_bytes()[:-8])
    expect("truncated central/end record rejected",not run(app(),z)["ok"])
    def title_hash(data,seed=0,signed=True):
        h=seed
        for c in data:
            if signed and c>=128:c-=256
            h=((h+c)*1025)&0xffffffff;h^=h>>6
        h=(h*9)&0xffffffff;h^=h>>11
        return (h*32769)&0xffffffff
    def title_blob(raw,corrupt_hash=False):
        assert len(raw)==256
        h=title_hash(raw,title_hash(struct.pack('<I',1)))
        return struct.pack('>I',1)+raw+struct.pack('>I',h^(1 if corrupt_hash else 0))+b'contract-only payload'
    def title_field(text):
        raw=text.encode('utf-16-le')+b'\0\0'
        assert len(raw)<=256
        return raw.ljust(256,b'\0')
    for label,raw,expected,corrupt in (
        ('ascii',title_field('(Autosave) Contract - 10/19/26 15:27:48'),'(Autosave) Contract - 10/19/26 15:27:48',False),
        ('unicode signed bytes',title_field('中文存档 é 😀'),'中文存档 é 😀',False),
        ('JSON escaping',title_field('Mission "quote" \\ path'),'Mission "quote" \\ path',False),
        ('bad title hash',title_field('corrupt metadata'),'SGTA50006',True),
        ('no NUL terminator',b'A\0'*128,'SGTA50006',False),
        ('unpaired high surrogate',b'\x00\xd8\0\0'+bytes(252),'SGTA50006',False),
        ('unpaired low surrogate',b'\x00\xdc\0\0'+bytes(252),'SGTA50006',False),
        ('trailing high surrogate',b'A\0'*127+b'\x00\xd8','SGTA50006',False),
        ('control newline',title_field('line\nbreak'),'SGTA50006',False),
        ('bidi control',title_field('name\u202eexe'),'SGTA50006',False),
        ('empty',bytes(256),'SGTA50006',False),
        ('127-unit boundary',title_field('A'*127),'A'*127,False),
    ):
        r=run(app(),archive([('SGTA50006',title_blob(raw,corrupt))]))
        expect('bounded metadata '+label,r['ok'] and r['saves'][0]['title']==expected)
    high=title_field('中文存档 é 😀')
    expect('signed-byte hash differs from unsigned-byte Jenkins',title_hash(high,title_hash(struct.pack('<I',1)))!=title_hash(high,title_hash(struct.pack('<I',1)),signed=False))
    real_result=None
    if args.real_fixture:
        raw=args.real_fixture.read_bytes();before=hashlib.sha256(raw).hexdigest()
        assert len(raw)>=264 and int.from_bytes(raw[:4],'big')==1
        expected_hash=title_hash(raw[4:260],title_hash(struct.pack('<I',1)))
        expect('real header hash matches inferred algorithm',expected_hash==int.from_bytes(raw[260:264],'big'))
        expected_title=raw[4:260].decode('utf-16-le',errors='strict').split('\0',1)[0]
        for container in ('','Profiles/ABCDEF12/','Rockstar Games/GTA V/Profiles/ABCDEF12/','Default/','saves/'+NATIVE,'saves/'+SANDBOX):
            b=app();name=container+'SGTA50015';r=run(b,archive([(name,raw),(name+'.bak',raw)]))
            expected_id=(SANDBOX if container=='saves/'+SANDBOX else NATIVE)+'SGTA50015'
            expect('real fixture import '+(container or 'raw'),r['ok'] and r['imported']==2 and len(r['saves'])==1 and r['saves'][0]['id']==expected_id and r['saves'][0]['title']==expected_title and hashlib.sha256((b/'saves'/expected_id).read_bytes()).hexdigest()==before)
        expect('original real fixture remained unchanged',hashlib.sha256(args.real_fixture.read_bytes()).hexdigest()==before)
        real_result={'source':str(args.real_fixture),'bytes':len(raw),'sha256':before,'headerBytes':264,'payloadBytes':len(raw)-264,'versionBE':1,'storedHashBE':int.from_bytes(raw[260:264],'big'),'inferredHash':expected_hash,'titleUTF16LE':expected_title,'boundary':'Real fixture host import and title/hash checks do not establish an original-engine load of the resulting progress. This ASCII real title alone does not distinguish signed-byte from unsigned-byte hashing.'}
    result={"passed":len(passed),"tests":passed,"compilerCommand":command,"scope":"ZIP/filesystem/metadata contracts under ASan+UBSan; synthetic metadata cases are not real game progress. Optional real-fixture checks remain host-only.","realFixture":real_result}
    (directory/"result.json").write_text(json.dumps(result,indent=2)+"\n")
    print(json.dumps({"passed":len(passed),"directory":str(directory),"scope":result["scope"]}))


if __name__ == "__main__":
    main()
