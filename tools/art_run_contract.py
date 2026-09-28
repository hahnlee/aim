#!/usr/bin/env python3
"""Compile an AOSP ART ``run.py`` into a safe, auditable Contract IR.

This module deliberately parses source without importing or executing it.  The
result is an intermediate representation of the context API calls and control
flow that a future runner can interpret.  Anything outside the small,
explicitly supported grammar is retained in the IR as ``unsupported`` and is
also reported; it is never silently guessed or dropped.
"""

from __future__ import annotations

import argparse
import ast
from copy import deepcopy
from dataclasses import dataclass
import json
import os
from pathlib import Path
import re
import shlex
import sys
from typing import Any, Iterable
import warnings


@dataclass(frozen=True)
class Span:
    line: int
    column: int
    end_line: int
    end_column: int

    @classmethod
    def from_node(cls, node: ast.AST) -> "Span":
        return cls(
            getattr(node, "lineno", 0),
            getattr(node, "col_offset", 0),
            getattr(node, "end_lineno", getattr(node, "lineno", 0)),
            getattr(node, "end_col_offset", getattr(node, "col_offset", 0)),
        )

    def as_dict(self) -> dict[str, int]:
        return {
            "line": self.line,
            "column": self.column,
            "end_line": self.end_line,
            "end_column": self.end_column,
        }


@dataclass(frozen=True)
class Issue:
    span: Span
    reason: str
    node: str

    def as_dict(self) -> dict[str, Any]:
        return {
            "span": self.span.as_dict(),
            "reason": self.reason,
            "node": self.node,
        }


@dataclass
class Contract:
    path: str
    function: str | None
    nodes: list[dict[str, Any]]
    issues: list[Issue]
    capabilities: set[str]

    @property
    def supported(self) -> bool:
        return not self.issues

    def as_dict(self) -> dict[str, Any]:
        return {
            "path": self.path,
            "function": self.function,
            "supported": self.supported,
            "capabilities": sorted(self.capabilities),
            "issues": [issue.as_dict() for issue in self.issues],
            "nodes": self.nodes,
        }


def _span(node: ast.AST) -> dict[str, Any]:
    return Span.from_node(node).as_dict()


def _node(kind: str, node: ast.AST, **values: Any) -> dict[str, Any]:
    return {"kind": kind, "span": _span(node), **values}


def _qualified_name(node: ast.AST) -> str | None:
    """Return a dotted name for simple Name/Attribute chains only."""
    if isinstance(node, ast.Name):
        return node.id
    if isinstance(node, ast.Attribute):
        base = _qualified_name(node.value)
        return f"{base}.{node.attr}" if base is not None else None
    return None


