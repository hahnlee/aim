#!/usr/bin/env python3
"""Tests for the unified custom-contract manifest/audit boundary."""

from __future__ import annotations

import copy
import json
from pathlib import Path
from tempfile import TemporaryDirectory
import unittest

try:
    from tools import art_custom_contract_manifest as manifest
except ModuleNotFoundError:  # Direct ``python tools/test_art_custom_contract_manifest.py``.
    import art_custom_contract_manifest as manifest


ROOT = Path(__file__).resolve().parent.parent


class CustomContractManifestTest(unittest.TestCase):
    def test_real_corpus_is_complete_and_deterministic(self) -> None:
        first = manifest.build_manifest(ROOT)
        second = manifest.build_manifest(ROOT)
        self.assertEqual(first, second)
        self.assertEqual(first["summary"], {
            "directories": 419,
            "sources": 491,
            "unsupported": 0,
            "opaque_shell": 0,
            "unowned_source_bytes": 0,
            "omitted_directories": 0,
        })
        encoded = manifest.manifest_json(first)
        self.assertEqual(encoded, manifest.manifest_json(second))
        self.assertEqual(json.loads(encoded), first)
        for record in first["directories"]:
            self.assertTrue(record["relative_source_paths"])
            self.assertEqual(sorted(record["relative_source_paths"]),
                             record["relative_source_paths"])
            self.assertEqual(set(record["source_hashes"]),
                             set(record["normalized_ir_hashes"]))
            self.assertEqual(set(record["source_hashes"]),
                             {source["path"] for source in record["sources"]})
            self.assertEqual(set(record["variants"]), {"host", "jvm"})

    def test_tampered_source_hash_is_rejected(self) -> None:
        original = manifest.build_manifest(ROOT)
        tampered = copy.deepcopy(original)
        source_path = tampered["directories"][0]["relative_source_paths"][0]
        tampered["directories"][0]["source_hashes"][source_path] = "0" * 64
        errors = manifest.verify_manifest(ROOT, tampered)
        self.assertTrue(any("hashes" in error for error in errors))

    def test_omitted_directory_is_rejected(self) -> None:
        original = manifest.build_manifest(ROOT)
        tampered = copy.deepcopy(original)
        tampered["directories"].pop()
        errors = manifest.verify_manifest(ROOT, tampered)
        self.assertTrue(any("omitted custom-script directories" in error
                            for error in errors))

    def test_unknown_script_and_unowned_bytes_fail_closed(self) -> None:
        with TemporaryDirectory() as directory:
            root = Path(directory)
            test = root / "_aosp" / "art" / "test" / "123-adversarial"
            test.mkdir(parents=True)
            (test / "run.py").write_text("def run(ctx, args):\n    pass\n",
                                         encoding="utf-8")
            (test / "mystery.sh").write_text("touch /tmp/pwned\n",
                                              encoding="utf-8")
            result = manifest.build_manifest(root)
            self.assertNotEqual(result["summary"]["unsupported"], 0)
            self.assertNotEqual(result["summary"]["unowned_source_bytes"], 0)
            self.assertTrue(any("unknown custom script" in error
                                for error in result["errors"]))


if __name__ == "__main__":
    unittest.main()
