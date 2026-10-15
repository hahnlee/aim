"""Guest-shell delegates init authentication and preserves explicit image argv."""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]


class GuestShellInputs(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name).resolve()
        self.image = self.root / 'native-image'
        self.image.mkdir()
        self.data = self.root / 'data'
        self.runtime = self.root / 'data.run'
        self.runtime.mkdir()
        (self.runtime / 'path-map').write_text('fixture mapping')
        (self.runtime / 'environ').write_text('PATH=/system/bin\nBOOTCLASSPATH=original-classpath\n')
        self.owner_args = self.root / 'owner-args.json'
        self.owner = self.program('owner',
            f'from pathlib import Path\nPath({str(self.owner_args)!r}).write_text(json.dumps(sys.argv[1:]))\nprint("21731")\n')
        self.linux = self.program('linux-run', 'print(json.dumps({"argv": sys.argv[1:], "env": dict(os.environ)}))\n')

    def program(self, name, body):
        path = self.root / name
        path.write_text(f'#!{sys.executable}\nimport json, os, sys\n' + body)
        path.chmod(0o755)
        return path

    def command(self, *extra):
        return ['bash', str(ROOT / 'tools/guest-shell.sh'), '--repository-root', str(ROOT),
                '--linux-run', str(self.linux), '--cts-owner', str(self.owner),
                '--image', str(self.image), *extra, str(self.data), 'id']

    def test_selected_image_init_and_original_environment_are_forwarded(self):
        result = subprocess.run(self.command(), capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        output = json.loads(result.stdout)
        args = output['argv']
        self.assertEqual(args[args.index('--root') + 1], str(self.image))
        self.assertEqual(args[args.index('--binder') + 1], 'dev.aim.guest-init.21731.binder')
        self.assertEqual(args[args.index('--by-pid') + 1], str(self.runtime / 'identity/by-pid'))
        self.assertEqual(args[args.index('--path-map') + 1], str(self.runtime / 'path-map'))
        self.assertIn('--mount-namespace-from-init', args)
        self.assertEqual(args[-3:], ['/system/bin/sh', '-c', 'id'])
        self.assertEqual(output['env']['BOOTCLASSPATH'], 'original-classpath')
        self.assertEqual(json.loads(self.owner_args.read_text()), ['--init-pid', str(self.runtime / 'identity/by-pid')])

    def test_authentication_error_blocks_shell_and_keeps_error(self):
        self.owner.write_text(f'#!{sys.executable}\nimport sys\nprint("metadata EPERM", file=sys.stderr)\nsys.exit(1)\n')
        result = subprocess.run(self.command(), capture_output=True, text=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('metadata EPERM', result.stderr)
        self.assertEqual(result.stdout, '')

    def test_invalid_pid_is_not_a_binder_owner(self):
        self.owner.write_text(f'#!{sys.executable}\nprint("0")\n')
        result = subprocess.run(self.command(), capture_output=True, text=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('invalid authenticated init PID', result.stderr)
        self.assertEqual(result.stdout, '')

    def test_missing_image_argument_is_rejected_before_authentication(self):
        result = subprocess.run(['bash', str(ROOT / 'tools/guest-shell.sh'), '--image'], capture_output=True, text=True)
        self.assertEqual(result.returncode, 2)
        self.assertIn('requires a value', result.stderr)
        self.assertFalse(self.owner_args.exists())


if __name__ == '__main__':
    unittest.main()