class Compiler:
    """Fail-closed AST compiler for the small run-test context contract."""

    # These are the only context methods that a Contract IR may mark as
    # executable.  The eventual interpreter can assign each method its own
    # capability/sandbox policy.
    _context_methods = frozenset({"default_run", "run", "bash", "echo", "export"})
    _inert_imports = frozenset({"os", "re", "resource", "sys"})

    def __init__(self, path: Path):
        self.path = str(path)
        self.issues: list[Issue] = []
        self.capabilities: set[str] = set()
        self._loop_depth = 0

    @staticmethod
    def _is_live_list(node: ast.AST) -> bool:
        return (
            isinstance(node, ast.Attribute)
            and isinstance(node.value, ast.Name)
            and node.value.id == "args"
            and node.attr == "runtime_option"
        )

    def issue(self, node: ast.AST, reason: str) -> dict[str, Any]:
        issue = Issue(Span.from_node(node), reason, type(node).__name__)
        self.issues.append(issue)
        self.capabilities.add("unsupported")
        return _node("unsupported", node, reason=reason)

    def expr(self, node: ast.AST) -> dict[str, Any]:
        if isinstance(node, ast.Constant):
            value = node.value
            if not isinstance(value, (str, int, float, bool, type(None))):
                return self.issue(node, "constant type is not in the safe literal set")
            return _node("literal", node, value=value)
        if isinstance(node, ast.Name):
            return _node("name", node, id=node.id)
        if isinstance(node, ast.Attribute):
            return _node("attribute", node, base=self.expr(node.value), attr=node.attr)
        if isinstance(node, ast.Subscript):
            self.capabilities.add("subscript")
            return _node("subscript", node, base=self.expr(node.value), index=self.expr(node.slice))
        if isinstance(node, (ast.List, ast.Tuple, ast.Set)):
            self.capabilities.add("literal_sequence")
            return _node(
                "sequence",
                node,
                sequence_type=type(node).__name__.lower(),
                items=[self.expr(item) for item in node.elts],
            )
        if isinstance(node, ast.Dict):
            self.capabilities.add("literal_mapping")
            items = []
            for key, value in zip(node.keys, node.values):
                value_ir = self.expr(value)
                if key is None:
                    # In the AST a None key denotes ``**value``.  Keep the
                    # value in the IR, but reject the expansion until the
                    # eventual interpreter defines its policy.
                    self.issue(value, "mapping ** expansion is not supported")
                    key_ir = None
                else:
                    key_ir = self.expr(key)
                items.append({"key": key_ir, "value": value_ir})
            return _node(
                "mapping",
                node,
                items=items,
            )
        if isinstance(node, ast.JoinedStr):
            self.capabilities.add("f_string")
            return _node(
                "f_string",
                node,
                parts=[
                    self.expr(part) if isinstance(part, ast.FormattedValue)
                    else _node("literal", part, value=part.value)
                    for part in node.values
                ],
            )
        if isinstance(node, ast.FormattedValue):
            return _node(
                "formatted_value",
                node,
                value=self.expr(node.value),
                conversion=node.conversion,
                format_spec=None if node.format_spec is None else self.expr(node.format_spec),
            )
        if isinstance(node, ast.Slice):
            return _node(
                "slice", node,
                lower=None if node.lower is None else self.expr(node.lower),
                upper=None if node.upper is None else self.expr(node.upper),
                step=None if node.step is None else self.expr(node.step),
            )
        if isinstance(node, ast.IfExp):
            self.capabilities.add("conditional_expression")
            return _node(
                "conditional_expression",
                node,
                condition=self.expr(node.test),
                when_true=self.expr(node.body),
                when_false=self.expr(node.orelse),
            )
        if isinstance(node, ast.UnaryOp):
            self.capabilities.add("expression_operator")
            return _node("unary", node, operator=type(node.op).__name__, operand=self.expr(node.operand))
        if isinstance(node, (ast.BinOp, ast.BoolOp, ast.Compare)):
            self.capabilities.add("expression_operator")
            values: dict[str, Any] = {}
            if isinstance(node, ast.BinOp):
                values = {"operator": type(node.op).__name__, "left": self.expr(node.left), "right": self.expr(node.right)}
            elif isinstance(node, ast.BoolOp):
                values = {"operator": type(node.op).__name__, "values": [self.expr(value) for value in node.values]}
            else:
                values = {
                    "left": self.expr(node.left),
                    "operators": [type(op).__name__ for op in node.ops],
                    "comparators": [self.expr(value) for value in node.comparators],
                }
            return _node("expression_operator", node, **values)
        if isinstance(node, ast.Call):
            qualified = _qualified_name(node.func)
            if qualified == "os.environ.get":
                if not (1 <= len(node.args) <= 2) or node.keywords:
                    return self.issue(node, "os.environ.get only accepts a literal key and optional literal default")
                key = node.args[0]
                if not isinstance(key, ast.Constant) or key.value != "ART_TEST_ON_VM":
                    return self.issue(node, "only the observed ART_TEST_ON_VM environment key is supported")
                default = None if len(node.args) == 1 else self.expr(node.args[1])
                if len(node.args) == 2 and not isinstance(node.args[1], ast.Constant):
                    return self.issue(node, "os.environ.get default must be a literal")
                self.capabilities.add("env_get")
                return _node(
                    "environment_get",
                    node,
                    key=_node("literal", key, value=key.value),
                    default=default,
                )
            if qualified == "ctx.expected_stdout.with_suffix":
                if len(node.args) != 1 or node.keywords or not isinstance(node.args[0], ast.Constant) or not isinstance(node.args[0].value, str):
                    return self.issue(node, "with_suffix requires one literal suffix on ctx.expected_stdout")
                suffix = node.args[0].value
                if suffix and not suffix.startswith("."):
                    return self.issue(node, "with_suffix suffix must be empty or start with a dot")
                self.capabilities.add("path_transform")
                return _node(
                    "path_ref_with_suffix",
                    node,
                    base=_node("path_ref", node.func.value, path="ctx.expected_stdout"),
                    suffix=_node("literal", node.args[0], value=suffix),
                )
            if qualified == "enumerate":
                if len(node.args) != 1 or node.keywords or not self._is_live_list(node.args[0]):
                    return self.issue(node, "only enumerate(args.runtime_option) is supported")
                self.capabilities.add("live_list_iteration")
                return _node(
                    "enumerate",
                    node,
                    iterable=self.expr(node.args[0]),
                    start=_node("literal", node, value=0),
                )
            if qualified == "args.runtime_option.pop":
                if len(node.args) != 1 or node.keywords or not self._is_live_list(node.func.value):
                    return self.issue(node, "only args.runtime_option.pop(index) is supported")
                self.capabilities.add("live_list_mutation")
                return _node(
                    "live_list_pop",
                    node,
                    list=self.expr(node.func.value),
                    index=self.expr(node.args[0]),
                )
            if qualified == "opt.startswith":
                if (_qualified_name(node.func.value) != "opt" or len(node.args) != 1 or node.keywords
                        or not isinstance(node.args[0], ast.Constant)
                        or node.args[0].value != "-Djava.library.path="):
                    return self.issue(node, "only opt.startswith('-Djava.library.path=') is supported")
                self.capabilities.add("typed_string_operation")
                return _node(
                    "string_startswith",
                    node,
                    value=self.expr(node.func.value),
                    prefix=_node("literal", node.args[0], value=node.args[0].value),
                )
            if qualified == "opt.split":
                if (_qualified_name(node.func.value) != "opt" or len(node.args) != 1 or node.keywords
                        or not isinstance(node.args[0], ast.Constant)
                        or node.args[0].value != ":"):
                    return self.issue(node, "only opt.split(':') is supported")
                self.capabilities.add("typed_string_operation")
                return _node(
                    "string_split",
                    node,
                    value=self.expr(node.func.value),
                    separator=_node("literal", node.args[0], value=node.args[0].value),
                )
            # Calls are executable effects unless they are one of the explicit
            # context APIs, which are handled at statement level.  Keeping the
            # call in IR still makes the rejected source auditable.
            return self.issue(node, f"expression call is not an allowed context API: {qualified or type(node.func).__name__}")
        if isinstance(node, ast.Starred):
            return self.issue(node, "starred expression is not supported")
        return self.issue(node, f"expression node is not supported: {type(node).__name__}")

    def _shell_template(self, node: ast.AST) -> tuple[str, dict[str, dict[str, Any]]] | None:
        """Lower a string expression to text plus typed interpolation slots.

        Only constants and f-strings are accepted.  Interpolations are never
        rendered into shell text: they remain expression IR slots so the
        eventual operation interpreter can validate paths at its boundary.
        """
        slots: dict[str, dict[str, Any]] = {}
        if isinstance(node, ast.Constant) and isinstance(node.value, str):
            return node.value, slots
        if not isinstance(node, ast.JoinedStr):
            self.issue(node, "shell command must be a literal string or f-string")
            return None
        pieces: list[str] = []
        for part in node.values:
            if isinstance(part, ast.Constant) and isinstance(part.value, str):
                pieces.append(part.value)
                continue
            if isinstance(part, ast.FormattedValue):
                slot = f"__ART_EXPR_{len(slots)}__"
                slots[slot] = self.expr(part.value)
                # A format spec changes shell bytes and is not needed by the
                # AOSP command corpus.  Reject it rather than normalizing it.
                if part.format_spec is not None:
                    self.issue(part.format_spec, "shell interpolation format specs are not supported")
                if part.conversion != -1:
                    self.issue(part, "shell interpolation conversions are not supported")
                pieces.append(slot)
                continue
            self.issue(part, f"f-string shell part is not supported: {type(part).__name__}")
        return "".join(pieces), slots

    @staticmethod
    def _shell_tokens(text: str) -> list[str] | None:
        if "\n" in text or "\r" in text:
            return None
        lexer = shlex.shlex(text, posix=True, punctuation_chars=True)
        lexer.whitespace_split = True
        lexer.commenters = ""
        try:
            return list(lexer)
        except ValueError:
            return None

    @staticmethod
    def _has_slot(value: str) -> bool:
        return "__ART_EXPR_" in value

    def _word(self, value: str, slots: dict[str, dict[str, Any]]) -> dict[str, Any]:
        """Represent a shell word without ever evaluating interpolation."""
        pieces: list[dict[str, Any]] = []
        pattern = re.compile(r"(__ART_EXPR_\d+__)")
        for part in pattern.split(value):
            if not part:
                continue
            if part in slots:
                pieces.append({"kind": "dynamic", "value": slots[part]})
            else:
                pieces.append({"kind": "literal", "value": part})
        if len(pieces) == 1 and pieces[0]["kind"] == "literal":
            return pieces[0]
        return {"kind": "word", "parts": pieces}

    def _path(self, value: str, slots: dict[str, dict[str, Any]]) -> dict[str, Any]:
        """Attach the evaluation-boundary path policy to every file operand."""
        return {
            "kind": "path",
            "value": self._word(value, slots),
            "policy": "sandbox-relative-no-escape",
        }

    def _static(self, value: str, slots: dict[str, dict[str, Any]]) -> str | None:
        return value if not self._has_slot(value) else None

    @staticmethod
    def _split_unescaped(value: str, delimiter: str, start: int = 0) -> tuple[str, int] | None:
        escaped = False
        for index in range(start, len(value)):
            char = value[index]
            if escaped:
                escaped = False
            elif char == "\\":
                escaped = True
            elif char == delimiter:
                return value[start:index], index + 1
        return None

    def _sed_script(self, script: str, slots: dict[str, dict[str, Any]]) -> dict[str, Any] | None:
        """Parse the small, typed sed grammar used by ART run.py filters."""
        if script.startswith("/"):
            parsed = self._split_unescaped(script, "/", 1)
            if parsed is None:
                return None
            pattern, end = parsed
            suffix = script[end:]
            if suffix == "d":
                return {"kind": "delete_matching", "pattern": self._word(pattern, slots)}
            if suffix == "!d":
                return {"kind": "keep_matching", "pattern": self._word(pattern, slots)}
            match = re.fullmatch(r",\+(\d+)d", suffix)
            if match:
                return {
                    "kind": "delete_matching_with_following_lines",
                    "pattern": self._word(pattern, slots),
                    "following_lines": int(match.group(1)),
                }
            return None
        if not script.startswith("s") or len(script) < 3:
            return None
        delimiter = script[1]
        parsed_pattern = self._split_unescaped(script, delimiter, 2)
        if parsed_pattern is None:
            return None
        pattern, after_pattern = parsed_pattern
        parsed_replacement = self._split_unescaped(script, delimiter, after_pattern)
        if parsed_replacement is None:
            return None
        replacement, after_replacement = parsed_replacement
        flags = script[after_replacement:]
        if any(flag not in "gip" for flag in flags):
            return None
        return {
            "kind": "replace",
            "pattern": self._word(pattern, slots),
            "replacement": self._word(replacement, slots),
            "flags": flags,
        }

    def _parse_sed(self, tokens: list[str], slots: dict[str, dict[str, Any]]) -> dict[str, Any] | None:
        index = 1
        in_place = False
        extended = False
        quiet = False
        scripts: list[dict[str, Any]] = []
        bare_script_seen = False
        while index < len(tokens):
            token = tokens[index]
            if token == "-e":
                index += 1
                if index >= len(tokens):
                    return None
                script = tokens[index]
                parsed = self._sed_script(script, slots)
                if parsed is None:
                    return None
                scripts.append(parsed)
                bare_script_seen = True
                index += 1
                continue
            if token in {"-i", "-E", "-r", "-n"}:
                in_place |= token == "-i"
                extended |= token in {"-E", "-r"}
                quiet |= token == "-n"
                index += 1
                continue
            if token == "-rnie":
                in_place = True
                extended = True
                quiet = True
                index += 1
                if index >= len(tokens):
                    return None
                parsed = self._sed_script(tokens[index], slots)
                if parsed is None:
                    return None
                scripts.append(parsed)
                index += 1
                bare_script_seen = True
                break
            if token.startswith("-"):
                return None
            if not bare_script_seen:
                parsed = self._sed_script(token, slots)
                if parsed is None:
                    return None
                scripts.append(parsed)
                bare_script_seen = True
                index += 1
                break
            break
        files = tokens[index:]
        if not in_place or not scripts or len(files) != 1:
            return None
        return {
            "kind": "sed_filter",
            "in_place": True,
            "extended_regex": extended,
            "print_only_matches": quiet,
            "scripts": scripts,
            "file": self._path(files[0], slots),
        }

    def _parse_line_count(self, tokens: list[str], index: int, slots: dict[str, dict[str, Any]]) -> tuple[dict[str, Any], int] | None:
        # shlex's punctuation mode gives the exact token sequence for
        # ``$(wc -l < path)``.  No other command substitution is accepted.
        expected = ["$", "(", "wc", "-l", "<"]
        if tokens[index:index + len(expected)] != expected:
            return None
        index += len(expected)
        if index >= len(tokens):
            return None
        source = tokens[index]
        index += 1
        if index >= len(tokens) or tokens[index] != ")":
            return None
        return {"kind": "line_count", "file": self._path(source, slots)}, index + 1

    def _parse_shell_segment(self, tokens: list[str], slots: dict[str, dict[str, Any]]) -> dict[str, Any] | None:
        if not tokens:
            return None
        command = self._static(tokens[0], slots)
        if command == ">":
            if len(tokens) != 2:
                return None
            return {"kind": "truncate", "file": self._path(tokens[1], slots)}
        if command == "sed":
            return self._parse_sed(tokens, slots)
        if command in {"cat", "touch", "mv", "ln", "grep", "head", "tail", "truncate"}:
            pass
        elif command is None or command == "":
            return None
        else:
            return None
        if command == "cat":
            if len(tokens) != 2:
                return None
            return {"kind": "print", "file": self._path(tokens[1], slots)}
        if command == "touch":
            if len(tokens) < 2 or any(self._has_slot(token) and not token for token in tokens[1:]):
                return None
            return {"kind": "touch", "files": [self._path(token, slots) for token in tokens[1:]]}
        if command == "mv":
            if len(tokens) != 3:
                return None
            return {"kind": "move", "source": self._path(tokens[1], slots), "destination": self._path(tokens[2], slots)}
        if command == "ln":
            if len(tokens) != 4 or tokens[1] != "-sf":
                return None
            return {"kind": "symlink", "source": self._path(tokens[2], slots), "destination": self._path(tokens[3], slots)}
        if command == "grep":
            if len(tokens) != 7 or tokens[1:3] != ["-v", "-f"] or tokens[5] != ">":
                return None
            return {
                "kind": "grep_filter",
                "exclude_patterns": self._path(tokens[3], slots),
                "file": self._path(tokens[4], slots),
                "output": self._path(tokens[6], slots),
            }
        if command in {"head", "tail"}:
            if len(tokens) < 5 or tokens[1] != "-n":
                return None
            index = 2
            if command == "head" and tokens[index] == "$":
                count_result = self._parse_line_count(tokens, index, slots)
                if count_result is None:
                    return None
                count, index = count_result
            else:
                count_text = self._static(tokens[index], slots)
                if count_text is None or not re.fullmatch(r"[0-9]+", count_text):
                    return None
                count = {"kind": "literal", "value": int(count_text)}
                index += 1
            if len(tokens) != index + 3 or tokens[index + 1] != ">":
                return None
            return {
                "kind": command,
                "count": count,
                "file": self._path(tokens[index], slots),
                "output": self._path(tokens[index + 2], slots),
            }
        if command == "truncate":
            if len(tokens) != 4 or tokens[1:3] != ["-s", "0"]:
                return None
            return {"kind": "truncate", "file": self._path(tokens[3], slots)}
        return None

    def _shell_command(self, call: ast.Call) -> dict[str, Any] | None:
        if len(call.args) != 1:
            self.issue(call, "ctx.run/ctx.bash require exactly one command argument")
            return None
        template = self._shell_template(call.args[0])
        if template is None:
            return None
        text, slots = template
        tokens = self._shell_tokens(text)
        if tokens is None:
            self.issue(call, "shell command has invalid quoting or newlines")
            return None
        segments: list[list[str]] = [[]]
        for token in tokens:
            if token == "&&":
                if not segments[-1]:
                    self.issue(call, "empty command around &&")
                    return None
                segments.append([])
                continue
            if token in {";", "||", "|", "&"}:
                self.issue(call, f"shell operator is not modeled: {token}")
                return None
            segments[-1].append(token)
        if not segments[-1]:
            self.issue(call, "empty command after &&")
            return None
        operations = [self._parse_shell_segment(segment, slots) for segment in segments]
        if any(operation is None for operation in operations):
            self.issue(call, "shell command is outside the typed Contract IR grammar")
            return None
        for operation in operations:
            assert operation is not None
            self.capabilities.add(f"shell.{operation['kind']}")
        return {"kind": "shell_sequence", "operations": operations}

    def _context_call(self, node: ast.Call) -> dict[str, Any] | None:
        qualified = _qualified_name(node.func)
        if qualified is None or not qualified.startswith("ctx."):
            return None
        method = qualified[4:]
        if method not in self._context_methods:
            return self.issue(node, f"context API is not allowlisted: {qualified}")
        self.capabilities.add(f"ctx.{method}")
        if any(keyword.arg is None for keyword in node.keywords):
            return self.issue(node, f"{qualified} does not allow **kwargs in Contract IR")
        context_node = _node(
            "context_call",
            node,
            api=qualified,
            args=[self.expr(arg) for arg in node.args],
            kwargs=[
                {"name": keyword.arg, "value": self.expr(keyword.value)}
                for keyword in node.keywords
            ],
        )
        if method in {"run", "bash"}:
            check_values = [keyword.value for keyword in node.keywords if keyword.arg == "check"]
            unknown = [keyword.arg for keyword in node.keywords if keyword.arg != "check"]
            if unknown:
                self.issue(node, f"{qualified} has unsupported shell keyword(s): {','.join(unknown)}")
            if check_values:
                if len(check_values) != 1 or not isinstance(check_values[0], ast.Constant) or not isinstance(check_values[0].value, bool):
                    self.issue(node, f"{qualified}.check must be a boolean literal")
                context_node["check"] = check_values[0].value if len(check_values) == 1 and isinstance(check_values[0], ast.Constant) and isinstance(check_values[0].value, bool) else None
            else:
                context_node["check"] = True
            context_node["command"] = self._shell_command(node)
        return context_node

    def stmt(self, node: ast.stmt) -> dict[str, Any]:
        if isinstance(node, ast.Expr):
            if isinstance(node.value, ast.Call):
                call = self._context_call(node.value)
                if call is not None:
                    return call
            return _node("expression_statement", node, expression=self.expr(node.value))
        if isinstance(node, ast.Assign):
            self.capabilities.add("assignment")
            for target in node.targets:
                if isinstance(target, ast.Subscript) and not self._is_live_list(target.value):
                    self.issue(target, "only args.runtime_option indexed assignment is supported")
            return _node("assignment", node, targets=[self.expr(target) for target in node.targets], value=self.expr(node.value))
        if isinstance(node, ast.AnnAssign):
            self.capabilities.add("assignment")
            return _node("annotated_assignment", node, target=self.expr(node.target), annotation=self.expr(node.annotation), value=None if node.value is None else self.expr(node.value))
        if isinstance(node, ast.AugAssign):
            self.capabilities.add("argument_mutation" if _qualified_name(node.target) and _qualified_name(node.target).startswith("args.") else "assignment")
            return _node("augmented_assignment", node, target=self.expr(node.target), operator=type(node.op).__name__, value=self.expr(node.value))
        if isinstance(node, ast.Delete):
            return self.issue(node, "delete statement is not supported")
        if isinstance(node, ast.If):
            self.capabilities.add("branch")
            return _node("branch", node, condition=self.expr(node.test), body=self.block(node.body), otherwise=self.block(node.orelse))
        if isinstance(node, (ast.For, ast.AsyncFor)):
            self.capabilities.add("loop")
            if isinstance(node, ast.AsyncFor):
                self.issue(node, "async for is not supported")
            return _node(
                "loop",
                node,
                target=self.expr(node.target),
                iterable=self.expr(node.iter),
                body=self.block(node.body, in_loop=True),
                otherwise=self.block(node.orelse),
            )
        if isinstance(node, ast.While):
            self.capabilities.add("loop")
            return _node(
                "while",
                node,
                condition=self.expr(node.test),
                body=self.block(node.body, in_loop=True),
                otherwise=self.block(node.orelse),
            )
        if isinstance(node, ast.Return):
            self.capabilities.add("return")
            return _node("return", node, value=None if node.value is None else self.expr(node.value))
        if isinstance(node, ast.Pass):
            return _node("pass", node)
        if isinstance(node, ast.Break):
            if self._loop_depth == 0:
                return self.issue(node, "break outside a loop is not supported")
            self.capabilities.add("loop_control")
            return _node("break", node)
        if isinstance(node, ast.Assert):
            self.capabilities.add("assertion")
            return _node("assert", node, condition=self.expr(node.test), message=None if node.msg is None else self.expr(node.msg))
        if isinstance(node, ast.Raise):
            return self.issue(node, "raise statement is not supported")
        if isinstance(node, ast.Try):
            return self.issue(node, "try/except control flow is not supported")
        if isinstance(node, (ast.With, ast.AsyncWith)):
            return self.issue(node, "with control flow is not supported")
        if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef)):
            return self.issue(node, f"nested definition is not supported: {getattr(node, 'name', '')}")
        return self.issue(node, f"statement node is not supported: {type(node).__name__}")

    def block(self, nodes: Iterable[ast.stmt], in_loop: bool = False) -> list[dict[str, Any]]:
        previous_depth = self._loop_depth
        if in_loop:
            self._loop_depth += 1
        try:
            return [self.stmt(node) for node in nodes]
        finally:
            self._loop_depth = previous_depth

    def compile(self, source: str) -> Contract:
        try:
            # A few upstream tests intentionally contain old-style escaped
            # strings.  Their SyntaxWarning is source data, not a compiler
            # failure, and should not pollute machine-readable audit output.
            with warnings.catch_warnings():
                warnings.simplefilter("ignore", SyntaxWarning)
                tree = ast.parse(source, filename=self.path)
        except SyntaxError as error:
            fake = ast.Constant(value=None)
            fake.lineno = error.lineno or 0  # type: ignore[attr-defined]
            fake.col_offset = error.offset or 0  # type: ignore[attr-defined]
            self.issue(fake, f"syntax error: {error.msg}")
            return Contract(self.path, None, [], self.issues, self.capabilities)

        run_functions = [
            node for node in tree.body
            if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef)) and node.name == "run"
        ]
        if len(run_functions) != 1:
            self.issue(tree, "run.py must contain exactly one top-level run(ctx, args) function")
            function = None
            nodes = []
        else:
            function_node = run_functions[0]
            function = function_node.name
            self.capabilities.add("run_function")
            if isinstance(function_node, ast.AsyncFunctionDef):
                self.issue(function_node, "async run function is not supported")
            if function_node.decorator_list:
                for decorator in function_node.decorator_list:
                    self.issue(decorator, "run function decorators are not executed by the safe compiler")
            if function_node.args.defaults or function_node.args.kw_defaults:
                self.issue(function_node, "run function default arguments are not part of the ctx,args contract")
            if function_node.args.vararg or function_node.args.kwarg or function_node.args.kwonlyargs:
                self.issue(function_node, "run function variadic arguments are not part of the ctx,args contract")
            if len(function_node.args.args) < 2:
                self.issue(function_node, "run function must expose ctx and args parameters")
            nodes = self.block(function_node.body)

        # Imports and top-level effects are intentionally visible to the audit,
        # even though the runner never executes them.  A future Contract IR
        # interpreter must decide whether each one can be sandboxed.
        for top_level in tree.body:
            if isinstance(top_level, (ast.Import, ast.ImportFrom)):
                aliases = top_level.names
                allowed = (
                    isinstance(top_level, ast.Import)
                    and all(alias.name in self._inert_imports and alias.asname is None for alias in aliases)
                )
                if allowed:
                    self.capabilities.add("inert_import")
                else:
                    self.capabilities.add("module_import")
                    self.issue(top_level, "top-level import is not in the inert allowlist")
            elif isinstance(top_level, ast.Expr) and isinstance(top_level.value, ast.Constant) and isinstance(top_level.value.value, str):
                # Module docstrings are metadata, not an effect.
                continue
            elif isinstance(top_level, (ast.FunctionDef, ast.AsyncFunctionDef)) and top_level.name == "run":
                continue
            elif not isinstance(top_level, (ast.Import, ast.ImportFrom)):
                self.issue(top_level, "top-level executable code is outside the run contract")

        return Contract(self.path, function, nodes, self.issues, self.capabilities)


