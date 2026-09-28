#!/usr/bin/env python3
"""Focused tests for the generic ART corpus ledger."""

from __future__ import annotations

import json
import importlib.util
from pathlib import Path
import stat
import sys
import tempfile
import textwrap
import unittest

_MODULE_PATH = Path(__file__).with_name("run-art-upstream-corpus.py")
_SPEC = importlib.util.spec_from_file_location("run_art_upstream_corpus", _MODULE_PATH)
assert _SPEC is not None and _SPEC.loader is not None
corpus = importlib.util.module_from_spec(_SPEC)
sys.modules[_SPEC.name] = corpus
_SPEC.loader.exec_module(corpus)


class CorpusRunnerTest(unittest.TestCase):
    def setUp(self) -> None:
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name)
        self.archive = self.root / "_aosp/art/test"
        self.archive.mkdir(parents=True)
        for name in ("003-third", "001-first", "002-second", "004-fourth"):
            test = self.archive / name
            test.mkdir()
            (test / "input.txt").write_text(name, encoding="utf-8")
            # A runnable AOSP ART test is identified by the harness contract,
            # not merely by its directory name. Keep the fixture aligned with
            # discover_tests() so these tests exercise production discovery.
            (test / "expected-stdout.txt").write_text("", encoding="utf-8")
        self.runner = self.root / "fake-runner.py"
        self.runner.write_text(textwrap.dedent("""
            import argparse
            from pathlib import Path
            import sys

            parser = argparse.ArgumentParser()
            parser.add_argument("test")
            parser.add_argument("--root", required=True)
            parser.add_argument("--keep", action="store_true")
            args = parser.parse_args()
            counter = Path(args.root) / "count"
            counter.write_text(str(int(counter.read_text()) + 1) if counter.exists() else "1")
            if args.test == "002-second":
                print("fake failure", file=sys.stderr)
                raise SystemExit(7)
            print("artifacts=/tmp/fake-" + args.test)
        """), encoding="utf-8")
        self.runner.chmod(self.runner.stat().st_mode | stat.S_IXUSR)
        self.ledger = self.root / "ledger"

    def tearDown(self) -> None:
        self.temp.cleanup()

    def test_discovery_and_hash_are_deterministic(self) -> None:
        self.assertEqual(
            corpus.discover_tests(self.root),
            ["001-first", "002-second", "003-third", "004-fourth"],
        )
        first = corpus.input_hash(self.archive / "001-first")
        self.assertEqual(first, corpus.input_hash(self.archive / "001-first"))
        (self.archive / "001-first/input.txt").write_text("changed", encoding="utf-8")
        self.assertNotEqual(first, corpus.input_hash(self.archive / "001-first"))

    def test_ledger_resume_uses_input_and_runner_hash(self) -> None:
        result = corpus.main([
            "--root", str(self.root), "--runner", str(self.runner),
            "--ledger", str(self.ledger), "--limit", "2",
        ])
        self.assertEqual(result, 1)
        self.assertEqual((self.root / "count").read_text(), "2")
        payload = json.loads((self.ledger / "summary.json").read_text())
        self.assertEqual([item["test"] for item in payload["results"]],
                         ["001-first", "002-second"])
        self.assertEqual(payload["results"][0]["status"], "passed")
        self.assertEqual(payload["results"][1]["exit_code"], 7)
        self.assertTrue(Path(payload["results"][0]["runner_stdout"]).is_file())
        self.assertTrue(payload["results"][0]["artifacts"].startswith("/tmp/fake-"))
        self.assertEqual(
            corpus.main([
                "--root", str(self.root), "--runner", str(self.runner),
                "--ledger", str(self.ledger), "--limit", "2", "--resume",
            ]),
            1,
        )
        # Resume must retry the failed test while retaining the passed row.
        self.assertEqual((self.root / "count").read_text(), "3")
        (self.archive / "001-first/input.txt").write_text("changed", encoding="utf-8")
        self.assertEqual(
            corpus.main([
                "--root", str(self.root), "--runner", str(self.runner),
                "--ledger", str(self.ledger), "--limit", "2", "--resume",
            ]),
            1,
        )
        # The changed first test and the still-failing second test both run.
        self.assertEqual((self.root / "count").read_text(), "5")

    def test_fail_fast_does_not_start_unqueued_inputs(self) -> None:
        (self.root / "count").unlink(missing_ok=True)
        self.assertEqual(
            corpus.main([
                "--root", str(self.root), "--runner", str(self.runner),
                "--ledger", str(self.root / "fail-fast-ledger"),
                "--limit", "4", "--fail-fast",
            ]),
            1,
        )
        self.assertEqual((self.root / "count").read_text(), "2")
        payload = json.loads(
            (self.root / "fail-fast-ledger/summary.json").read_text())
        self.assertEqual(
            [item["test"] for item in payload["results"]],
            ["001-first", "002-second"],
        )

    def test_runtime_identity_covers_boot_components_and_official_graph(self) -> None:
        for relative in (
            "target/debug/aim-host",
            "_build/runtime-graphics-link-probe/libaim_runtime_graphics.dylib",
            "_build/android16-boot-image-darwin/boot-framework-compat.oat",
            "_build/android16-boot-image-darwin/boot-framework-compat.vdex",
            "_build/android16-boot-image-darwin/boot-core-libart.art",
            "_prebuilt/android-16/bootclasspath/core-libart.jar",
            "_build/android16-ps16k-r07/extracted/conscrypt/javalib/conscrypt.jar",
            "_build/dex-probe/unsafe-boot-dex/unsafe-boot.jar",
            "_build/native-graph/build.ninja",
            "_build/native-graph/build.inputs.sha256",
        ):
            with self.subTest(relative=relative):
                before = corpus.runtime_identity(self.root)
                path = self.root / relative
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_bytes(b"first")
                created = corpus.runtime_identity(self.root)
                self.assertNotEqual(before, created)
                path.write_bytes(b"other")  # Same size, different content.
                self.assertNotEqual(created, corpus.runtime_identity(self.root))

    def test_resume_invalidates_legacy_and_changed_runtime_records(self) -> None:
        args = ["--root", str(self.root), "--runner", str(self.runner),
                "--ledger", str(self.ledger), "--limit", "2", "--resume"]
        self.assertEqual(corpus.main(args), 1)
        payload = json.loads((self.ledger / "summary.json").read_text())
        for record in payload["results"]:
            del record["runtime_identity"]
        (self.ledger / "summary.json").write_text(json.dumps(payload))
        self.assertEqual(corpus.main(args), 1)
        self.assertEqual((self.root / "count").read_text(), "4")
        self.assertEqual(corpus.main(args), 1)
        # The failed row is retried even though its inputs are unchanged.
        self.assertEqual((self.root / "count").read_text(), "5")
        component = self.root / "_build/android16-boot-image-darwin/boot-framework-compat.oat"
        component.parent.mkdir(parents=True)
        component.write_bytes(b"new boot component")
        self.assertEqual(corpus.main(args), 1)
        self.assertEqual((self.root / "count").read_text(), "7")

    def test_shard_selection_is_index_based(self) -> None:
        self.assertEqual(
            corpus.shard_tests(corpus.discover_tests(self.root), 1, 2),
            ["002-second", "004-fourth"],
        )

    def test_contiguous_range_uses_inclusive_named_boundaries(self) -> None:
        tests = corpus.discover_tests(self.root)
        self.assertEqual(
            corpus.select_contiguous_range(tests, "002-second", "003-third"),
            ["002-second", "003-third"],
        )
        self.assertEqual(
            corpus.select_contiguous_range(tests, start_at="003-third"),
            ["003-third", "004-fourth"],
        )
        self.assertEqual(
            corpus.select_contiguous_range(tests, stop_after="002-second"),
            ["001-first", "002-second"],
        )
        with self.assertRaisesRegex(ValueError, "start-at test is not in the corpus"):
            corpus.select_contiguous_range(tests, start_at="999-missing")
        with self.assertRaisesRegex(ValueError, "start-at test must not follow"):
            corpus.select_contiguous_range(tests, "004-fourth", "001-first")

    def test_main_runs_only_the_named_contiguous_range(self) -> None:
        self.assertEqual(
            corpus.main([
                "--root", str(self.root), "--runner", str(self.runner),
                "--ledger", str(self.ledger),
                "--start-at", "003-third", "--stop-after", "004-fourth",
            ]),
            0,
        )
        self.assertEqual((self.root / "count").read_text(), "2")
        payload = json.loads((self.ledger / "summary.json").read_text())
        self.assertEqual(
            [item["test"] for item in payload["results"]],
            ["003-third", "004-fourth"],
        )

    def test_range_preserves_out_of_range_records_and_runner_hash_summary(self) -> None:
        records = {
            "001-first": corpus.TestResult(
                test="001-first", input_hash="input-1", runner_hash="runner-old",
                status="passed", exit_code=0, runner_stdout="", runner_stderr="",
                artifacts="", error=""),
            "004-fourth": corpus.TestResult(
                test="004-fourth", input_hash="input-4", runner_hash="runner-new",
                status="passed", exit_code=0, runner_stdout="", runner_stderr="",
                artifacts="", error=""),
        }
        corpus.write_ledgers(self.ledger, records)
        payload = json.loads((self.ledger / "summary.json").read_text())
        self.assertEqual(payload["runner_hashes"], ["runner-new", "runner-old"])
        self.assertEqual(
            [item["test"] for item in payload["results"]],
            ["001-first", "004-fourth"],
        )


if __name__ == "__main__":
    unittest.main()
