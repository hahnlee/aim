import importlib.util
from pathlib import Path
import os
import subprocess
import tempfile
import unittest
import json
import hashlib
import zipfile
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

    def test_official_launcher_resolves_pinned_preparer_class_and_resource(self):
        harness = ROOT / '_build/cts-tradefed/android-cts'
        owner = harness / 'testcases/CtsAppSecurityHostTestCases/CtsAppSecurityHostTestCases.jar'
        pins = {}
        for line in (ROOT / 'upstream/cts-tradefed.lock').read_text().splitlines():
            if line.startswith('"'):
                entry, digest = line.strip('"').split('|')
                pins[entry] = digest
        pin = pins['android-cts/testcases/CtsAppSecurityHostTestCases/CtsAppSecurityHostTestCases.jar']
        self.assertEqual(hashlib.sha256(owner.read_bytes()).hexdigest(), pin)
        resource_name = 'android/appsecurity/cts/AppSecurityPreparer.class'
        with zipfile.ZipFile(owner) as jar:
            resource_sha = hashlib.sha256(jar.read(resource_name)).hexdigest()
        java = next(path for path in [
            Path('/Applications/Android Studio.app/Contents/jbr/Contents/Home/bin/java'),
            Path('/opt/homebrew/opt/openjdk@21/bin/java'),
        ] if path.is_file() and '21.' in subprocess.run(
            [str(path), '-version'], capture_output=True, text=True, check=True).stderr)
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            receipt = bundle.snapshot(ROOT, harness, root / 'generations')
            private = Path(receipt['directory']) / 'harness/android-cts'
            self.assertFalse((private / 'testcases').is_symlink())
            # These are the unchanged pinned launcher's actual discovery rules.
            launcher = (private / 'tools/cts-tradefed').read_text()
            self.assertIn("find ${CTS_ROOT}/android-cts/testcases -name '*.jar'", launcher)
            discover = lambda path: subprocess.check_output(
                ['find', str(path), '-name', '*.jar'], text=True).splitlines()
            jars = discover(private / 'testcases')
            self.assertEqual({str(Path(path).resolve()) for path in jars},
                             {str(Path(path).resolve()) for path in discover((harness / 'testcases').resolve())})
            self.assertIn(str((private / 'testcases/CtsAppSecurityHostTestCases') / owner.name), jars)
            tools = [str(path) for path in sorted((private / 'tools').glob('*.jar'))]
            probe = root / 'PreparerProbe.java'
            probe.write_text(r"""
import android.appsecurity.cts.AppSecurityPreparer;
import java.nio.file.Path;
import java.security.MessageDigest;
import java.util.HexFormat;
public class PreparerProbe {
    public static void main(String[] args) throws Exception {
        AppSecurityPreparer preparer = new AppSecurityPreparer();
        Path actual = Path.of(preparer.getClass().getProtectionDomain().getCodeSource().getLocation().toURI()).toRealPath();
        if (!actual.equals(Path.of(args[0]).toRealPath())) throw new AssertionError(actual);
        try (var resource = AppSecurityPreparer.class.getResourceAsStream("/android/appsecurity/cts/AppSecurityPreparer.class")) {
            if (resource == null) throw new AssertionError("missing preparer resource");
            String sha = HexFormat.of().formatHex(MessageDigest.getInstance("SHA-256").digest(resource.readAllBytes()));
            if (!sha.equals(args[1])) throw new AssertionError(sha);
        }
        System.out.println("real pinned preparer class, constructor, resource and artifact delegation resolved");
    }
}
""")
            broken = root / 'old-testcases'
            broken.symlink_to((harness / 'testcases').resolve(), target_is_directory=True)
            self.assertEqual(discover(broken), [])
            failed = subprocess.run([str(java), '-cp', ':'.join(tools + discover(broken)),
                                     str(probe), str(owner), resource_sha],
                                    capture_output=True, text=True, timeout=30)
            self.assertNotEqual(failed.returncode, 0)
            self.assertIn('android.appsecurity.cts', failed.stderr)
            passed = subprocess.run([str(java), '-cp', ':'.join(tools + jars),
                                     str(probe), str(owner), resource_sha],
                                    capture_output=True, text=True, timeout=30)
            self.assertEqual(passed.returncode, 0, passed.stderr)
            self.assertIn('real pinned preparer class', passed.stdout)
            self.assertEqual(hashlib.sha256(owner.read_bytes()).hexdigest(), pin)
            bundle.validate(receipt['directory'])
            link = private / 'testcases/CtsAppSecurityHostTestCases' / owner.name
            link.unlink()
            foreign = root / 'foreign.jar'
            foreign.write_bytes(b'foreign artifact')
            link.symlink_to(foreign)
            with self.assertRaisesRegex(ValueError, 'testcase artifact link differs'):
                bundle.validate(receipt['directory'])

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