def compile_source(source: str, path: str = "<memory>") -> Contract:
    return Compiler(Path(path)).compile(source)


def compile_path(path: Path) -> Contract:
    try:
        source = path.read_text(encoding="utf-8")
    except (OSError, UnicodeError) as error:
        contract = Contract(str(path), None, [], [], set())
        compiler = Compiler(path)
        compiler.issue(ast.Constant(value=None), f"cannot read source: {error}")
        return Contract(str(path), None, [], compiler.issues, compiler.capabilities)
    return Compiler(path).compile(source)


@dataclass(frozen=True)
class PlanIssue:
    span: dict[str, int]
    reason: str

    def as_dict(self) -> dict[str, Any]:
        return {"span": self.span, "reason": self.reason}


@dataclass
class ActionPlan:
    path: str
    variant: str
    actions: list[dict[str, Any]]
    issues: list[PlanIssue]
    # ``run.py`` may select its golden output after the final default_run.
    # Keep that post-statement value alongside the ordered actions so callers
    # do not have to infer it from an earlier invocation snapshot.
    final_expected_stdout: str | None = None

    @property
    def supported(self) -> bool:
        return not self.issues

    def as_dict(self) -> dict[str, Any]:
        return {
            "path": self.path,
            "variant": self.variant,
            "supported": self.supported,
            "actions": self.actions,
            "issues": [issue.as_dict() for issue in self.issues],
            "final_expected_stdout": self.final_expected_stdout,
        }


