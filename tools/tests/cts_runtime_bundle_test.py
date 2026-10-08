import importlib.util
from pathlib import Path
import os
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location('bundle', ROOT / 'tools/lib/cts_runtime_bundle.py')
bundle = importlib.util.module_from_spec(spec)
spec.loader.exec_module(bundle)


class Lifetime(unittest.TestCase):
    def test_waiting_bash_retains_detached_generation_after_source_edit(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            for name in bundle.SCRIPTS:
                path = root / name
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text('#!/bin/bash\nprintf "ready\\n"\nread -r release\nprintf "original generation\\n"\nexit 7\n')
            harness = root / 'official/android-cts'
            (harness / 'tools').mkdir(parents=True)
            for name in bundle.HARNESS_SCRIPTS:
                (harness / 'tools' / name).write_text('#!/bin/bash\nexit 0\n')
            (harness / 'tools/tradefed.jar').write_bytes(b'actual fixture artifact')
            receipt = bundle.snapshot(root, harness, root / 'snapshots')
            script = Path(receipt['directory']) / 'tools/cts-tradefed.sh'
            self.assertNotEqual(script.stat().st_ino, (root / 'tools/cts-tradefed.sh').stat().st_ino)
            process = subprocess.Popen(['bash', str(script)], stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True)
            try:
                self.assertEqual(process.stdout.readline(), 'ready\n')
                (root / 'tools/cts-tradefed.sh').write_text('#!/bin/bash\nexit 127\n')
                output, _ = process.communicate('continue\n', timeout=5)
                self.assertEqual(process.returncode, 7)
                self.assertEqual(output, 'original generation\n')
                bundle.validate(receipt['directory'])
                script.chmod(0o755)
                with self.assertRaisesRegex(ValueError, 'mode differs'):
                    bundle.validate(receipt['directory'])
                script.chmod(0o555)
                tools = Path(receipt['directory']) / 'harness/android-cts/tools'
                jar = tools / 'tradefed.jar'
                jar.unlink()
                other = root / 'foreign.jar'
                other.write_bytes(b'foreign artifact')
                jar.symlink_to(other)
                with self.assertRaisesRegex(ValueError, 'JAR link differs'):
                    bundle.validate(receipt['directory'])
                jar.unlink()
                jar.symlink_to(harness / 'tools/tradefed.jar')
                (tools / 'extra.jar').symlink_to(other)
                with self.assertRaisesRegex(ValueError, 'JAR inventory differs'):
                    bundle.validate(receipt['directory'])
                (tools / 'extra.jar').unlink()
                (harness / 'tools/tradefed.jar').write_bytes(b'changed official fixture artifact')
                with self.assertRaisesRegex(ValueError, 'external harness artifact'):
                    bundle.validate(receipt['directory'])
            finally:
                if process.poll() is None:
                    process.kill()
                    process.wait()


if __name__ == '__main__':
    unittest.main()
