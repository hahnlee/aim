#!/usr/bin/env python3
"""Execution-time confinement tests for run.py typed shell actions."""

from __future__ import annotations

import importlib.util
from pathlib import Path
import sys
import tempfile
import unittest


TOOLS = Path(__file__).parent
sys.path.insert(0, str(TOOLS))
_SPEC = importlib.util.spec_from_file_location("run_art_upstream_test", TOOLS / "run-art-upstream-test.py")
assert _SPEC is not None and _SPEC.loader is not None
RUNNER = importlib.util.module_from_spec(_SPEC)
sys.modules[_SPEC.name] = RUNNER
_SPEC.loader.exec_module(RUNNER)


class TypedRunActionTest(unittest.TestCase):
    def test_aosp_test_args_use_shell_word_boundaries(self) -> None:
        self.assertEqual(
            RUNNER.expand_aosp_test_args(("--locks-only -o 100",)),
            ("--locks-only", "-o", "100"),
        )
        self.assertEqual(
            RUNNER.expand_aosp_test_args(("--label 'two words'",)),
            ("--label", "two words"),
        )
        with self.assertRaises(RuntimeError):
            RUNNER.expand_aosp_test_args(("--unterminated 'quote",))

    def test_aosp_runtime_options_use_shell_word_boundaries(self) -> None:
        self.assertEqual(
            RUNNER.expand_aosp_runtime_options((
                "-Xcompiler-option --compiler-filter=verify",
                "-Xjitinitialsize:32M",
            )),
            ("-Xcompiler-option", "--compiler-filter=verify",
             "-Xjitinitialsize:32M"),
        )
        with self.assertRaises(RuntimeError):
            RUNNER.expand_aosp_runtime_options(("-Xbad 'quote",))

    def test_default_run_scalar_snapshot_survives_materialization(self) -> None:
        invocation = RUNNER.invocation_from_action({
            "kind": "default_run",
            "args_snapshot": {
                "prebuild": False,
                "image": False,
                "relocate": True,
                "runtime_option": [],
                "android_runtime_option": [],
                "test_args": [],
                "testlib": [],
                "Xcompiler_option": [],
                "compiler_only_option": [],
            },
            "kwargs": [],
        })
        self.assertFalse(invocation.prebuild)
        self.assertFalse(invocation.image)
        self.assertTrue(invocation.relocate)

    def test_implicit_modes_use_aosp_verify_only_prebuild(self) -> None:
        for mode in ("interpreter", "jit"):
            invocation = RUNNER.implicit_default_invocation(mode)
            self.assertEqual(
                invocation.compiler_options, ("--compiler-filter=verify",))
        self.assertEqual(
            RUNNER.implicit_default_invocation("host").compiler_options, ())

    def test_javac_post_argument_condition_is_detected(self) -> None:
        from art_javac_post_contract import compile_contract

        script = Path("_aosp/art/test/126-miranda-multidex/javac_post.sh")
        self.assertTrue(RUNNER._post_requires_argument(
            compile_contract(script.read_bytes(), script)))

    def test_positive_sed_head_move_and_symlink(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            source = root / "input.txt"
            source.write_bytes(b"keep\nDROP\nlast\n")
            filtered = root / "filtered.txt"
            RUNNER.apply_typed_file_operation({
                "kind": "sed_filter",
                "file": str(source),
                "scripts": [{"kind": "delete_matching", "pattern": "DROP"}],
            }, root)
            self.assertEqual(source.read_bytes(), b"keep\nlast\n")
            RUNNER.apply_typed_file_operation({
                "kind": "head", "file": str(source), "output": str(filtered), "count": 1,
            }, root)
            self.assertEqual(filtered.read_bytes(), b"keep\n")
            moved = root / "moved.txt"
            RUNNER.apply_typed_file_operation({
                "kind": "move", "source": str(filtered), "destination": str(moved),
            }, root)
            link_dir = root / "links"
            link_dir.mkdir()
            RUNNER.apply_typed_file_operation({
                "kind": "symlink", "source": str(moved), "destination": str(link_dir),
            }, root)
            link = link_dir / moved.name
            self.assertTrue(link.is_symlink())
            self.assertEqual(link.resolve(), moved.resolve())

    def test_parent_traversal_and_symlink_directory_escape_are_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as temporary, tempfile.TemporaryDirectory() as outside:
            root, escaped = Path(temporary), Path(outside)
            source = root / "source.txt"
            source.write_bytes(b"data\n")
            with self.assertRaises(RuntimeError):
                RUNNER.apply_typed_file_operation({
                    "kind": "head", "file": str(source),
                    "output": str(root / ".." / "escaped.txt"), "count": 1,
                }, root)
            (root / "link").symlink_to(escaped, target_is_directory=True)
            with self.assertRaises(RuntimeError):
                RUNNER.apply_typed_file_operation({
                    "kind": "touch", "files": [str(root / "link" / "created")],
                }, root)
            self.assertFalse((escaped / "created").exists())

    def test_symlink_action_can_replace_escaping_final_entry(self) -> None:
        with tempfile.TemporaryDirectory() as temporary, tempfile.TemporaryDirectory() as outside:
            root, escaped = Path(temporary), Path(outside)
            source = root / "source.so"
            source.write_bytes(b"fixture")
            target = root / "translated.so"
            target.symlink_to(escaped / "old.so")

            RUNNER.apply_typed_file_operation({
                "kind": "symlink",
                "source": str(source),
                "destination": str(target),
            }, root)

            self.assertTrue(target.is_symlink())
            self.assertEqual(target.resolve(), source.resolve())

    def test_line_count_and_check_false_do_not_bypass_confinement(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            source = root / "source.txt"
            source.write_bytes(b"one\ntwo\n")
            outside = root.parent / "outside-count.txt"
            try:
                with self.assertRaises(RuntimeError):
                    RUNNER.apply_typed_file_operation({
                        "kind": "head", "file": str(source),
                        "output": str(root / "output.txt"),
                        "count": {"kind": "line_count", "file": str(outside)},
                    }, root)
                with self.assertRaises(RuntimeError):
                    RUNNER.apply_typed_shell_action({
                        "check": False,
                        "operations": [{"kind": "truncate", "file": str(root / "../outside")}],
                    }, root)
            finally:
                outside.unlink(missing_ok=True)

    def test_boolean_head_count_is_not_an_integer_literal(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            source = root / "source.txt"
            source.write_bytes(b"one\ntwo\n")
            with self.assertRaises(RuntimeError):
                RUNNER.apply_typed_file_operation({
                    "kind": "head", "file": str(source),
                    "output": str(root / "output.txt"), "count": True,
                }, root)

    def test_host_log_cursor_preserves_order_without_duplicates(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            host_log = root / "host.log"
            output = bytearray()
            host_log.write_bytes(b"first\n")
            cursor = RUNNER.append_new_host_log_bytes(host_log, 0, output)
            self.assertEqual(output, b"first\n")
            host_log.write_bytes(b"first\nsecond\n")
            cursor = RUNNER.append_new_host_log_bytes(host_log, cursor, output)
            self.assertEqual(output, b"first\nsecond\n")
            # A truncation is a new stream lifecycle, not a reason to replay
            # bytes from the old host log.
            host_log.write_bytes(b"fresh\n")
            RUNNER.append_new_host_log_bytes(host_log, cursor, output)
            self.assertEqual(output, b"first\nsecond\nfresh\n")

    def test_sed_basic_regex_supports_capture_groups(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            source = root / "stderr"
            source.write_bytes(b"prefix ClassVerificationTotalTimeDelta suffix\nother\n")
            RUNNER.apply_typed_file_operation({
                "kind": "sed_filter",
                "extended_regex": False,
                "print_only_matches": True,
                "scripts": [{
                    "kind": "replace",
                    "pattern": r".*\(ClassVerificationTotalTimeDelta\).*",
                    "replacement": r"\1",
                    "flags": "p",
                }],
                "file": str(source),
            }, root)
            self.assertEqual(source.read_bytes(), b"ClassVerificationTotalTimeDelta\n")


if __name__ == "__main__":
    unittest.main()