class _Unknown:
    pass


@dataclass
class _LiveEnumerator:
    values: list[Any]
    start: int


class Evaluator:
    """Deterministically interpret ContractIR into data-only actions.

    This is deliberately not a Python evaluator: it has no globals, imports,
    attribute fallback, filesystem access, process execution, or shell.  Any
    value or action outside the typed IR is an issue with the original IR
    span, and evaluation stops before emitting a guessed action.
    """

    def __init__(self, contract: Contract, context: dict[str, Any], variant: str = "custom"):
        self.contract = contract
        self.variant = variant
        self.context = context
        self.locals: dict[str, Any] = {
            "args": deepcopy(context.get("args", {})),
            "ctx": {
                "env": deepcopy(context.get("env", {})),
                "expected_stdout": context.get("expected_stdout"),
            },
        }
        self.actions: list[dict[str, Any]] = []
        self.issues: list[PlanIssue] = []
        self.aborted = False

    def issue(self, node: dict[str, Any] | None, reason: str) -> _Unknown:
        span = node.get("span", {}) if node is not None else {}
        self.issues.append(PlanIssue(span, reason))
        self.aborted = True
        return _Unknown()

    @staticmethod
    def _is_unknown(value: Any) -> bool:
        return isinstance(value, _Unknown)

    def _eval_word(self, node: dict[str, Any]) -> str | _Unknown:
        kind = node.get("kind")
        if kind == "literal":
            if isinstance(node.get("value"), str):
                return node["value"]
            return self.issue(node, "shell word literal is not a string")
        if kind == "dynamic":
            value = self.expr(node.get("value", {}))
            if self._is_unknown(value):
                return value
            if not isinstance(value, str):
                return self.issue(node, "dynamic shell word must evaluate to a string")
            return value
        if kind == "word":
            pieces: list[str] = []
            for part in node.get("parts", []):
                value = self._eval_word(part)
                if self._is_unknown(value):
                    return value
                pieces.append(value)
            return "".join(pieces)
        return self.issue(node, f"unknown shell word IR kind: {kind}")

    def _path(self, node: dict[str, Any]) -> str | _Unknown:
        if node.get("kind") != "path" or node.get("policy") != "sandbox-relative-no-escape":
            return self.issue(node, "path operand lacks the sandbox evaluation policy")
        value = self._eval_word(node.get("value", {}))
        if self._is_unknown(value):
            return value
        root_value = self.context.get("path_root")
        if not isinstance(root_value, str) or not root_value:
            return self.issue(node, "path_root is required to validate a path operand")
        root = os.path.normpath(root_value)
        candidate = os.path.normpath(value)
        resolved = candidate if os.path.isabs(candidate) else os.path.normpath(os.path.join(root, candidate))
        try:
            if os.path.commonpath([root, resolved]) != root:
                return self.issue(node, "path escapes the evaluation root")
        except ValueError:
            return self.issue(node, "path is on an incompatible filesystem root")
        return resolved

    def _assign(self, target: dict[str, Any], value: Any) -> bool:
        kind = target.get("kind")
        if kind == "name":
            self.locals[target.get("id", "")] = value
            return True
        if kind == "attribute":
            base = self.expr(target.get("base", {}))
            if self._is_unknown(base) or not isinstance(base, dict):
                self.issue(target, "attribute assignment target is not a typed mapping")
                return False
            # ``ctx.env`` models a process environment.  Python's
            # ``os.environ`` coerces assigned scalar values to strings; keep
            # the same contract so ``ctx.env.FOO = 2`` produces a valid
            # string-valued environment snapshot for ``default_run``.
            if base is self.locals.get("ctx", {}).get("env"):
                value = str(value)
            base[target.get("attr")] = value
            return True
        if kind == "subscript":
            base = self.expr(target.get("base", {}))
            index = self.expr(target.get("index", {}))
            if self._is_unknown(base) or self._is_unknown(index):
                return False
            if base is self.locals.get("ctx", {}).get("env"):
                value = str(value)
            try:
                base[index] = value
            except (IndexError, KeyError, TypeError):
                self.issue(target, "subscript assignment target is invalid")
                return False
            return True
        self.issue(target, f"unsupported assignment target: {kind}")
        return False

    def _snapshot(self, value: Any) -> Any:
        return deepcopy(value)

    def expr(self, node: dict[str, Any]) -> Any:
        if self.aborted:
            return _Unknown()
        kind = node.get("kind")
        if kind == "literal":
            return node.get("value")
        if kind == "name":
            name = node.get("id")
            if name not in self.locals:
                return self.issue(node, f"unknown local value: {name}")
            return self.locals[name]
        if kind == "attribute":
            base = self.expr(node.get("base", {}))
            if self._is_unknown(base):
                return base
            if not isinstance(base, dict) or node.get("attr") not in base:
                return self.issue(node, f"unknown attribute value: {node.get('attr')}")
            return base[node.get("attr")]
        if kind == "path_ref":
            ctx = self.locals.get("ctx", {})
            value = ctx.get("expected_stdout") if node.get("path") == "ctx.expected_stdout" else None
            if not isinstance(value, str):
                return self.issue(node, "ctx.expected_stdout is not a concrete path")
            return value
        if kind == "path_ref_with_suffix":
            base = self.expr(node.get("base", {}))
            suffix = self.expr(node.get("suffix", {}))
            if self._is_unknown(base) or self._is_unknown(suffix):
                return _Unknown()
            if not isinstance(base, str) or not isinstance(suffix, str):
                return self.issue(node, "path suffix operands are not strings")
            directory, filename = os.path.split(base)
            stem, _ = os.path.splitext(filename)
            return os.path.join(directory, stem + suffix) if suffix else os.path.join(directory, stem)
        if kind == "subscript":
            base = self.expr(node.get("base", {}))
            index = self.expr(node.get("index", {}))
            if self._is_unknown(base) or self._is_unknown(index):
                return _Unknown()
            try:
                return base[index]
            except (IndexError, KeyError, TypeError):
                return self.issue(node, "subscript evaluation failed")
        if kind == "sequence":
            values = [self.expr(item) for item in node.get("items", [])]
            if any(self._is_unknown(value) for value in values):
                return _Unknown()
            return tuple(values) if node.get("sequence_type") == "tuple" else list(values)
        if kind == "mapping":
            result: dict[Any, Any] = {}
            for item in node.get("items", []):
                if item.get("key") is None:
                    return self.issue(node, "mapping expansion cannot be evaluated")
                key = self.expr(item["key"])
                value = self.expr(item["value"])
                if self._is_unknown(key) or self._is_unknown(value):
                    return _Unknown()
                result[key] = value
            return result
        if kind == "f_string":
            result: list[str] = []
            for part in node.get("parts", []):
                value = self.expr(part)
                if self._is_unknown(value):
                    return value
                result.append(str(value))
            return "".join(result)
        if kind == "formatted_value":
            value = self.expr(node.get("value", {}))
            if self._is_unknown(value):
                return value
            format_spec = node.get("format_spec")
            if format_spec is None:
                return str(value)
            spec = self.expr(format_spec)
            if self._is_unknown(spec):
                return spec
            try:
                return format(value, spec)
            except (ValueError, TypeError):
                return self.issue(node, "f-string format evaluation failed")
        if kind == "environment_get":
            key = node.get("key", {}).get("value")
            env = self.locals.get("ctx", {}).get("env", {})
            if not isinstance(env, dict):
                return self.issue(node, "environment snapshot is not a mapping")
            if key in env:
                return env[key]
            default = node.get("default")
            return None if default is None else self.expr(default)
        if kind == "conditional_expression":
            condition = self.expr(node.get("condition", {}))
            if self._is_unknown(condition):
                return condition
            return self.expr(node.get("when_true" if bool(condition) else "when_false", {}))
        if kind == "unary":
            operand = self.expr(node.get("operand", {}))
            if self._is_unknown(operand):
                return operand
            try:
                return {"Not": lambda: not operand, "USub": lambda: -operand, "UAdd": lambda: +operand}.get(node.get("operator"), lambda: self.issue(node, "unary operator is not supported"))()
            except (TypeError, ValueError):
                return self.issue(node, "unary operation failed")
        if kind == "expression_operator":
            return self._operator(node)
        if kind == "enumerate":
            values = self.expr(node.get("iterable", {}))
            start = self.expr(node.get("start", {}))
            if self._is_unknown(values) or self._is_unknown(start):
                return _Unknown()
            if not isinstance(values, list) or not isinstance(start, int):
                return self.issue(node, "enumerate requires a concrete mutable list and integer start")
            return _LiveEnumerator(values, start)
        if kind == "live_list_pop":
            values = self.expr(node.get("list", {}))
            index = self.expr(node.get("index", {}))
            if self._is_unknown(values) or self._is_unknown(index):
                return _Unknown()
            if not isinstance(values, list) or not isinstance(index, int):
                return self.issue(node, "live list pop requires a list and integer index")
            try:
                return values.pop(index)
            except IndexError:
                return self.issue(node, "live list pop index is out of range")
        if kind == "string_startswith":
            value = self.expr(node.get("value", {}))
            prefix = self.expr(node.get("prefix", {}))
            if self._is_unknown(value) or self._is_unknown(prefix):
                return _Unknown()
            return value.startswith(prefix) if isinstance(value, str) and isinstance(prefix, str) else self.issue(node, "startswith operands are not strings")
        if kind == "string_split":
            value = self.expr(node.get("value", {}))
            separator = self.expr(node.get("separator", {}))
            if self._is_unknown(value) or self._is_unknown(separator):
                return _Unknown()
            return value.split(separator) if isinstance(value, str) and isinstance(separator, str) else self.issue(node, "split operands are not strings")
        return self.issue(node, f"opaque or unknown expression IR kind: {kind}")

    def _operator(self, node: dict[str, Any]) -> Any:
        operator = node.get("operator")
        if operator in {"And", "Or"}:
            values = node.get("values", [])
            result: Any = True if operator == "And" else False
            for child in values:
                result = self.expr(child)
                if self._is_unknown(result):
                    return result
                if operator == "And" and not bool(result):
                    return result
                if operator == "Or" and bool(result):
                    return result
            return result
        if "left" in node and "right" in node:
            left = self.expr(node["left"])
            right = self.expr(node["right"])
            if self._is_unknown(left) or self._is_unknown(right):
                return _Unknown()
            try:
                operations = {
                    "Add": lambda: left + right, "Sub": lambda: left - right,
                    "Mult": lambda: left * right, "Div": lambda: left / right,
                    "FloorDiv": lambda: left // right, "Mod": lambda: left % right,
                }
                if operator not in operations:
                    return self.issue(node, f"binary operator is not supported: {operator}")
                return operations[operator]()
            except (TypeError, ValueError, ZeroDivisionError):
                return self.issue(node, "binary operation failed")
        left = self.expr(node.get("left", {}))
        comparators = [self.expr(value) for value in node.get("comparators", [])]
        if self._is_unknown(left) or any(self._is_unknown(value) for value in comparators):
            return _Unknown()
        current = left
        for operator, value in zip(node.get("operators", []), comparators):
            try:
                if operator == "Eq":
                    result = current == value
                elif operator == "NotEq":
                    result = current != value
                elif operator == "Lt":
                    result = current < value
                elif operator == "LtE":
                    result = current <= value
                elif operator == "Gt":
                    result = current > value
                elif operator == "GtE":
                    result = current >= value
                elif operator == "In":
                    result = current in value
                elif operator == "NotIn":
                    result = current not in value
                elif operator == "Is":
                    result = current is value
                elif operator == "IsNot":
                    result = current is not value
                else:
                    return self.issue(node, f"comparison operator is not supported: {operator}")
            except (KeyError, TypeError):
                return self.issue(node, f"comparison operator is not supported: {operator}")
            if not result:
                return False
            current = value
        return True

    def _eval_shell_operation(self, operation: dict[str, Any]) -> dict[str, Any] | _Unknown:
        result: dict[str, Any] = {"kind": operation.get("kind")}
        for key, value in operation.items():
            if key in {"kind", "policy"}:
                continue
            if key in {"file", "output", "source", "destination", "exclude_patterns"} and isinstance(value, dict) and value.get("kind") == "path":
                evaluated = self._path(value)
            elif key == "files":
                evaluated = [self._path(item) for item in value]
            elif key in {"pattern", "replacement"}:
                evaluated = self._eval_word(value)
            elif key == "scripts":
                evaluated = []
                for script in value:
                    script_result = {"kind": script.get("kind")}
                    for script_key, script_value in script.items():
                        if script_key == "kind":
                            continue
                        if isinstance(script_value, dict):
                            script_result[script_key] = self._eval_word(script_value)
                        else:
                            script_result[script_key] = script_value
                    evaluated.append(script_result)
            elif key == "count" and isinstance(value, dict):
                if value.get("kind") == "line_count":
                    evaluated_file = self._path(value.get("file", {}))
                    evaluated = {"kind": "line_count", "file": evaluated_file}
                elif (value.get("kind") == "literal" and
                      isinstance(value.get("value"), int) and
                      value["value"] >= 0):
                    evaluated = value["value"]
                else:
                    return self.issue(operation, "shell line count is not a nonnegative integer")
            else:
                evaluated = value
            if self._is_unknown(evaluated) or (isinstance(evaluated, list) and any(self._is_unknown(item) for item in evaluated)):
                return _Unknown()
            result[key] = evaluated
        return result

    def _context_call(self, node: dict[str, Any]) -> bool:
        api = node.get("api")
        values = [self.expr(value) for value in node.get("args", [])]
        if any(self._is_unknown(value) for value in values):
            return False
        kwargs: list[dict[str, Any]] = []
        for item in node.get("kwargs", []):
            value = self.expr(item.get("value", {}))
            if self._is_unknown(value):
                return False
            kwargs.append({"name": item.get("name"), "value": self._snapshot(value)})
        if api == "ctx.default_run":
            self.actions.append({
                "kind": "default_run",
                "span": node.get("span", {}),
                "args": self._snapshot(values),
                "kwargs": kwargs,
                "args_snapshot": self._snapshot(self.locals.get("args", {})),
                "env_snapshot": self._snapshot(self.locals.get("ctx", {}).get("env", {})),
                "expected_stdout_snapshot": self._snapshot(self.locals.get("ctx", {}).get("expected_stdout")),
            })
            return True
        if api in {"ctx.run", "ctx.bash"}:
            command = node.get("command")
            if not isinstance(command, dict):
                self.issue(node, "shell context call has no typed command")
                return False
            operations = [self._eval_shell_operation(operation) for operation in command.get("operations", [])]
            if any(self._is_unknown(operation) for operation in operations):
                return False
            self.actions.append({
                "kind": "shell",
                "api": api,
                "span": node.get("span", {}),
                "check": node.get("check", True),
                "operations": operations,
            })
            return True
        if api == "ctx.echo":
            self.actions.append({"kind": "echo", "span": node.get("span", {}), "args": self._snapshot(values)})
            return True
        if api == "ctx.export":
            if len(values) != 2 or not all(isinstance(value, str) for value in values):
                self.issue(node, "ctx.export requires name and string value")
                return False
            self.locals["ctx"]["env"][values[0]] = values[1]
            self.actions.append({"kind": "export", "span": node.get("span", {}), "name": values[0], "value": values[1]})
            return True
        self.issue(node, f"unknown context action: {api}")
        return False

    def _block(self, nodes: list[dict[str, Any]]) -> str | None:
        for node in nodes:
            signal = self.stmt(node)
            if signal is not None or self.aborted:
                return signal
        return None

    def stmt(self, node: dict[str, Any]) -> str | None:
        if self.aborted:
            return None
        kind = node.get("kind")
        if kind == "context_call":
            self._context_call(node)
            return None
        if kind in {"assignment", "annotated_assignment", "augmented_assignment"}:
            if kind == "augmented_assignment":
                current = self.expr(node.get("target", {}))
                value = self.expr(node.get("value", {}))
                if self._is_unknown(current) or self._is_unknown(value):
                    return None
                result = self._operator({"kind": "expression_operator", "operator": node.get("operator"), "left": {"kind": "literal", "value": current}, "right": {"kind": "literal", "value": value}})
                if self._is_unknown(result):
                    return None
                self._assign(node.get("target", {}), result)
                return None
            value = self.expr(node.get("value", {})) if node.get("value") is not None else None
            if self._is_unknown(value):
                return None
            if kind == "annotated_assignment":
                target = node.get("target", {})
            else:
                for target in node.get("targets", []):
                    if not self._assign(target, self._snapshot(value)):
                        return None
                return None
            self._assign(target, value)
            return None
        if kind == "expression_statement":
            self.expr(node.get("expression", {}))
            return None
        if kind == "branch":
            condition = self.expr(node.get("condition", {}))
            if self._is_unknown(condition):
                return None
            return self._block(node.get("body", []) if bool(condition) else node.get("otherwise", []))
        if kind == "loop":
            iterable = self.expr(node.get("iterable", {}))
            if self._is_unknown(iterable):
                return None
            if isinstance(iterable, _LiveEnumerator):
                index = iterable.start
                while index < len(iterable.values):
                    self._assign_loop_target(node.get("target", {}), (index, iterable.values[index]), node)
                    signal = self._block(node.get("body", []))
                    if self.aborted:
                        return None
                    if isinstance(signal, str) and signal == "break":
                        return None
                    index += 1
                return self._block(node.get("otherwise", []))
            if not isinstance(iterable, (list, tuple, str)):
                self.issue(node, "loop iterable is not concrete")
                return None
            for item in list(iterable):
                self._assign_loop_target(node.get("target", {}), item, node)
                signal = self._block(node.get("body", []))
                if self.aborted:
                    return None
                if signal == "break":
                    return None
            return self._block(node.get("otherwise", []))
        if kind == "while":
            for _ in range(10000):
                condition = self.expr(node.get("condition", {}))
                if self._is_unknown(condition):
                    return None
                if not bool(condition):
                    return self._block(node.get("otherwise", []))
                signal = self._block(node.get("body", []))
                if self.aborted:
                    return None
                if signal == "break":
                    return None
            self.issue(node, "while loop exceeded deterministic iteration limit")
            return None
        if kind == "break":
            return "break"
        if kind == "return":
            if node.get("value") is not None:
                self.expr(node["value"])
            return "return"
        if kind == "assert":
            condition = self.expr(node.get("condition", {}))
            if self._is_unknown(condition):
                return None
            if not bool(condition):
                self.issue(node, "assertion evaluated false")
            return None
        if kind == "pass":
            return None
        self.issue(node, f"opaque or unknown action IR kind: {kind}")
        return None

    def _assign_loop_target(self, target: dict[str, Any], value: Any, node: dict[str, Any]) -> None:
        if target.get("kind") == "sequence":
            items = target.get("items", [])
            if len(items) != 2 or not isinstance(value, (tuple, list)) or len(value) != 2:
                self.issue(node, "enumerate loop target must contain exactly two variables")
                return
            self._assign(items[0], value[0])
            self._assign(items[1], value[1])
        else:
            self._assign(target, value)

    def evaluate(self) -> ActionPlan:
        if not self.contract.supported:
            for issue in self.contract.issues:
                self.issues.append(PlanIssue(issue.span.as_dict(), f"cannot evaluate unsupported contract: {issue.reason}"))
            return ActionPlan(self.contract.path, self.variant, [], self.issues)
        self._block(self.contract.nodes)
        return ActionPlan(
            self.contract.path,
            self.variant,
            self.actions,
            self.issues,
            self.locals.get("ctx", {}).get("expected_stdout"),
        )


