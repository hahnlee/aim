#!/usr/bin/env python3
"""Safety and execution tests for typed ART ActionPlans."""

from __future__ import annotations

import importlib.util
from pathlib import Path
import sys
import tempfile
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from tools.art_action_plan import (ActionContext, ActionPlan, ActionPlanError,
                                   PlannedOperation, dry_run_corpus,
                                   evaluate_contract)


def _load(name: str, filename: str):
    path = Path(__file__).with_name(filename)
    spec = importlib.util.spec_from_file_location(name, path)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


build = _load("action_plan_build_contract", "art_build_contract.py")
javac = _load("action_plan_javac_contract", "art_javac_post_contract.py")


class ActionPlanTest(unittest.TestCase):
    def test_build_generate_delete_mkdir_and_move_use_file_apis(self) -> None:
        source = """\
import os
def build(ctx):
    ctx.bash('./generate-sources')
    os.mkdir(ctx.test_dir / 'made')
    os.rename(ctx.test_dir / 'old', ctx.test_dir / 'new')
    ctx.bash('rm -rf classes*')
"""
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / "old").write_bytes(b"move me")
            (root / "classes").mkdir()
            (root / "classes2").mkdir()
            generator = root / "generate-sources"
            generator.write_text(
                "#!/usr/bin/env python3\n"
                "from pathlib import Path\n"
                "Path('generated.marker').write_text('ok')\n")
            generator.chmod(0o755)
            contract = build.compile_contract(source, root / "build.py")
            plan = evaluate_contract(contract, ActionContext(root))
            plan.execute(ActionContext(root))
            self.assertEqual((root / "generated.marker").read_text(), "ok")
            self.assertEqual((root / "new").read_bytes(), b"move me")
            self.assertTrue((root / "made").is_dir())
            self.assertFalse((root / "classes").exists())
            self.assertFalse((root / "classes2").exists())

    def test_file_edit_assertion_and_write_are_typed(self) -> None:
        source = """\
import os
def build(ctx):
    with open(ctx.test_dir / 'classes.dex', 'rb+') as f:
        assert f.read(8) == b'dex\\n035\\0'
        f.seek(4)
        f.write(b'037\\0')
"""
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            target = root / "classes.dex"
            target.write_bytes(b"dex\n035\0payload")
            plan = evaluate_contract(build.compile_contract(source, root / "build.py"),
                                     ActionContext(root))
            plan.execute(ActionContext(root))
            self.assertEqual(target.read_bytes()[:8], b"dex\n037\0")

    def test_javac_class_transform_and_move_are_argv_operations(self) -> None:
        source_path = Path(__file__).parents[1] / "_aosp/art/test/2265-const-method-type-cached/javac_post.sh"
        with tempfile.TemporaryDirectory() as temporary, tempfile.TemporaryDirectory() as toolchain:
            root, tools = Path(temporary), Path(toolchain)
            classes = root / "classes"
            classes.mkdir()
            (classes / "A.class").write_bytes(b"input")
            (root / "transformer.jar").write_bytes(b"jar")
            asm = tools / "prebuilts/misc/common/asm/asm-9.6.jar"
            asm.parent.mkdir(parents=True)
            asm.write_bytes(b"asm")
            java = tools / "java"
            java.write_text(
                "#!/usr/bin/env python3\n"
                "import pathlib, sys\n"
                "pathlib.Path(sys.argv[-1]).write_bytes(pathlib.Path(sys.argv[-2]).read_bytes()+b'-out')\n")
            java.chmod(0o755)
            contract = javac.compile_contract(source_path.read_bytes(), source_path)
            context = ActionContext(root, toolchain_root=tools, argument1=classes,
                                    tools={"java": java})
            plan = evaluate_contract(contract, context)
            plan.execute(context)
            self.assertEqual((classes / "A.class").read_bytes(), b"input-out")

    def test_execution_rechecks_symlink_escape_and_unknown_action(self) -> None:
        source = """\
import os
def build(ctx):
    os.mkdir(ctx.test_dir / 'link/out')
"""
        with tempfile.TemporaryDirectory() as temporary, tempfile.TemporaryDirectory() as outside:
            root, escaped = Path(temporary), Path(outside)
            (root / "link").symlink_to(escaped, target_is_directory=True)
            contract = build.compile_contract(source, root / "build.py")
            with self.assertRaises(ActionPlanError):
                evaluate_contract(contract, ActionContext(root))
            span = contract.actions[0].span
            with self.assertRaises(ActionPlanError):
                ActionPlan(str(root / "build.py"),
                           (PlannedOperation("not-allowed", {}, span),)).execute(ActionContext(root))

    def test_corpus_dry_run_covers_every_contract_without_generators(self) -> None:
        count, errors = dry_run_corpus(Path(__file__).parents[1])
        self.assertEqual(count, 141)
        self.assertEqual(errors, [])


if __name__ == "__main__":
    unittest.main()
