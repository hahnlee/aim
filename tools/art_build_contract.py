#!/usr/bin/env python3
"""Compile AOSP ``build.py`` files into a small, fail-closed IR.

This is deliberately an AST compiler, not a Python runner.  A build contract
is data which a later build backend can consume; importing a test's script (or
executing a string found in one) would make auditing both non-deterministic and
unsafe.  The accepted language is the small, documented subset used by ART's
numeric tests.  New syntax must be added here explicitly instead of being
silently approximated.
"""

from __future__ import annotations

import argparse
import ast
from dataclasses import asdict, dataclass
import json
from pathlib import Path
import re
import shlex
import sys
from typing import Any, Iterable


class ContractError(ValueError):
    """A build script contains syntax outside the audited contract language."""

    def __init__(self, path: Path, node: ast.AST | None, message: str) -> None:
        line = getattr(node, "lineno", 1)
        column = getattr(node, "col_offset", 0)
        super().__init__(f"{path}:{line}:{column + 1}: {message}")
        self.path = path
        self.node = node


@dataclass(frozen=True)
class SourceSpan:
    path: str
    line: int
    column: int
    end_line: int
    end_column: int


@dataclass(frozen=True)
class ValueIR:
    """A typed, unresolved expression (for example ``ctx.test_dir / 'x'``)."""

    kind: str
    value: Any
    span: SourceSpan


@dataclass(frozen=True)
class FileOperationIR:
    kind: str
    arguments: dict[str, Any]
    span: SourceSpan


@dataclass(frozen=True)
class ActionIR:
    kind: str
    arguments: dict[str, Any]
    span: SourceSpan


@dataclass(frozen=True)
class BuildContract:
    test: str
    source: str
    actions: tuple[ActionIR, ...]


DEFAULT_BUILD_KWARGS = frozenset({
    "use_desugar", "use_hiddenapi", "need_dex", "zip_compression_method",
    "zip_align_bytes", "api_level", "javac_args", "javac_classpath",
    "d8_flags", "d8_dex_container", "smali_args", "use_smali", "use_jasmin",
    "javac_source_arg", "javac_target_arg", "delete_srcs",
})
IMPORTS = frozenset({"os", "shutil"})
PATH_CONTEXT = {"test_dir": "path"}
STRING_CONTEXT = {"mode": "string"}
BOOL_CONTEXT = {"jvm": "bool"}


def _span(path: Path, node: ast.AST) -> SourceSpan:
    return SourceSpan(
        str(path),
        getattr(node, "lineno", 1),
        getattr(node, "col_offset", 0),
        getattr(node, "end_lineno", getattr(node, "lineno", 1)),
        getattr(node, "end_col_offset", getattr(node, "col_offset", 0)),
    )


def _serial(value: Any) -> Any:
    if isinstance(value, bytes):
        # Keep byte literals unambiguous in the diagnostic JSON without
        # decoding arbitrary source bytes as text.
        return {"encoding": "hex", "value": value.hex()}
    if isinstance(value, (SourceSpan, ValueIR, FileOperationIR, ActionIR, BuildContract)):
        return {key: _serial(item) for key, item in asdict(value).items()}
    if isinstance(value, tuple):
        return [_serial(item) for item in value]
    if isinstance(value, list):
        return [_serial(item) for item in value]
    if isinstance(value, dict):
        return {key: _serial(item) for key, item in value.items()}
    return value


def contract_json(contract: BuildContract) -> dict[str, Any]:
    return _serial(contract)