def evaluate_contract(contract: Contract, context: dict[str, Any], variant: str = "custom") -> ActionPlan:
    return Evaluator(contract, context, variant).evaluate()


def representative_contexts() -> dict[str, dict[str, Any]]:
    """Return deterministic host/JVM contexts for corpus-level dry runs."""
    base = {
        "host": True,
        "prebuild": True,
        "jvmti_redefine_stress": False,
        "switch_interpreter": False,
        "jvm": False,
        "O": True,
        "relocate": True,
        "testlib": ["libarttest.so"],
        "runtime_option": ["-Xopaque-jni-ids:true", "-Djava.library.path=/tmp/art-contract/native"],
        "stdout_file": "/tmp/art-contract/stdout.txt",
        "stderr_file": "/tmp/art-contract/stderr.txt",
    }

    def make(name: str, args_overrides: dict[str, Any], env_overrides: dict[str, Any]) -> tuple[str, dict[str, Any]]:
        args = dict(base)
        args.update(args_overrides)
        return name, {
            "args": args,
            "env": {
                "DEX_LOCATION": "/tmp/art-contract/dex",
                "ANDROID_LOG_TAGS": "*:e",
                "TEST_NAME": "art-contract",
                **env_overrides,
            },
            "expected_stdout": "/tmp/art-contract/expected.stdout",
            "path_root": "/tmp/art-contract",
        }

    host_name, host = make("host", {"prebuild": False}, {"ART_TEST_ON_VM": "1", "ANDROID_LOG_TAGS": "*:e", "TEST_NAME": "art-contract"})
    jvm_name, jvm = make(
        "jvm",
        {"jvm": True, "O": False, "prebuild": False, "switch_interpreter": True, "jvmti_redefine_stress": True},
        {},
    )
    return {host_name: host, jvm_name: jvm}


