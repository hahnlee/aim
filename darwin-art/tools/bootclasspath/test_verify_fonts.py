import contextlib
import hashlib
import io
import json
from pathlib import Path
import tempfile
import unittest

from verify_fonts import verify


class FontInventoryTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="font-inventory-test-")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        (self.root / "system/etc").mkdir(parents=True)
        (self.root / "system/fonts").mkdir()
        config = b"<familyset><family><font>Roboto-Regular.ttf</font><font>Absent.ttf</font></family></familyset>"
        contents = {
            "system/etc/fonts.xml": config,
            "system/etc/font_fallback.xml": config,
            "system/fonts/Roboto-Regular.ttf": b"manifest test fixture, not a real font",
        }
        self.manifest = {"version": 1, "absent_in_image": ["Absent.ttf"], "files": []}
        for name, data in contents.items():
            (self.root / name).write_bytes(data)
            self.manifest["files"].append({"path": name, "sha256": hashlib.sha256(data).hexdigest()})
        self.save()

    def save(self):
        (self.root / "manifest.json").write_text(json.dumps(self.manifest))

    def test_original_absence_is_preserved(self):
        with contextlib.redirect_stdout(io.StringIO()):
            verify(self.root)

    def test_changed_bytes_reject(self):
        (self.root / "system/fonts/Roboto-Regular.ttf").write_bytes(b"changed")
        with self.assertRaisesRegex(ValueError, "hash mismatch"):
            verify(self.root)

    def test_symlink_rejects(self):
        font = self.root / "system/fonts/Roboto-Regular.ttf"
        font.rename(self.root / "outside-font")
        font.symlink_to(self.root / "outside-font")
        with self.assertRaisesRegex(ValueError, "regular file"):
            verify(self.root)

    def test_unlisted_file_rejects(self):
        (self.root / "system/fonts/extra").write_bytes(b"extra")
        with self.assertRaisesRegex(ValueError, "inventory"):
            verify(self.root)

    def test_duplicate_and_traversal_reject(self):
        self.manifest["files"].append(self.manifest["files"][0])
        self.save()
        with self.assertRaisesRegex(ValueError, "duplicate"):
            verify(self.root)
        self.manifest["files"][-1] = {"path": "system/fonts/../../outside", "sha256": "0" * 64}
        self.save()
        with self.assertRaisesRegex(ValueError, "invalid"):
            verify(self.root)

    def test_changed_absence_inventory_rejects(self):
        self.manifest["absent_in_image"] = []
        self.save()
        with self.assertRaisesRegex(ValueError, "absence inventory"):
            verify(self.root)


if __name__ == "__main__":
    unittest.main()
