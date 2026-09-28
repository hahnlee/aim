#!/usr/bin/env python3
"""Focused tests for the AST-only build contract compiler."""

from __future__ import annotations

from contextlib import redirect_stderr, redirect_stdout
from io import StringIO
import json
from pathlib import Path
from tempfile import TemporaryDirectory
import unittest

try:
    from tools import art_build_contract as contract
except ModuleNotFoundError:  # Direct ``python tools/test_art_build_contract.py``.
    import art_build_contract as contract


class BuildContractTest(unittest.TestCase):
    def test_preserves_order_dynamic_paths_and_file_operations(self) -> None:
        source = """\
import os

def build(ctx):
    ctx.bash('./generate-sources --' + ctx.mode)
    ctx.default_build(use_hiddenapi=True, delete_srcs=False)
    os.mkdir(ctx.test_dir / 'res')
    os.rename(ctx.test_dir / 'input.jar', ctx.test_dir / 'res/boot.jar')
    ctx.bash('rm -rf classes*')
    ctx.default_build(use_hiddenapi=False)
"""
        result = contract.compile_contract(source, "/tmp/817-hiddenapi/build.py")
        self.assertEqual([action.kind for action in result.actions],
                         ["generate_sources", "default_build", "mkdir", "rename", "delete",
                          "default_build"])
        self.assertEqual(result.actions[0].arguments["argv"][1].kind, "string_concat")
        self.assertEqual(result.actions[2].arguments["path"].kind, "path_join")
        self.assertEqual(result.actions[1].span.line, 5)

    def test_preserves_jvm_branch_and_soong_zip(self) -> None:
        source = """\
def build(ctx):
    ctx.default_build(d8_dex_container=False)
    if ctx.jvm:
        return
    (ctx.test_dir / 'out.jar').unlink()
    ctx.soong_zip(['-o', ctx.test_dir / 'out.jar', '-j', '-f', ctx.test_dir / 'classes.dex'])
"""
        result = contract.compile_contract(source, "/tmp/test/build.py")
        branch = result.actions[1]
        self.assertEqual(branch.kind, "branch")
        self.assertEqual(branch.arguments["condition"].kind, "context_bool")
        self.assertEqual(branch.arguments["then"][0].kind, "return")
        self.assertEqual(result.actions[2].kind, "delete")
        self.assertEqual(result.actions[3].kind, "soong_zip")

    def test_unknown_keyword_and_call_fail_closed(self) -> None:
        with self.assertRaises(contract.ContractError):
            contract.compile_contract(
                "def build(ctx):\n    ctx.default_build(maybe_new=True)\n",
                "/tmp/unknown/build.py")
        with self.assertRaises(contract.ContractError):
            contract.compile_contract(
                "def build(ctx):\n    ctx.run_shell('rm -rf .')\n",
                "/tmp/unknown/build.py")
        with self.assertRaises(contract.ContractError):
            contract.compile_contract(
                "def build(ctx):\n    ctx.bash('rm -rf output')\n",
                "/tmp/unknown/build.py")
        with self.assertRaises(contract.ContractError):
            contract.compile_contract(
                "def build(ctx):\n    ctx.bash('echo $(touch /tmp/pwned)')\n",
                "/tmp/unknown/build.py")

    def test_strict_cli_requires_zero_unsupported(self) -> None:
        with TemporaryDirectory() as directory:
            root = Path(directory)
            test = root / "_aosp" / "art" / "test" / "123-strict"
            test.mkdir(parents=True)
            (test / "build.py").write_text(
                "def build(ctx):\n    ctx.run_shell('unsupported')\n",
                encoding="utf-8")
            output = StringIO()
            errors = StringIO()
            with redirect_stdout(output), redirect_stderr(errors):
                status = contract.main(["--root", str(root), "--strict"])
            self.assertEqual(status, 1)
            self.assertIn("unsupported=1", output.getvalue())
            self.assertIn("run_shell", errors.getvalue())

    def test_contract_json_serializes_byte_literals(self) -> None:
        result = contract.compile_contract(
            "def build(ctx):\n"
            "    with open(ctx.test_dir / 'classes.dex', 'rb+') as f:\n"
            "        assert f.read(4) == b'dex\\n'\n",
            "/tmp/bytes/build.py")
        encoded = json.dumps(contract.contract_json(result))
        self.assertIn('"encoding": "hex"', encoded)
        self.assertIn('"6465780a"', encoded)


if __name__ == "__main__":
    unittest.main()
