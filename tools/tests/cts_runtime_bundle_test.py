import importlib.util
from pathlib import Path
import os
import subprocess
import tempfile
import unittest
import json
from types import SimpleNamespace
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location('bundle', ROOT / 'tools/lib/cts_runtime_bundle.py')
bundle = importlib.util.module_from_spec(spec)
spec.loader.exec_module(bundle)
pm_spec = importlib.util.spec_from_file_location('cts_pm', ROOT / 'tools/cts-pm.py')
pm = importlib.util.module_from_spec(pm_spec)
pm_spec.loader.exec_module(pm)


class Lifetime(unittest.TestCase):
    def test_existing_campaign_output_is_rejected_before_snapshot_writes(self):
        with tempfile.TemporaryDirectory() as temporary:
            output = Path(temporary)
            sentinel = output / 'campaign.json'
            sentinel.write_text('retained campaign bytes')
            args = SimpleNamespace(output=output, resume=False, stop_after=None)
            with patch.object(pm.cts_runtime_bundle, 'snapshot') as snapshot:
                with self.assertRaisesRegex(ValueError, 'already exists'):
                    pm.run(args)
                snapshot.assert_not_called()
            self.assertEqual(sentinel.read_text(), 'retained campaign bytes')
            self.assertEqual({p.name for p in output.iterdir()}, {'campaign.json'})

    def test_resume_rejects_changed_wrapper_generation_and_arguments(self):
        with tempfile.TemporaryDirectory() as temporary:
            output = Path(temporary)
            provenance = {'wrapper_bundle': {'identity': 'retained'}, 'cts_args': []}
            (output / 'campaign.json').write_text(json.dumps(dict(provenance, runs=[])))
            args = SimpleNamespace(output=output)
            self.assertEqual(pm.resume_state(args, provenance)['wrapper_bundle'], provenance['wrapper_bundle'])
            with self.assertRaisesRegex(ValueError, 'wrapper_bundle'):
                pm.resume_state(args, dict(provenance, wrapper_bundle={'identity': 'changed'}))
            with self.assertRaisesRegex(ValueError, 'cts_args'):
                pm.resume_state(args, dict(provenance, cts_args=['--changed']))

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
