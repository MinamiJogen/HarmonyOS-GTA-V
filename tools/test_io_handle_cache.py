#!/usr/bin/env python3
"""Compile and run the actual isolated native resource handle-cache module."""
import json
from pathlib import Path
import subprocess
import tempfile
import unittest


class IoHandleCacheTests(unittest.TestCase):
    def test_real_rust_module(self):
        root = Path(__file__).resolve().parents[1]
        with tempfile.TemporaryDirectory(prefix='muguet-io-cache-tests-') as temporary:
            directory = Path(temporary)
            harness = directory / 'harness.rs'
            harness.write_text('#[path = ' + json.dumps(str(root / 'game-runtime/src/io_handle_cache.rs'), ensure_ascii=False)
                               + ']\nmod handle_cache;\n')
            binary = directory / 'io-handle-cache-tests'
            compiled = subprocess.run(['/bin/bash', '-c', 'source "$1"; shift; exec "$@"', 'io-cache-tests',
                                       str(root / 'game-runtime/scripts/env.sh'), 'rustc', '--edition=2021',
                                       '--test', str(harness), '-o', str(binary)],
                                      text=True, capture_output=True)
            self.assertEqual(compiled.returncode, 0, compiled.stdout + compiled.stderr)
            tested = subprocess.run([str(binary), '--nocapture'], text=True, capture_output=True)
            print(tested.stdout, end='')
            self.assertEqual(tested.returncode, 0, tested.stdout + tested.stderr)
            self.assertIn('11 passed; 0 failed', tested.stdout)


if __name__ == '__main__':
    unittest.main()
