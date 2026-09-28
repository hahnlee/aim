#!/usr/bin/env python3
"""Self-tests for the fail-closed ART run contract compiler."""

from __future__ import annotations

from pathlib import Path
import sys
import tempfile


sys.path.insert(0, str(Path(__file__).parent))
import art_run_contract as MODULE  # noqa: E402


def test_supported_context_calls() -> None:
    contract = MODULE.compile_source(
        """
def run(ctx, args):
  ctx.default_run(args, jvmti=True, runtime_option=["-Xopaque-jni-ids:true"])
  ctx.echo(f"{ctx.env.TEST_NAME}")
""",
        "supported.py",
    )
    assert contract.supported
    assert "ctx.default_run" in contract.capabilities
    assert "ctx.echo" in contract.capabilities
    assert contract.nodes[0]["kind"] == "context_call"
    assert contract.nodes[0]["kwargs"][0]["name"] == "jvmti"


def test_branch_and_loop_are_preserved() -> None:
    contract = MODULE.compile_source(
        """
def run(ctx, args):
  if args.jvm:
    ctx.default_run(args)
  for value in args.runtime_option:
    ctx.echo(value)
""",
        "control.py",
    )
    assert contract.supported
    assert {"branch", "loop"} <= contract.capabilities
    assert [node["kind"] for node in contract.nodes] == ["branch", "loop"]


def test_unsupported_call_is_not_dropped() -> None:
    contract = MODULE.compile_source(
        """
def run(ctx, args):
  ctx.default_run(args, runtime_option=make_options())
""",
        "mutation.py",
    )
    assert not contract.supported
    assert contract.nodes[0]["kind"] == "context_call"
    assert contract.nodes[0]["kwargs"][0]["value"]["kind"] == "unsupported"
    assert "expression call is not an allowed context API" in contract.issues[0].reason


def test_expression_metadata_is_preserved_and_expansion_is_rejected() -> None:
    contract = MODULE.compile_source(
        """
def run(ctx, args):
  ctx.echo(f"{args.value:.2f}")
  ctx.echo({"safe": 1, **args.extra})
""",
        "expressions.py",
    )
    assert not contract.supported
    formatted = contract.nodes[0]["args"][0]["parts"][0]
    assert formatted["conversion"] == -1
    assert formatted["format_spec"]["kind"] == "f_string"
    assert contract.nodes[1]["args"][0]["kind"] == "mapping"
    assert contract.nodes[1]["args"][0]["items"][1]["key"] is None
    assert any("mapping ** expansion" in issue.reason for issue in contract.issues)


def test_no_import_or_execution() -> None:
    with tempfile.TemporaryDirectory() as directory:
        marker = Path(directory) / "marker"
        source = f"open({str(marker)!r}, 'w').write('executed')\n\ndef run(ctx, args):\n  ctx.default_run(args)\n"
        contract = MODULE.compile_source(source, "effects.py")
        assert not marker.exists()
        assert not contract.supported
        assert any("top-level executable code" in issue.reason for issue in contract.issues)


def test_function_metadata_is_fail_closed() -> None:
    contract = MODULE.compile_source(
        """
@decorate
async def run(ctx, args, optional=True):
  ctx.default_run(args)
""",
        "metadata.py",
    )
    assert not contract.supported
    reasons = [issue.reason for issue in contract.issues]
    assert any("async run" in reason for reason in reasons)
    assert any("decorators" in reason for reason in reasons)
    assert any("default arguments" in reason for reason in reasons)


