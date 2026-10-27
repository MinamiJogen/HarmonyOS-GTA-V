#!/usr/bin/env python3
"""Embed a hash-verified AOT candidate and immutable identity; preserve generic."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess


def digest(path):
    value = hashlib.sha256()
    with path.open('rb') as file:
        for chunk in iter(lambda: file.read(1024 * 1024), b''):
            value.update(chunk)
    return value.hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('artifact', type=Path)
    parser.add_argument('metadata', type=Path)
    parser.add_argument('object', type=Path)
    args = parser.parse_args()
    artifact, metadata, output = (path.resolve() for path in (args.artifact, args.metadata, args.object))
    data = json.loads(metadata.read_text())
    profile = data.get('isaProfile')
    if profile not in ('generic', 'lse'):
        parser.error('metadata requires an explicit generic/lse profile')
    expected = profile == 'lse'
    if (data.get('requestedIsaFlags') != {'has_lse': expected} or
            data.get('requiredCpuFeatures') != (['FEAT_LSE'] if expected else []) or
            data.get('target') != 'aarch64-unknown-linux-ohos' or
            data.get('memoryReservationBytes') != 5 * 1024**3 or
            data.get('memoryGuardBytes') != 65536 or
            data.get('memoryMayMove') is not False):
        parser.error('unexpected or incomplete candidate ABI/ISA/memory metadata')
    artifact_hash = digest(artifact)
    metadata_hash = digest(metadata)
    if data.get('artifactSha256') != artifact_hash or data.get('artifactBytes') != artifact.stat().st_size:
        parser.error('artifact size/SHA256 does not match its compiler sidecar')
    assembly = output.with_suffix('.S')
    manifest = output.with_suffix('.identity.json')
    for path in (output, assembly, manifest):
        if path.exists():
            parser.error(f'refusing to overwrite {path}')
    output.parent.mkdir(parents=True, exist_ok=True)
    escaped = str(artifact).replace('\\', '\\\\').replace('"', '\\"')
    text = f'''.section .text.mg_game_aot,"ax",@progbits
.balign 65536
.global mg_game_aot_start
.global mg_game_aot_end
.hidden mg_game_aot_start
.hidden mg_game_aot_end
mg_game_aot_start:
.incbin "{escaped}"
mg_game_aot_end:
.section .rodata.mg_game_aot_identity,"a",@progbits
'''
    for symbol, value in [('mg_game_aot_isa_profile', profile),
                          ('mg_game_aot_sha256', artifact_hash),
                          ('mg_game_aot_metadata_sha256', metadata_hash)]:
        text += f'''.global {symbol}
.hidden {symbol}
.type {symbol},@object
{symbol}:
.asciz "{value}"
.size {symbol},.-{symbol}
'''
    text += '.section .note.GNU-stack,"",@progbits\n'
    with assembly.open('x') as file:
        file.write(text)
    root = Path(__file__).resolve().parents[1]
    subprocess.run([str(root / 'scripts/ohos-clang.sh'), '-c', str(assembly), '-o', str(output)], check=True)
    if digest(artifact) != artifact_hash or digest(metadata) != metadata_hash:
        raise RuntimeError('candidate changed while embedding; object is not qualified')
    sdk = Path(os.environ.get('OHOS_SDK_NATIVE', '/Applications/DevEco-Studio.app/Contents/sdk/default/openharmony/native'))
    symbols = subprocess.run([str(sdk / 'llvm/bin/llvm-nm'), '--defined-only', str(output)], capture_output=True, text=True, check=True).stdout
    for name in ['mg_game_aot_start', 'mg_game_aot_end', 'mg_game_aot_isa_profile', 'mg_game_aot_sha256', 'mg_game_aot_metadata_sha256']:
        if name not in symbols:
            raise RuntimeError(f'embedded candidate is missing {name}')
    identity = {'schemaVersion': 1, 'isaProfile': profile, 'requiredCpuFeatures': data['requiredCpuFeatures'],
                'artifactPath': str(artifact), 'artifactBytes': artifact.stat().st_size, 'artifactSha256': artifact_hash,
                'metadataPath': str(metadata), 'metadataSha256': metadata_hash,
                'objectPath': str(output), 'objectSha256': digest(output), 'symbols': symbols.strip().splitlines(),
                'identitySource': 'read-only symbols in the same hash-verified object as the embedded AOT, independent of user config',
                'runtimeGate': 'Wasmtime49 serialized ISA compatibility check for has_lse; no compatibility bypass or detect_host_feature override in production',
                'performanceGainEstablished': False}
    with manifest.open('x') as file:
        json.dump(identity, file, ensure_ascii=False, indent=2)
        file.write('\n')
    print(json.dumps({'status': 'embedded_verified_candidate', 'object': str(output), 'manifest': str(manifest)}, ensure_ascii=False))


if __name__ == '__main__':
    main()