class _Compiler:
    def __init__(self, path: Path) -> None:
        self.path = path

    def error(self, node: ast.AST, message: str) -> ContractError:
        return ContractError(self.path, node, message)

    def value(self, node: ast.AST, *, locals: frozenset[str] = frozenset()) -> ValueIR:
        span = _span(self.path, node)
        if isinstance(node, ast.Constant):
            if node.value is None or isinstance(node.value, (bool, int, str, bytes)):
                return ValueIR("literal", node.value, span)
            raise self.error(node, "unsupported literal type")
        if isinstance(node, ast.Name):
            raise self.error(node, f"unknown name {node.id!r}")
        if isinstance(node, ast.Attribute):
            if isinstance(node.value, ast.Name) and node.value.id == "ctx":
                if node.attr in PATH_CONTEXT:
                    return ValueIR("context_path", node.attr, span)
                if node.attr in STRING_CONTEXT:
                    return ValueIR("context_string", node.attr, span)
                if node.attr in BOOL_CONTEXT:
                    return ValueIR("context_bool", node.attr, span)
            raise self.error(node, "attribute is not an approved context value")
        if isinstance(node, ast.BinOp):
            if isinstance(node.op, ast.Div):
                left = self.value(node.left, locals=locals)
                right = self.value(node.right, locals=locals)
                if left.kind not in {"context_path", "path_join"}:
                    raise self.error(node, "path division must start with ctx.test_dir")
                return ValueIR("path_join", {"left": left, "right": right}, span)
            if isinstance(node.op, ast.Add):
                left = self.value(node.left, locals=locals)
                right = self.value(node.right, locals=locals)
                if left.kind not in {"literal", "context_string", "string_concat"}:
                    raise self.error(node, "string concatenation has an invalid left operand")
                if right.kind not in {"literal", "context_string", "string_concat"}:
                    raise self.error(node, "string concatenation has an invalid right operand")
                return ValueIR("string_concat", {"left": left, "right": right}, span)
            raise self.error(node, "only path division and string addition are supported")
        if isinstance(node, (ast.List, ast.Tuple)):
            items = tuple(self.value(item, locals=locals) for item in node.elts)
            return ValueIR("list" if isinstance(node, ast.List) else "tuple", items, span)
        if isinstance(node, ast.Dict):
            if any(key is None for key in node.keys):
                raise self.error(node, "dictionary unpacking is not supported")
            items = tuple((self.value(key, locals=locals), self.value(item, locals=locals))
                          for key, item in zip(node.keys, node.values))
            return ValueIR("dict", items, span)
        # File reads are only legal while compiling a with-open body.  Keeping
        # them as IR means a backend can implement the exact mutation without
        # evaluating arbitrary Python.
        if isinstance(node, ast.Call):
            if (isinstance(node.func, ast.Attribute)
                    and isinstance(node.func.value, ast.Name)
                    and node.func.value.id in locals
                    and node.func.attr == "read"):
                if node.keywords or len(node.args) > 1:
                    raise self.error(node, "file.read accepts at most one positional argument")
                size = self.value(node.args[0], locals=locals) if node.args else None
                return ValueIR("file_read", {"handle": node.func.value.id, "size": size}, span)
        raise self.error(node, f"unsupported expression {ast.unparse(node)!r}")

    def condition(self, node: ast.AST, *, locals: frozenset[str] = frozenset()) -> ValueIR:
        if isinstance(node, ast.UnaryOp) and isinstance(node.op, ast.Not):
            return ValueIR("not", self.condition(node.operand, locals=locals), _span(self.path, node))
        if isinstance(node, ast.Compare) and len(node.ops) == 1 and len(node.comparators) == 1:
            if not isinstance(node.ops[0], ast.Eq):
                raise self.error(node, "only equality conditions are supported")
            return ValueIR("equals", {
                "left": self.value(node.left, locals=locals),
                "right": self.value(node.comparators[0], locals=locals),
            }, _span(self.path, node))
        value = self.value(node, locals=locals)
        if value.kind != "context_bool":
            raise self.error(node, "condition must be ctx.jvm or an approved equality")
        return value

    @staticmethod
    def _qualified(node: ast.Call) -> tuple[str, ...] | None:
        parts: list[str] = []
        current: ast.AST = node.func
        while isinstance(current, ast.Attribute):
            parts.append(current.attr)
            current = current.value
        if isinstance(current, ast.Name):
            parts.append(current.id)
            return tuple(reversed(parts))
        return None

    def command_action(self, node: ast.Call) -> ActionIR:
        """Compile the tiny command vocabulary used by ART build.py.

        ``ctx.bash`` is not a shell escape hatch in the IR.  The current AOSP
        corpus only uses the source generator and one cleanup command.  Both
        are represented as typed actions; every other command, including shell
        expansion or redirection, is rejected with its source span.
        """
        command = node.args[0]
        if isinstance(command, ast.Constant) and isinstance(command.value, str):
            try:
                argv = shlex.split(command.value, posix=True)
            except ValueError as error:
                raise self.error(command, f"invalid command quoting: {error}") from error
            if argv == ["./generate-sources"]:
                return ActionIR("generate_sources", {
                    "argv": (ValueIR("literal", "./generate-sources", _span(self.path, command)),),
                }, _span(self.path, node))
            if argv == ["rm", "-rf", "classes*"]:
                return ActionIR("delete", {
                    "path": ValueIR("glob", {"pattern": "classes*"},
                                    _span(self.path, command)),
                    "recursive": True,
                }, _span(self.path, node))
            raise self.error(command, "unsupported ctx.bash command; shell text is not executable IR")
        # The only dynamic command in the AOSP corpus is
        # './generate-sources --' + ctx.mode.  Keep the mode as an expression
        # in argv[1], rather than interpolating or retaining shell text.
        if (isinstance(command, ast.BinOp) and isinstance(command.op, ast.Add)
                and isinstance(command.left, ast.Constant)
                and command.left.value == "./generate-sources --"):
            mode = self.value(command.right)
            if mode.kind != "context_string" or mode.value != "mode":
                raise self.error(command, "dynamic generator argument must be ctx.mode")
            prefix = ValueIR("literal", "--", _span(self.path, command.left))
            argument = ValueIR("string_concat", {"left": prefix, "right": mode},
                               _span(self.path, command))
            return ActionIR("generate_sources", {
                "argv": (
                    ValueIR("literal", "./generate-sources", _span(self.path, command.left)),
                    argument,
                ),
            }, _span(self.path, node))
        raise self.error(command, "ctx.bash must be a fully recognized command expression")

    def call_action(self, node: ast.Call) -> ActionIR:
        if (isinstance(node.func, ast.Attribute) and node.func.attr == "unlink"
                and isinstance(node.func.value, ast.BinOp)
                and isinstance(node.func.value.op, ast.Div)
                and not node.args and not node.keywords):
            return ActionIR("delete", {"path": self.value(node.func.value),
                                        "recursive": False}, _span(self.path, node))
        qualified = self._qualified(node)
        if qualified is None:
            raise self.error(node, "call target must be a known qualified function")
        if any(keyword.arg is None for keyword in node.keywords):
            raise self.error(node, "keyword unpacking is not supported")
        kwargs = {keyword.arg for keyword in node.keywords if keyword.arg is not None}
        if qualified == ("ctx", "default_build"):
            if node.args:
                raise self.error(node, "default_build accepts keyword arguments only")
            unknown = kwargs - DEFAULT_BUILD_KWARGS
            if unknown:
                raise self.error(node, f"unknown default_build keyword(s): {sorted(unknown)}")
            values = {keyword.arg: self.value(keyword.value)
                      for keyword in node.keywords if keyword.arg is not None}
            return ActionIR("default_build", {"kwargs": values}, _span(self.path, node))
        if qualified == ("ctx", "bash"):
            if len(node.args) != 1 or kwargs:
                raise self.error(node, "ctx.bash accepts exactly one positional command")
            return self.command_action(node)
        if qualified == ("ctx", "soong_zip"):
            if len(node.args) != 1 or kwargs:
                raise self.error(node, "ctx.soong_zip accepts one argv list")
            argv = self.value(node.args[0])
            if argv.kind not in {"list", "tuple"}:
                raise self.error(node.args[0], "ctx.soong_zip argv must be a list or tuple")
            return ActionIR("soong_zip", {"argv": argv}, _span(self.path, node))
        if qualified in {
                ("os", "mkdir"), ("os", "remove"), ("os", "unlink"),
                ("shutil", "rmtree")}:
            if len(node.args) != 1 or kwargs:
                raise self.error(node, f"{'.'.join(qualified)} accepts one positional path")
            return ActionIR("mkdir" if qualified[-1] == "mkdir" else "delete",
                            {"path": self.value(node.args[0]),
                             "recursive": qualified == ("shutil", "rmtree")},
                            _span(self.path, node))
        if qualified in {
                ("os", "rename"), ("shutil", "move"),
                ("shutil", "copy"), ("shutil", "copyfile"),
                ("shutil", "copy2")}:
            if len(node.args) != 2 or kwargs:
                raise self.error(node, f"{'.'.join(qualified)} accepts two positional paths")
            if qualified == ("os", "rename") or qualified == ("shutil", "move"):
                kind = "rename" if qualified == ("os", "rename") else "move"
            else:
                kind = "copy"
            return ActionIR(kind, {"source": self.value(node.args[0]),
                                   "destination": self.value(node.args[1])},
                            _span(self.path, node))
        raise self.error(node, f"call {'.'.join(qualified)!r} is not whitelisted")

    def statements(self, body: Iterable[ast.stmt], *, locals: frozenset[str] = frozenset()) -> tuple[ActionIR, ...]:
        actions: list[ActionIR] = []
        for node in body:
            if isinstance(node, ast.Pass):
                actions.append(ActionIR("noop", {}, _span(self.path, node)))
            elif isinstance(node, ast.Return):
                if node.value is not None:
                    raise self.error(node, "build return values are not supported")
                actions.append(ActionIR("return", {}, _span(self.path, node)))
            elif isinstance(node, ast.Expr) and isinstance(node.value, ast.Call):
                actions.append(self.call_action(node.value))
            elif isinstance(node, ast.Expr) and isinstance(node.value, ast.Attribute):
                # Path.unlink() is the one method form used by ART build.py.
                target = node.value
                if target.attr != "unlink" or not isinstance(target.value, ast.BinOp):
                    raise self.error(node, "expression is not an approved build action")
                actions.append(ActionIR("delete", {"path": self.value(target.value),
                                                     "recursive": False}, _span(self.path, node)))
            elif isinstance(node, ast.If):
                actions.append(ActionIR(
                    "branch", {
                        "condition": self.condition(node.test, locals=locals),
                        "then": self.statements(node.body, locals=locals),
                        "else": self.statements(node.orelse, locals=locals),
                    }, _span(self.path, node)))
            elif isinstance(node, ast.With):
                actions.append(self.with_file(node))
            else:
                raise self.error(node, f"unsupported statement {ast.unparse(node)!r}")
        return tuple(actions)

    def with_file(self, node: ast.With) -> ActionIR:
        if len(node.items) != 1:
            raise self.error(node, "only one file context may be opened")
        item = node.items[0]
        if not (isinstance(item.context_expr, ast.Call)
                and isinstance(item.context_expr.func, ast.Name)
                and item.context_expr.func.id == "open"):
            raise self.error(node, "only open(...) file contexts are supported")
        call = item.context_expr
        if len(call.args) not in {1, 2} or call.keywords:
            raise self.error(call, "open accepts path and an optional literal mode")
        if not isinstance(item.optional_vars, ast.Name):
            raise self.error(node, "open context must bind a simple file name")
        mode = self.value(call.args[1]) if len(call.args) == 2 else ValueIR(
            "literal", "r", _span(self.path, call))
        if mode.kind != "literal" or mode.value not in {"r", "rb", "r+", "rb+", "w", "wb", "w+", "wb+"}:
            raise self.error(call, "open mode must be a supported literal file mode")
        handle = item.optional_vars.id
        operations = self.file_statements(node.body, handle)
        return ActionIR("file_edit", {"path": self.value(call.args[0]),
                                       "mode": mode, "handle": handle,
                                       "operations": operations}, _span(self.path, node))

    def file_statements(self, body: Iterable[ast.stmt], handle: str) -> tuple[FileOperationIR, ...]:
        operations: list[FileOperationIR] = []
        locals = frozenset({handle})
        for node in body:
            if isinstance(node, ast.Assert):
                if not (isinstance(node.test, ast.Compare) and len(node.test.ops) == 1
                        and isinstance(node.test.ops[0], ast.Eq)):
                    raise self.error(node, "file assertions must compare one read with a literal")
                operations.append(FileOperationIR("assert_equals", {
                    "left": self.value(node.test.left, locals=locals),
                    "right": self.value(node.test.comparators[0], locals=locals),
                }, _span(self.path, node)))
            elif isinstance(node, ast.Expr) and isinstance(node.value, ast.Call):
                call = node.value
                if not (isinstance(call.func, ast.Attribute)
                        and isinstance(call.func.value, ast.Name)
                        and call.func.value.id == handle):
                    raise self.error(node, "file body contains an unknown call")
                if call.keywords or len(call.args) != 1:
                    raise self.error(call, "file method requires one positional argument")
                if call.func.attr not in {"seek", "write"}:
                    raise self.error(call, f"file method {call.func.attr!r} is not supported")
                operations.append(FileOperationIR(call.func.attr,
                                                  {"argument": self.value(call.args[0], locals=locals)},
                                                  _span(self.path, call)))
            elif isinstance(node, ast.If):
                operations.append(FileOperationIR("branch", {
                    "condition": self.condition(node.test, locals=locals),
                    "then": self.file_statements(node.body, handle),
                    "else": self.file_statements(node.orelse, handle),
                }, _span(self.path, node)))
            else:
                raise self.error(node, "unsupported file operation")
        return tuple(operations)

    def compile(self, source: str) -> BuildContract:
        try:
            tree = ast.parse(source, filename=str(self.path))
        except SyntaxError as error:
            raise ContractError(self.path, None, str(error)) from error
        function: ast.FunctionDef | None = None
        for node in tree.body:
            if isinstance(node, (ast.Import, ast.ImportFrom)):
                if not isinstance(node, ast.Import) or any(alias.name not in IMPORTS for alias in node.names):
                    raise self.error(node, "only imports of os and shutil are allowed")
                continue
            if isinstance(node, ast.FunctionDef) and node.name == "build" and function is None:
                function = node
                continue
            raise self.error(node, "module may contain only approved imports and build(ctx)")
        if function is None:
            raise ContractError(self.path, None, "missing build(ctx) function")
        args = function.args
        if (len(args.posonlyargs) + len(args.args) != 1 or args.vararg or args.kwarg
                or args.kwonlyargs or args.defaults or args.kw_defaults):
            raise self.error(function, "build must have exactly one positional ctx argument")
        return BuildContract(self.path.parent.name, str(self.path), self.statements(function.body))


