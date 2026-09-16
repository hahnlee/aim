import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest

from package_runtime import package_runtime


class PackageTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.base = Path(self.temporary.name).resolve()
        self.source = self.base / "source"
        self.destination = self.base / "Manager App/runtime"
        self.inventory = self.source / "_build/android16-bootclasspath-original"
        self.inventory.mkdir(parents=True)
        config = self.source / "tools/bootclasspath"
        config.mkdir(parents=True)
        shutil.copy2(Path(__file__).with_name("resolve.py"), config / "resolve.py")
        records = []
        for name in ("a", "b"):
            data = name.encode()
            (self.inventory / f"{name}.jar").write_bytes(data)
            records.append(dict(device_path=f"/system/{name}.jar",
                                relative_path=f"{name}.jar",
                                sha256=hashlib.sha256(data).hexdigest()))
        self.manifest = dict(fragments=records,
                             bootclasspath=[r["device_path"] for r in reversed(records)])
        self.write_manifest()
        (self.source / "replacement.jar").write_bytes(b"explicit platform replacement")
        (config / "replacements.json").write_text(json.dumps({"/system/a.jar": "replacement.jar"}))
        (self.inventory / "unrelated-large-cache").write_bytes(b"not packaged")

    def write_manifest(self):
        (self.inventory / "manifest.json").write_text(json.dumps(self.manifest))

    def test_relocated_cli_keeps_order_without_source_and_reuses_identical_inputs(self):
        result = package_runtime(self.source, self.destination)
        resolver = self.destination / "tools/bootclasspath/resolve.py"
        inode = resolver.stat().st_ino
        self.assertEqual(package_runtime(self.source, self.destination), result)
        self.assertEqual(resolver.stat().st_ino, inode)
        self.assertFalse((self.destination / "_build/android16-bootclasspath-original/unrelated-large-cache").exists())
        self.source.rename(self.base / "unavailable-source")
        cli = subprocess.run([sys.executable, str(resolver)], check=True,
                             capture_output=True, text=True)
        self.assertEqual(cli.stdout.strip(), ":".join(result))
        self.assertEqual(Path(result[0]).name, "b.jar")
        self.assertEqual(Path(result[1]).name, "replacement.jar")

    def test_changed_original_rejected_before_publication(self):
        (self.inventory / "a.jar").write_bytes(b"corrupt")
        with self.assertRaisesRegex(ValueError, "hash mismatch"):
            package_runtime(self.source, self.destination)
        self.assertFalse(self.destination.exists())

    def test_duplicate_manifest_rejected(self):
        self.manifest["fragments"].append(self.manifest["fragments"][0])
        self.write_manifest()
        with self.assertRaisesRegex(ValueError, "duplicate"):
            package_runtime(self.source, self.destination)

    def test_conflict_preserved(self):
        self.destination.mkdir(parents=True)
        conflict = self.destination / "replacement.jar"
        conflict.write_bytes(b"existing")
        with self.assertRaisesRegex(ValueError, "conflicting"):
            package_runtime(self.source, self.destination)
        self.assertEqual(conflict.read_bytes(), b"existing")
        self.assertFalse((self.destination / "tools").exists())

    def test_destination_ancestor_symlink_escape_rejected(self):
        self.destination.mkdir(parents=True)
        outside = self.base / "outside"
        outside.mkdir()
        (self.destination / "tools").symlink_to(outside, target_is_directory=True)
        with self.assertRaisesRegex(ValueError, "target escapes"):
            package_runtime(self.source, self.destination)
        self.assertEqual(list(outside.iterdir()), [])

    def test_external_replacement_rejected(self):
        (self.base / "external.jar").write_bytes(b"outside")
        (self.source / "tools/bootclasspath/replacements.json").write_text(
            json.dumps({"/system/a.jar": "../external.jar"}))
        with self.assertRaisesRegex(ValueError, "runtime JAR escapes"):
            package_runtime(self.source, self.destination)


class PinnedArtifactPackageTests(unittest.TestCase):
    @unittest.skipUnless(os.environ.get("DARWIN_ART_TEST_PINNED_BOOTCLASSPATH") == "1",
                         "requires materialized pinned Android artifacts")
    def test_actual_inventory_packaging_and_relocated_cli(self):
        source = Path(__file__).resolve().parents[2]
        with tempfile.TemporaryDirectory(prefix="android-bootclasspath-package.") as temporary:
            destination = Path(temporary).resolve() / "Manager App/runtime"
            result = package_runtime(source, destination)
            cli = subprocess.run(
                [sys.executable, str(destination / "tools/bootclasspath/resolve.py")],
                check=True, capture_output=True, text=True)
            self.assertEqual(cli.stdout.strip(), ":".join(result))
            self.assertTrue(result)
            for path in result:
                self.assertTrue(Path(path).is_relative_to(destination))


if __name__ == "__main__":
    unittest.main()
