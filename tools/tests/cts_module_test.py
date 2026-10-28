import hashlib
import importlib.util
from pathlib import Path
import struct
import tempfile
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location('cts_module', Path(__file__).resolve().parents[1] / 'cts-module.py')
cts = importlib.util.module_from_spec(spec)
spec.loader.exec_module(cts)


class Inputs(unittest.TestCase):
    def test_cache_fetch_and_hash_failure_in_both_layouts(self):
        name = 'android-cts/testcases/Module/Input.apk'
        body = b'pinned APK bytes'
        digest = hashlib.sha256(body).hexdigest()
        header = struct.pack('<30x')
        for tradefed in [False, True]:
            with self.subTest(tradefed=tradefed), tempfile.TemporaryDirectory() as root:
                destination = Path(root) / '_build' / ('cts-tradefed' if tradefed else 'cts')
                destination /= name if tradefed else 'Input.apk'
                args = ['cts-module.py', '--tradefed'] if tradefed else ['cts-module.py']
                with patch.object(cts, 'ROOT', root), patch.object(cts.sys, 'argv', args), \
                     patch.object(cts, 'lock', return_value=({'URL': 'fixture', 'SIZE': '100'}, [(name, digest)])), \
                     patch.object(cts, 'central_directory', return_value={name: (0, len(body), len(body), 0)}) as directory, \
                     patch.object(cts, 'fetch', side_effect=[header, body]) as fetch:
                    cts.main()
                    self.assertEqual(destination.read_bytes(), body)
                    directory.assert_called_once()
                    self.assertEqual(fetch.call_count, 2)
                    directory.reset_mock()
                    fetch.reset_mock(side_effect=True)
                    fetch.side_effect = AssertionError('cached input must not use network')
                    cts.main()
                    directory.assert_not_called()
                    fetch.assert_not_called()
                    destination.write_bytes(b'corrupted cache')
                    fetch.side_effect = [header, b'bad replacement']
                    with self.assertRaisesRegex(SystemExit, 'sha256'):
                        cts.main()
                    self.assertEqual(destination.read_bytes(), b'corrupted cache')
                    fetch.side_effect = [header, body]
                    cts.main()
                    self.assertEqual(destination.read_bytes(), body)


if __name__ == '__main__':
    unittest.main()