def compile_contract(source: str, path: Path | str = "<memory>") -> BuildContract:
    """Compile source without importing it or touching its filesystem."""
    return _Compiler(Path(path)).compile(source)


def audit(root: Path, tests: Iterable[str] | None = None) -> tuple[list[BuildContract], list[str]]:
    test_root = root / "_aosp" / "art" / "test"
    paths = []
    requested = set(tests or ())
    for path in sorted(test_root.glob("*/build.py")):
        if not re.match(r"^\d", path.parent.name):
            continue
        if requested and path.parent.name not in requested:
            continue
        paths.append(path)
    contracts: list[BuildContract] = []
    errors: list[str] = []
    for path in paths:
        try:
            contracts.append(compile_contract(path.read_text(encoding="utf-8"), path))
        except (OSError, ContractError) as error:
            errors.append(str(error))
    return contracts, errors


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path,
                        default=Path(__file__).resolve().parent.parent,
                        help="aim repository root")
    parser.add_argument("--test", action="append", default=[],
                        help="audit one numeric ART test (repeatable)")
    parser.add_argument("--json", action="store_true", dest="as_json",
                        help="emit the typed contracts as JSON")
    parser.add_argument("--strict", action="store_true",
                        help="require every selected build.py to compile")
    args = parser.parse_args(argv)
    contracts, errors = audit(args.root.resolve(), args.test)
    if args.as_json:
        print(json.dumps({"contracts": [contract_json(c) for c in contracts],
                          "errors": errors}, indent=2, sort_keys=True))
    else:
        action_count = sum(len(contract.actions) for contract in contracts)
        print(f"BUILD-CONTRACT {'PASS' if not errors else 'FAIL'} "
              f"tests={len(contracts)} actions={action_count} unsupported={len(errors)}")
        for error in errors:
            print(error, file=sys.stderr)
    # Keep the historical fail-closed exit status by default.  ``--strict``
    # makes that policy explicit for CI callers and remains intentionally
    # equivalent: an unsupported build.py is never treated as a pass.
    return 0 if not errors else 1
