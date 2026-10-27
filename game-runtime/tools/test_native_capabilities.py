#!/usr/bin/env python3
"""Test exact production decoding and compare all bit labels with local SDK headers."""
from pathlib import Path
import re
import subprocess
import tempfile


def main():
    runtime = Path(__file__).resolve().parents[1]
    source = (runtime / 'src/native_capabilities.rs').read_text()
    sdk = Path('/Applications/DevEco-Studio.app/Contents/sdk/default/openharmony/native/sysroot/usr/include')
    hwcap_header = (sdk / 'aarch64-linux-ohos/bits/hwcap.h').read_text()
    actual = {name: int(bit) for bit, name in re.findall(r'\((\d+),"(HWCAP\w+)"\)', source)}
    expected = {name: int(bit) for name, bit in re.findall(r'#define\s+(HWCAP\w+)\s+\(1(?:UL)?\s*<<\s*(\d+)\)', hwcap_header)}
    assert actual == expected, 'CPU flag decoder must match this SDK exactly'
    elf_header = (sdk / 'elf.h').read_text()
    for name, key in [('AT_HWCAP',16),('AT_HWCAP2',26)]:
        assert re.search(r'#define\s+' + name + r'\s+' + str(key) + r'\b', elf_header)
    lock = (runtime / 'Cargo.lock').read_text()
    version = re.search(r'\[\[package\]\]\nname = "serde_json"\nversion = "([^"]+)"', lock).group(1)
    with tempfile.TemporaryDirectory(prefix='native-capabilities-contract-') as directory:
        root = Path(directory)
        (root / 'src').mkdir()
        (root / 'Cargo.toml').write_text('[package]\nname="native-capabilities-contract"\nversion="0.0.0"\nedition="2021"\n[workspace]\n[dependencies]\nserde_json="=' + version + '"\n')
        (root / 'src/lib.rs').write_text('#![allow(dead_code)]\n' + source)
        subprocess.run(['cargo', 'test', '--offline', '--manifest-path', str(root / 'Cargo.toml')], check=True)
    print(f'Native CPU capability contracts and all {len(actual)} SDK bit definitions passed; current device flags require the app build.')


if __name__ == '__main__':
    main()