def test_typed_shell_grammar_and_adversarial_rejections() -> None:
    supported = MODULE.compile_source(
        """
def run(ctx, args):
  ctx.run(f"head -n $(wc -l < '{args.stdout_file}') expected.txt > expected.tmp && mv expected.tmp '{args.stdout_file}'")
""",
        "line-count.py",
    )
    assert supported.supported
    sequence = supported.nodes[0]["command"]
    assert [operation["kind"] for operation in sequence["operations"]] == ["head", "move"]
    assert sequence["operations"][0]["count"]["kind"] == "line_count"
    assert sequence["operations"][0]["file"]["policy"] == "sandbox-relative-no-escape"

    cases = {
        "cat '{args.stdout_file}'; rm -rf output": "shell operator",
        "cat '{args.stdout_file}' | sed -n '1p'": "shell operator",
        "cat $(id)": "typed Contract IR grammar",
        "cat -n '{args.stdout_file}'": "typed Contract IR grammar",
        "head -n $(echo 1) input > output": "typed Contract IR grammar",
    }
    for command, expected in cases.items():
        contract = MODULE.compile_source(
            f"def run(ctx, args):\n  ctx.run(f\"{command}\")\n",
            "adversarial.py",
        )
        assert not contract.supported, (command, contract.as_dict())
        assert any(expected in issue.reason for issue in contract.issues), (command, contract.as_dict())

    path_contract = MODULE.compile_source(
        """
def run(ctx, args):
  ctx.run("cat '../outside'")
""",
        "path-policy.py",
    )
    assert path_contract.supported
    path = path_contract.nodes[0]["command"]["operations"][0]["file"]
    assert path["policy"] == "sandbox-relative-no-escape"


def test_observed_ast_contracts_and_adversarial_rejections() -> None:
    observed = MODULE.compile_source(
        """
import os
import sys
import re
import resource

def run(ctx, args):
  bridge = "release" if args.O else "debug"
  if os.environ.get("ART_TEST_ON_VM"):
    ctx.expected_stdout = ctx.expected_stdout.with_suffix(".jvm.txt")
  for i, opt in enumerate(args.runtime_option):
    if opt.startswith("-Djava.library.path="):
      args.runtime_option[i] = "-Djava.library.path=" + bridge
      args.runtime_option.pop(i)
      break
""",
        "observed.py",
    )
    assert observed.supported, observed.as_dict()
    assert {"inert_import", "env_get", "path_transform", "live_list_iteration", "live_list_mutation"} <= observed.capabilities

    rejected_sources = {
        "import subprocess\ndef run(ctx, args):\n  ctx.default_run(args)\n": "inert allowlist",
        "import os\ndef run(ctx, args):\n  if os.environ.get('OTHER'):\n    pass\n": "only the observed ART_TEST_ON_VM",
        "def run(ctx, args):\n  ctx.expected_stdout = ctx.expected_stdout.with_suffix(args.suffix)\n": "literal suffix",
        "def run(ctx, args):\n  args.other.pop(0)\n": "not an allowed context API",
        "def run(ctx, args):\n  if foo.startswith('x'):\n    pass\n": "not an allowed context API",
        "def run(ctx, args):\n  break\n": "break outside a loop",
    }
    for source, expected in rejected_sources.items():
        contract = MODULE.compile_source(source, "rejected.py")
        assert not contract.supported, (source, contract.as_dict())
        assert any(expected in issue.reason for issue in contract.issues), (source, contract.as_dict())
        assert all(issue.span.line > 0 for issue in contract.issues)


def test_evaluator_order_snapshots_and_live_mutation() -> None:
    contract = MODULE.compile_source(
        """
def run(ctx, args):
  if args.jvm:
    ctx.expected_stdout = ctx.expected_stdout.with_suffix(".jvm.txt")
  ctx.default_run(args, runtime_option=["first"])
  args.runtime_option[0] = "second"
  ctx.default_run(args, runtime_option=args.runtime_option)
  ctx.echo(ctx.expected_stdout)
""",
        "evaluate.py",
    )
    plan = MODULE.evaluate_contract(
        contract,
        {
            "args": {"jvm": True, "runtime_option": ["original"]},
            "env": {},
            "expected_stdout": "/tmp/art-contract/expected.stdout",
            "path_root": "/tmp/art-contract",
        },
        "unit",
    )
    assert plan.supported, plan.as_dict()
    assert [action["kind"] for action in plan.actions] == ["default_run", "default_run", "echo"]
    assert plan.actions[0]["args_snapshot"]["runtime_option"] == ["original"]
    assert plan.actions[0]["expected_stdout_snapshot"] == "/tmp/art-contract/expected.jvm.txt"
    assert plan.actions[1]["args_snapshot"]["runtime_option"] == ["second"]
    assert plan.actions[2]["args"] == ["/tmp/art-contract/expected.jvm.txt"]