def discover(root: Path, paths: list[Path]) -> list[Path]:
    if paths:
        return sorted(path.resolve() for path in paths)
    return sorted(path.resolve() for path in root.glob("_aosp/art/test/[0-9]*/run.py"))


def summary(contract: Contract) -> str:
    capabilities = ",".join(sorted(contract.capabilities)) or "none"
    first = ""
    if contract.issues:
        issue = contract.issues[0]
        first = f" first_unsupported={issue.reason}@{issue.span.line}:{issue.span.column}"
    status = "SUPPORTED" if contract.supported else "AUDIT"
    return f"{status} {contract.path} capabilities={capabilities}{first}"


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("paths", nargs="*", type=Path, help="run.py files; defaults to all numeric AOSP tests")
    parser.add_argument("--root", type=Path, default=Path(__file__).resolve().parent.parent)
    parser.add_argument("--json", action="store_true", help="emit complete Contract IR as JSON")
    parser.add_argument("--strict", action="store_true", help="return failure when any unsupported construct is found")
    parser.add_argument("--evaluate", action="store_true", help="evaluate typed IR into data-only ActionPlans")
    parser.add_argument("--variant", choices=["host", "jvm"], help="limit --evaluate to one representative context")
    args = parser.parse_args(argv)
    paths = discover(args.root.resolve(), args.paths)
    contracts = [compile_path(path) for path in paths]
    if args.evaluate:
        contexts = representative_contexts()
        if args.variant:
            contexts = {args.variant: contexts[args.variant]}
        plans = [evaluate_contract(contract, context, variant) for variant, context in contexts.items() for contract in contracts]
        if args.json:
            json.dump([plan.as_dict() for plan in plans], sys.stdout, indent=2, sort_keys=True)
            sys.stdout.write("\n")
        else:
            for variant in contexts:
                variant_plans = [plan for plan in plans if plan.variant == variant]
                supported_plans = sum(plan.supported for plan in variant_plans)
                actions = sum(len(plan.actions) for plan in variant_plans)
                print(f"EVAL variant={variant} files={len(variant_plans)} supported={supported_plans} unknown={len(variant_plans) - supported_plans} actions={actions}")
        return 1 if args.strict and any(not plan.supported for plan in plans) else 0
    if args.json:
        json.dump([contract.as_dict() for contract in contracts], sys.stdout, indent=2, sort_keys=True)
        sys.stdout.write("\n")
    else:
        for contract in contracts:
            print(summary(contract))
        supported = sum(contract.supported for contract in contracts)
        capabilities = sorted({capability for contract in contracts for capability in contract.capabilities})
        print(
            f"TOTAL files={len(contracts)} supported={supported} "
            f"unsupported={len(contracts) - supported} capabilities={','.join(capabilities)}"
        )
    return 1 if args.strict and any(not contract.supported for contract in contracts) else 0


if __name__ == "__main__":
    raise SystemExit(main())
