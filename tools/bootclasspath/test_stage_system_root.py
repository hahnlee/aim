import unittest

import test_package_runtime
from stage_system_root import stage_system_root


class SystemRootTests(unittest.TestCase):
    setUp = test_package_runtime.PackageTests.setUp
    write_manifest = test_package_runtime.PackageTests.write_manifest

    def test_selected_bytes_at_android_locations(self):
        entries = stage_system_root(self.source, self.destination)
        self.assertEqual(entries, ["system/b.jar", "system/a.jar"])
        self.assertEqual((self.destination / "system/a.jar").read_bytes(),
                         b"explicit platform replacement")
        self.assertEqual((self.destination / "system/b.jar").read_bytes(), b"b")
        for entry in entries:
            status = (self.destination / entry).stat()
            self.assertEqual(status.st_mode & 0o777, 0o444)
            self.assertEqual(status.st_mtime, 1199145600)

    def test_corruption_rejected_before_staging(self):
        (self.inventory / "a.jar").write_bytes(b"corrupted")
        with self.assertRaisesRegex(ValueError, "hash mismatch"):
            stage_system_root(self.source, self.destination)
        self.assertFalse(self.destination.exists())

    def test_existing_file_preserved_without_partial_copy(self):
        existing = self.destination / "system/a.jar"
        existing.parent.mkdir(parents=True)
        existing.write_bytes(b"keep")
        with self.assertRaisesRegex(ValueError, "conflicting"):
            stage_system_root(self.source, self.destination)
        self.assertEqual(existing.read_bytes(), b"keep")
        self.assertFalse((self.destination / "system/b.jar").exists())

    def test_symlink_parent_cannot_escape(self):
        self.destination.mkdir(parents=True)
        outside = self.base / "outside"
        outside.mkdir()
        (self.destination / "system").symlink_to(outside, target_is_directory=True)
        with self.assertRaisesRegex(ValueError, "conflicting"):
            stage_system_root(self.source, self.destination)
        self.assertEqual(list(outside.iterdir()), [])


if __name__ == "__main__":
    unittest.main()