def test_evaluator_live_enumerate_pop_and_path_boundary() -> None:
    contract = MODULE.compile_source(
        """
def run(ctx, args):
  for i, opt in enumerate(args.runtime_option):
    if opt == "remove":
      args.runtime_option.pop(i)
      break
  ctx.default_run(args)
  ctx.run(f"cat '{args.stdout_file}'")
""",
        "live.py",
    )
    context = {
        "args": {"runtime_option": ["keep", "remove", "tail"], "stdout_file": "/tmp/art-contract/out"},
        "env": {},
        "expected_stdout": "/tmp/art-contract/expected.stdout",
        "path_root": "/tmp/art-contract",
    }
    plan = MODULE.evaluate_contract(contract, context, "unit")
    assert plan.supported, plan.as_dict()
    assert plan.actions[0]["args_snapshot"]["runtime_option"] == ["keep", "tail"]
    assert plan.actions[1]["operations"][0]["file"] == "/tmp/art-contract/out"

    escaped = dict(context)
    escaped["args"] = {"runtime_option": [], "stdout_file": "../../outside"}
    escaped_plan = MODULE.evaluate_contract(contract, escaped, "escape")
    assert not escaped_plan.supported
    assert any("escapes" in issue.reason for issue in escaped_plan.issues)


def test_evaluator_unknown_branch_fails_closed() -> None:
    contract = MODULE.compile_source(
        """
def run(ctx, args):
  if args.unknown:
    ctx.default_run(args)
""",
        "unknown.py",
    )
    plan = MODULE.evaluate_contract(
        contract,
        {"args": {}, "env": {}, "expected_stdout": "/tmp/art-contract/expected.stdout", "path_root": "/tmp/art-contract"},
        "unit",
    )
    assert not plan.supported
    assert not plan.actions
    assert "unknown attribute value" in plan.issues[0].reason


def test_evaluator_materializes_literal_line_count() -> None:
    contract = MODULE.compile_source(
        """
def run(ctx, args):
  ctx.run(f"tail -n 1 '{args.stdout_file}' > temp.txt")
""",
        "literal-count.py",
    )
    plan = MODULE.evaluate_contract(
        contract,
        {
            "args": {"stdout_file": "/tmp/art-contract/out"},
            "env": {},
            "expected_stdout": "/tmp/art-contract/expected.stdout",
            "path_root": "/tmp/art-contract",
        },
        "unit",
    )
    assert plan.supported, plan.as_dict()
    assert plan.actions[0]["operations"][0]["count"] == 1


def test_149_literal_tail_count_is_concrete() -> None:
    root = Path(__file__).resolve().parent.parent
    contract = MODULE.compile_path(root / "_aosp/art/test/149-suspend-all-stress/run.py")
    plan = MODULE.evaluate_contract(
        contract, MODULE.representative_contexts()["host"], "host")
    assert plan.supported, plan.as_dict()
    assert plan.actions[1]["operations"][0] == {
        "kind": "tail",
        "count": 1,
        "file": "/tmp/art-contract/stdout.txt",
        "output": "/tmp/art-contract/temp-stdout.txt",
    }


def test_representative_corpus_evaluation() -> None:
    root = Path(__file__).resolve().parent.parent
    paths = MODULE.discover(root, [])
    assert len(paths) == 348
    for variant, context in MODULE.representative_contexts().items():
        plans = [MODULE.evaluate_contract(MODULE.compile_path(path), context, variant) for path in paths]
        assert all(plan.supported for plan in plans), [plan.as_dict() for plan in plans if not plan.supported]


def main() -> None:
    test_supported_context_calls()
    test_branch_and_loop_are_preserved()
    test_unsupported_call_is_not_dropped()
    test_expression_metadata_is_preserved_and_expansion_is_rejected()
    test_no_import_or_execution()
    test_function_metadata_is_fail_closed()
    test_typed_shell_grammar_and_adversarial_rejections()
    test_observed_ast_contracts_and_adversarial_rejections()
    test_evaluator_order_snapshots_and_live_mutation()
    test_evaluator_live_enumerate_pop_and_path_boundary()
    test_evaluator_unknown_branch_fails_closed()
    test_evaluator_materializes_literal_line_count()
    test_149_literal_tail_count_is_concrete()
    test_representative_corpus_evaluation()
    print("art-run-contract self-tests: PASS")


if __name__ == "__main__":
    main()
