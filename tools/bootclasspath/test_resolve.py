import hashlib
import json
from pathlib import Path
import tempfile
import unittest
from resolve import resolve


class ResolveTests(unittest.TestCase):
    def test_resolves_order_and_rejects_changed_original(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary).resolve()
            inventory = root / "_build/android16-bootclasspath-original"
            inventory.mkdir(parents=True)
            config = root / "tools/bootclasspath"
            config.mkdir(parents=True)
            (config / "replacements.json").write_text("{}")
            records = []
            for name in ("a", "b"):
                data = name.encode()
                (inventory / f"{name}.jar").write_bytes(data)
                records.append(dict(device_path=f"/system/{name}.jar",
                                    relative_path=f"{name}.jar",
                                    sha256=hashlib.sha256(data).hexdigest()))
            manifest = dict(fragments=records,
                            bootclasspath=[r["device_path"] for r in reversed(records)])
            (inventory / "manifest.json").write_text(json.dumps(manifest))
            self.assertEqual(resolve(root), [str(inventory / "b.jar"), str(inventory / "a.jar")])
            self.assertEqual(resolve(root, locations=True), ["/system/b.jar", "/system/a.jar"])
            replacement = root / "replacement.jar"
            replacement.write_bytes(b"replacement")
            (config / "replacements.json").write_text(json.dumps({"/system/a.jar": "replacement.jar"}))
            self.assertEqual(resolve(root), [str(inventory / "b.jar"), str(replacement)])
            self.assertEqual(resolve(root, locations=True), ["/system/b.jar", "/system/a.jar"])
            (inventory / "a.jar").write_bytes(b"changed")
            with self.assertRaisesRegex(ValueError, "hash mismatch"):
                resolve(root)
            with self.assertRaisesRegex(ValueError, "hash mismatch"):
                resolve(root, locations=True)


if __name__ == "__main__":
    unittest.main()
