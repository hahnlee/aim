import importlib.util
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import shutil
import unittest
from types import SimpleNamespace

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / 'tools'))
from lib import cts_host_tools
spec = importlib.util.spec_from_file_location('cts_pm', ROOT / 'tools/cts-pm.py')
cts_pm = importlib.util.module_from_spec(spec)
spec.loader.exec_module(cts_pm)


class Selection(unittest.TestCase):
    def test_actual_receipts_and_unchanged_official_inputs(self):
        receipt = cts_host_tools.stage(ROOT / 'target/aim/cts-host-tools', ROOT, cts_pm.HARNESS)
        directory = Path(receipt['module_arg'].split(':=', 1)[1])
        for name, sha in receipt['official_inputs'].items():
            self.assertEqual(cts_host_tools.digest(directory / name), sha)
            self.assertEqual(cts_host_tools.digest(cts_pm.HARNESS / 'testcases' / cts_host_tools.MODULE / name), sha)
        self.assertNotEqual(receipt['official_zip_sha256'], receipt['selected_zip_sha256'])
        with tempfile.TemporaryDirectory() as temporary:
            image = Path(temporary)
            (image / '.overlay-receipt').write_text('actual fixture receipt')
            provenance = cts_pm.campaign_provenance(SimpleNamespace(image=image, data=image,
                port=5611, cts_args=[], label='native', linux_run=None,
                cts_host_toolsdir=ROOT / 'target/aim/cts-host-tools'))
            self.assertEqual(provenance['cts_host_tools'], receipt)
        self.assertEqual(cts_host_tools.arguments(receipt, 'CtsAppSecurityHostTestCases'), [])
        args = SimpleNamespace(port=5611, cts_args=[], host_tool_selection=receipt)
        command = ['cts', '-s', '127.0.0.1:5611', '--skip-device-info', '--skip-preconditions',
                   '-m', cts_host_tools.MODULE, *cts_host_tools.arguments(receipt, cts_host_tools.MODULE)]
        import shlex
        result = {'command': shlex.join(command), 'modules': [{'name': cts_host_tools.MODULE,
                  'done': True, 'declared_total': '1', 'tests': [{'result': 'pass'}]}]}
        self.assertTrue(cts_pm.validate_result(result, cts_host_tools.MODULE, args))
        result['command'] = shlex.join(command[:-2])
        with self.assertRaises(ValueError):
            cts_pm.validate_result(result, cts_host_tools.MODULE, args)

    def test_missing_proof_rejected_before_wrapper_device_actions(self):
        with tempfile.TemporaryDirectory() as temporary:
            process = subprocess.run([str(ROOT / 'tools/cts-tradefed.sh'), '--cts-host-toolsdir', temporary,
                                      '/unused/data', '5611', '-m', cts_host_tools.MODULE], capture_output=True)
            self.assertNotEqual(process.returncode, 0)
            self.assertIn(b'provenance.json', process.stderr)
            self.assertNotIn(b'adb', process.stdout)

    def test_wrong_module_rejected_before_device_actions(self):
        process = subprocess.run([str(ROOT / 'tools/cts-tradefed.sh'), '--cts-host-toolsdir',
                                  str(ROOT / 'target/aim/cts-host-tools'), '/unused/data', '5611',
                                  '-m', 'CtsAppSecurityHostTestCases'], capture_output=True)
        self.assertNotEqual(process.returncode, 0)
        self.assertIn(b'explicit CtsStagedInstallHostTestCases', process.stderr)

    def test_tampered_python_runtime_rejected(self):
        with tempfile.TemporaryDirectory() as temporary:
            copied = Path(temporary)
            source = ROOT / 'target/aim/cts-host-tools'
            for name in ['provenance.json', 'validation.json', 'deapexer', 'debugfs_static', 'fsck.erofs']:
                shutil.copy2(source / name, copied / name)
            shutil.copytree(source / 'python', copied / 'python')
            with (copied / 'python/deapexer.py').open('a') as output:
                output.write('\n# altered runtime\n')
            with self.assertRaisesRegex(ValueError, 'component differs'):
                cts_host_tools.stage(copied, ROOT, cts_pm.HARNESS)


if __name__ == '__main__':
    unittest.main()
