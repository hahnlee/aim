#!/usr/bin/env python3
"""Unit and corpus tests for the non-executing javac_post frontend."""

from __future__ import annotations

import importlib.util
import json
from pathlib import Path
import sys
import tempfile
import unittest


_MODULE_PATH = Path(__file__).with_name("art_javac_post_contract.py")
_SPEC = importlib.util.spec_from_file_location("art_javac_post_contract", _MODULE_PATH)
assert _SPEC is not None and _SPEC.loader is not None
contract = importlib.util.module_from_spec(_SPEC)
sys.modules[_SPEC.name] = contract
_SPEC.loader.exec_module(contract)

ROOT = _MODULE_PATH.parent.parent


class JavacPostContractTest(unittest.TestCase):
    def test_full_numeric_corpus_has_no_opaque_shell(self) -> None:
        contracts, errors = contract.audit(ROOT)
        self.assertEqual(errors, [])
        self.assertEqual(len(contracts), 17)
        self.assertTrue(all(item.operations for item in contracts))
        encoded = json.dumps([contract.contract_json(item) for item in contracts])
        self.assertNotIn("$(", encoded)
        self.assertNotIn("bash", encoded)
        self.assertNotIn("shell", encoded)

    def test_typed_transformer_and_branch_operations(self) -> None:
        source = """\
set -e
export ASM_JAR="${ANDROID_BUILD_TOP}/prebuilts/misc/common/asm/asm-9.6.jar"
mv $1 $1-intermediate-classes
mkdir $1
transformer_args="-cp ${ASM_JAR}:$PWD/transformer.jar transformer.ConstantTransformer"
for class in $1-intermediate-classes/*.class ; do
  if [[ $class == */FooConflict.class ]]; then
    javap -c -v -p $class >> /tmp/2277-javac-output
  fi
  transformed_class=$1/$(basename ${class})
  ${JAVA:-java} ${transformer_args} ${class} ${transformed_class}
done
"""
        result = contract.compile_contract(source, "/tmp/fixture/javac_post.sh")
        self.assertEqual([item.kind for item in result.operations],
                         ["set_errexit", "set_tool_path", "move", "mkdir",
                          "set_transformer", "for_each_class"])
        loop = result.operations[-1]
        self.assertEqual(loop.arguments["glob"].value["root"], "argument1")
        branch = loop.arguments["body"][0]
        self.assertEqual(branch.kind, "branch")
        self.assertEqual(branch.arguments["condition"].kind, "class_matches")
        self.assertEqual(branch.arguments["then"][0].kind, "capture_javap")
        self.assertEqual(loop.arguments["body"][2].kind, "transform_class")

    def test_rejects_unknown_commands_and_shell_injection(self) -> None:
        rejected = (
            "definitely-not-a-command\n",
            "rm -f ../outside.class\n",
            "rm -f /tmp/outside.class\n",
            "rm -f classes/A.class; touch /tmp/pwned\n",
            "rm -f classes/$(touch /tmp/pwned)\n",
            "rm -f classes/A.class && echo nope\n",
            "rm -f classes/A.class > /tmp/out\n",
            "for class in ../*.class; do\n  rm -f ${class}\ndone\n",
        )
        for source in rejected:
            with self.subTest(source=source), self.assertRaises(contract.ContractError):
                contract.compile_contract(source, "/tmp/adversarial/javac_post.sh")

    def test_only_the_known_basename_substitution_is_typed(self) -> None:
        accepted = """\
for class in intermediate-classes/*.class; do
  transformed_class=classes/$(basename ${class})
done
"""
        result = contract.compile_contract(accepted, "/tmp/fixture/javac_post.sh")
        value = result.operations[0].arguments["body"][0].arguments["value"]
        self.assertEqual(value.kind, "cwd_classes_basename_path")
        with self.assertRaises(contract.ContractError):
            contract.compile_contract(
                "x=classes/$(basename; touch /tmp/pwned)\n",
                "/tmp/adversarial/javac_post.sh")

    def test_rejects_invalid_source_bytes(self) -> None:
        with self.assertRaises(contract.ContractError):
            contract.compile_contract(b"rm -f classes/A.class\xff\n", "bad.sh")

    def test_strict_cli_rejects_unsupported_script(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            test = root / "_aosp" / "art" / "test" / "123-strict"
            test.mkdir(parents=True)
            (test / "javac_post.sh").write_text("touch ../outside\n")
            self.assertEqual(contract.main(["--root", str(root), "--strict"]), 1)


if __name__ == "__main__":
    unittest.main()
