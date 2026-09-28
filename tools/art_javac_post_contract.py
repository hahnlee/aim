#!/usr/bin/env python3
"""Parse AOSP ``javac_post.sh`` files into a typed, non-executing IR.

This is intentionally a small shell *frontend*, not a shell interpreter.  It
recognises the post-javac language used by the numeric ART tests and turns
every accepted effect into a typed operation.  No command text is retained in
the contract and no script is sourced, imported, or executed.  An unfamiliar
token is an error, including shell metacharacters and expansions.

The few command substitutions present in the pinned corpus are represented by
the ``basename`` value operation.  Only ``$(basename ${class})`` is accepted;
all other command substitutions are rejected.
"""

from __future__ import annotations

import argparse
from dataclasses import asdict, dataclass
import json
from pathlib import Path
import re
import sys
from typing import Any, Iterable


class ContractError(ValueError):
    """The input is outside the deliberately small post-javac language."""

    def __init__(self, path: Path, line: int, column: int, message: str) -> None:
        super().__init__(f"{path}:{line}:{column}: {message}")
        self.path = path
        self.line = line
        self.column = column


@dataclass(frozen=True)
class SourceSpan:
    path: str
    line: int
    column: int
    end_line: int
    end_column: int


@dataclass(frozen=True)
class ValueIR:
    kind: str
    value: Any
    span: SourceSpan


@dataclass(frozen=True)
class OperationIR:
    kind: str
    arguments: dict[str, Any]
    span: SourceSpan


@dataclass(frozen=True)
class JavacPostContract:
    test: str
    source: str
    operations: tuple[OperationIR, ...]

    @property
    def actions(self) -> tuple[OperationIR, ...]:
        """Compatibility spelling used by the other contract frontends."""
        return self.operations


@dataclass(frozen=True)
class _Part:
    kind: str
    value: str


@dataclass(frozen=True)
class _Token:
    kind: str
    value: str | tuple[_Part, ...]
    line: int
    column: int
    end_line: int
    end_column: int


_ASSIGNMENT_NAMES = frozenset({"ASM_JAR", "transformer_args", "transformed_class"})
_VARIABLES = frozenset({"1", "PWD", "class", "JAVA", "ASM_JAR", "ANDROID_BUILD_TOP",
                        "transformer_args",
                        "transformed_class"})
_SAFE_NAME = re.compile(r"^[A-Za-z_][A-Za-z0-9_]*$")


def _serial(value: Any) -> Any:
    if isinstance(value, (SourceSpan, ValueIR, OperationIR, JavacPostContract)):
        return {key: _serial(item) for key, item in asdict(value).items()}
    if isinstance(value, tuple):
        return [_serial(item) for item in value]
    if isinstance(value, list):
        return [_serial(item) for item in value]
    if isinstance(value, dict):
        return {key: _serial(item) for key, item in value.items()}
    return value


def contract_json(contract: JavacPostContract) -> dict[str, Any]:
    return _serial(contract)


def _span(path: Path, token: _Token) -> SourceSpan:
    return SourceSpan(str(path), token.line, token.column, token.end_line, token.end_column)


def _literal_span(path: Path, token: _Token) -> SourceSpan:
    return _span(path, token)


class _Lexer:
    """A lossless-enough lexer for the restricted shell grammar.

    Newlines become statement separators.  Comments and quoted characters are
    consumed here, while all expansions remain typed parts of a word.
    """

    _operators = ("&&", "||", ">>", "[[", "]]" )

    def __init__(self, path: Path, source: str) -> None:
        self.path = path
        self.source = source
        self.i = 0
        self.line = 1
        self.column = 1
        self.tokens: list[_Token] = []

    def error(self, line: int, column: int, message: str) -> ContractError:
        return ContractError(self.path, line, column, message)

    def advance(self, char: str) -> None:
        self.i += 1
        if char == "\n":
            self.line += 1
            self.column = 1
        else:
            self.column += 1

    def _word(self) -> None:
        start_line, start_column = self.line, self.column
        parts: list[_Part] = []
        literal: list[str] = []

        def flush() -> None:
            if literal:
                parts.append(_Part("literal", "".join(literal)))
                literal.clear()

        while self.i < len(self.source):
            char = self.source[self.i]
            if char in " \t\r\n" or char == ";" or char in "[]<>|&":
                break
            if char == "#":
                # A # in a word is data; a comment starts only at a token
                # boundary, which is handled by tokenize().
                literal.append(char)
                self.advance(char)
                continue
            if char in "'\"":
                quote = char
                self.advance(char)
                while self.i < len(self.source) and self.source[self.i] != quote:
                    char = self.source[self.i]
                    if char == "\n":
                        raise self.error(self.line, self.column, "newline in quoted word")
                    if quote == '"' and char == "$":
                        flush()
                        self._expansion(parts)
                    else:
                        literal.append(char)
                        self.advance(char)
                if self.i >= len(self.source):
                    raise self.error(start_line, start_column, "unterminated quote")
                self.advance(quote)
                continue
            if char == "\\":
                raise self.error(self.line, self.column, "backslash escapes are not allowed")
            if char == "`":
                raise self.error(self.line, self.column, "command substitution is not allowed")
            if char == "$":
                flush()
                self._expansion(parts)
                continue
            if char in "(){}?!":
                raise self.error(self.line, self.column, "shell metacharacter is not allowed")
            literal.append(char)
            self.advance(char)
        flush()
        if not parts:
            raise self.error(start_line, start_column, "empty shell word")
        self.tokens.append(_Token("word", tuple(parts), start_line, start_column,
                                  self.line, self.column))

    def _expansion(self, parts: list[_Part]) -> None:
        line, column = self.line, self.column
        assert self.source[self.i] == "$"
        self.advance("$")
        if self.i >= len(self.source):
            raise self.error(line, column, "incomplete expansion")
        if self.source[self.i] == "(":
            # The corpus has exactly one safe spelling.  Parse it into a
            # basename value; do not preserve the source command text.
            wanted = "(basename ${class})"
            if self.source.startswith(wanted, self.i):
                for char in wanted:
                    self.advance(char)
                parts.append(_Part("basename", "class"))
                return
            raise self.error(line, column, "command substitution is not allowed")
        if self.source[self.i] == "{":
            end = self.source.find("}", self.i + 1)
            if end < 0:
                raise self.error(line, column, "unterminated parameter expansion")
            body = self.source[self.i + 1:end]
            consumed = self.source[self.i:end + 1]
            if body == "JAVA:-java":
                for char in consumed:
                    self.advance(char)
                parts.append(_Part("env_default", "JAVA=java"))
                return
            if body not in _VARIABLES:
                raise self.error(line, column, f"unknown parameter expansion {body!r}")
            for char in consumed:
                self.advance(char)
            parts.append(_Part("variable", body))
            return
        if self.source[self.i].isdigit():
            name = self.source[self.i]
            self.advance(name)
        else:
            match = re.match(r"[A-Za-z_][A-Za-z0-9_]*", self.source[self.i:])
            if match is None:
                raise self.error(line, column, "invalid parameter expansion")
            name = match.group(0)
            for char in name:
                self.advance(char)
        if name not in _VARIABLES:
            raise self.error(line, column, f"unknown parameter expansion {name!r}")
        parts.append(_Part("variable", name))

    def tokenize(self) -> list[_Token]:
        while self.i < len(self.source):
            char = self.source[self.i]
            if char in " \t\r":
                self.advance(char)
                continue
            if char == "\n":
                self.tokens.append(_Token("op", ";", self.line, self.column,
                                          self.line, self.column + 1))
                self.advance(char)
                continue
            if char == "#":
                while self.i < len(self.source) and self.source[self.i] != "\n":
                    self.advance(self.source[self.i])
                continue
            matched = next((op for op in self._operators
                            if self.source.startswith(op, self.i)), None)
            if matched is not None:
                line, column = self.line, self.column
                for value in matched:
                    self.advance(value)
                self.tokens.append(_Token("op", matched, line, column,
                                          self.line, self.column))
                continue
            if char in ";[]<>|&":
                line, column = self.line, self.column
                self.advance(char)
                self.tokens.append(_Token("op", char, line, column,
                                          line, column + 1))
                continue
            self._word()
        self.tokens.append(_Token("eof", "", self.line, self.column,
                                  self.line, self.column))
        return self.tokens


class _Parser:
    def __init__(self, path: Path, source: str) -> None:
        self.path = path
        self.tokens = _Lexer(path, source).tokenize()
        self.i = 0
        self.operations: list[OperationIR] = []
        self.variables: dict[str, ValueIR] = {}
        self.loop_variables: set[str] = set()
        self.transformer: str | None = None

    @property
    def current(self) -> _Token:
        return self.tokens[self.i]

    def error(self, token: _Token, message: str) -> ContractError:
        return ContractError(self.path, token.line, token.column, message)

    def take(self, expected: str | None = None) -> _Token:
        token = self.current
        if expected is not None and self._word_text(token) != expected:
            raise self.error(token, f"expected {expected!r}")
        self.i += 1
        return token

    def op(self, expected: str) -> _Token:
        token = self.current
        if token.kind != "op" or token.value != expected:
            raise self.error(token, f"expected shell separator {expected!r}")
        self.i += 1
        return token

    def separators(self) -> None:
        while self.current.kind == "op" and self.current.value == ";":
            self.i += 1

    def _word_text(self, token: _Token) -> str | None:
        if token.kind != "word":
            return None
        parts = token.value
        assert isinstance(parts, tuple)
        if any(part.kind != "literal" for part in parts):
            return None
        return "".join(part.value for part in parts)

    def _op_text(self, token: _Token) -> str | None:
        if token.kind == "op" and isinstance(token.value, str):
            return token.value
        return self._word_text(token)

    def _value(self, token: _Token, *, path: bool = False) -> ValueIR:
        if token.kind != "word":
            raise self.error(token, "expected a word")
        parts = token.value
        assert isinstance(parts, tuple)
        if path:
            return self._path(token, parts)
        if len(parts) == 1 and parts[0].kind == "variable":
            return ValueIR("variable", parts[0].value, _span(self.path, token))
        if len(parts) == 1 and parts[0].kind == "env_default":
            return ValueIR("env_default", parts[0].value, _span(self.path, token))
        if len(parts) == 1 and parts[0].kind == "basename":
            return ValueIR("basename", parts[0].value, _span(self.path, token))
        if all(part.kind == "literal" for part in parts):
            return ValueIR("literal", "".join(part.value for part in parts),
                           _span(self.path, token))
        return ValueIR("concat", tuple(ValueIR(part.kind, part.value, _span(self.path, token))
                                        for part in parts), _span(self.path, token))

    @staticmethod
    def _safe_components(text: str) -> tuple[str, ...]:
        if not text or text.startswith("/") or "\\" in text:
            raise ValueError("path must be relative and slash-separated")
        components = tuple(text.split("/"))
        if any(component in {"", ".", ".."} for component in components):
            raise ValueError("path contains an unsafe component")
        if any(any(char in component for char in "|;&<>`()[]*?\n\r")
               for component in components):
            raise ValueError("path contains shell metacharacters")
        return components

    def _path(self, token: _Token, parts: tuple[_Part, ...]) -> ValueIR:
        span = _span(self.path, token)
        # Fixed absolute diagnostic output is mapped to a typed diagnostic
        # namespace.  No arbitrary host absolute path can enter the IR.
        if len(parts) == 1 and parts[0].kind == "literal" \
                and parts[0].value == "/tmp/2277-javac-output":
            return ValueIR("diagnostic_path", "2277-javac-output", span)
        if all(part.kind == "literal" for part in parts):
            text = "".join(part.value for part in parts)
            try:
                components = self._safe_components(text)
            except ValueError as error:
                raise self.error(token, str(error)) from error
            return ValueIR("cwd_path", components, span)

        if len(parts) == 1 and parts[0].kind == "variable" and parts[0].value == "1":
            return ValueIR("argument_path", 1, span)
        if (len(parts) == 2 and parts[0].kind == "variable" and parts[0].value == "1"
                and parts[1].kind == "literal"):
            suffix = parts[1].value
            if not suffix.startswith("-") or "/" in suffix:
                raise self.error(token, "argument path suffix is not confined")
            return ValueIR("argument_path_suffix", suffix, span)
        if (len(parts) == 2 and parts[0].kind == "variable" and parts[0].value == "PWD"
                and parts[1].kind == "literal" and parts[1].value == "/"):
            raise self.error(token, "bare PWD path is not allowed")
        if (len(parts) == 2 and parts[0].kind == "variable" and parts[0].value == "PWD"
                and parts[1].kind == "variable" and parts[1].value in self.loop_variables):
            return ValueIR("cwd_loop_path", parts[1].value, span)
        if (len(parts) == 3 and parts[0].kind == "variable" and parts[0].value == "PWD"
                and parts[1].kind == "literal" and parts[1].value == "/"
                and parts[2].kind == "variable" and parts[2].value in self.loop_variables):
            return ValueIR("cwd_loop_path", parts[2].value, span)
        if (len(parts) == 2 and parts[0].kind == "variable" and parts[0].value == "PWD"
                and parts[1].kind == "basename"):
            if parts[1].value not in self.loop_variables:
                raise self.error(token, "basename class value is only valid inside the class loop")
            return ValueIR("cwd_basename_path", parts[1].value, span)
        if (len(parts) == 3 and parts[0].kind == "variable" and parts[0].value == "1"
                and parts[1].kind == "literal" and parts[1].value == "/"
                and parts[2].kind == "basename"):
            if parts[2].value not in self.loop_variables:
                raise self.error(token, "basename class value is only valid inside the class loop")
            return ValueIR("argument_basename_path", parts[2].value, span)
        if len(parts) == 1 and parts[0].kind == "variable" \
                and parts[0].value in self.loop_variables:
            return ValueIR("cwd_loop_path", parts[0].value, span)
        if len(parts) == 1 and parts[0].kind == "basename":
            if parts[0].value not in self.loop_variables:
                raise self.error(token, "basename class value is only valid inside the class loop")
            return ValueIR("basename_path", parts[0].value, span)
        if (len(parts) == 1 and parts[0].kind == "variable"
                and parts[0].value == "transformed_class"):
            value = self.variables.get("transformed_class")
            if value is None:
                raise self.error(token, "transformed_class is used before assignment")
            return value
        raise self.error(token, "path expression is not confined to the test sandbox")

    def _operation(self, kind: str, token: _Token, **arguments: Any) -> OperationIR:
        return OperationIR(kind, arguments, _span(self.path, token))

    def _simple(self) -> OperationIR:
        first = self.take()
        text = self._word_text(first)
        if text is None:
            if self._value(first).kind == "env_default":
                return self._java(first)
            assignment_parts = first.value if first.kind == "word" else ()
            assignment_name = (
                re.match(r"^([A-Za-z_][A-Za-z0-9_]*)=", assignment_parts[0].value).group(1)
                if isinstance(assignment_parts, tuple) and assignment_parts
                and assignment_parts[0].kind == "literal"
                and re.match(r"^([A-Za-z_][A-Za-z0-9_]*)=", assignment_parts[0].value)
                else None
            )
            if assignment_name is None:
                raise self.error(first, "command name must be a literal")
        if text == "set":
            flag = self.take()
            if self._word_text(flag) != "-e":
                raise self.error(flag, "only set -e is supported")
            return self._operation("set_errexit", first)
        if text == "return":
            status = self.take()
            if self._word_text(status) != "0":
                raise self.error(status, "only return 0 is supported")
            return self._operation("return", first, status=0)
        if text == "export":
            assignment = self.take()
            name, value = self._assignment(assignment)
            if name != "ASM_JAR":
                raise self.error(assignment, "only ASM_JAR may be exported")
            return self._operation("set_tool_path", first, name=name, path=value)
        assignment_parts = first.value if first.kind == "word" else ()
        assignment_name = (
            re.match(r"^([A-Za-z_][A-Za-z0-9_]*)=", assignment_parts[0].value).group(1)
            if isinstance(assignment_parts, tuple) and assignment_parts
            and assignment_parts[0].kind == "literal"
            and re.match(r"^([A-Za-z_][A-Za-z0-9_]*)=", assignment_parts[0].value)
            else None
        )
        if assignment_name is not None:
            name, value = self._assignment(first)
            if name == "transformer_args":
                transformer = self._transformer_from_args(value, first)
                self.transformer = transformer
                return self._operation("set_transformer", first, transformer=transformer,
                                       classpath=(
                                           ValueIR("tool_variable", "ASM_JAR", _span(self.path, first)),
                                           ValueIR("cwd_path", ("transformer.jar",), _span(self.path, first)),
                                       ))
            if name == "transformed_class":
                self.variables[name] = value
                return self._operation("set_output_class", first, value=value)
            raise self.error(first, f"unknown assignment {name!r}")
        if text in {"rm", "mkdir", "mv", "javap"}:
            return self._command(text, first)
        # JAVA has a default expansion and is deliberately not a free-form
        # executable name.
        raise self.error(first, f"unknown command {text!r}")

    def _assignment(self, token: _Token) -> tuple[str, ValueIR]:
        parts = token.value
        if token.kind != "word" or not isinstance(parts, tuple) or not parts \
                or parts[0].kind != "literal":
            raise self.error(token, "assignment name must be literal")
        match = re.match(r"^([A-Za-z_][A-Za-z0-9_]*)=", parts[0].value)
        if match is None:
            raise self.error(token, "expected a simple assignment")
        name = match.group(1)
        if name not in _ASSIGNMENT_NAMES:
            raise self.error(token, f"assignment {name!r} is not supported")
        # The lexer has separated expansion parts, so reconstruct the RHS by
        # dropping the literal assignment prefix from the original token.
        prefix = f"{name}="
        if not parts or parts[0].kind != "literal" or not parts[0].value.startswith(prefix):
            raise self.error(token, "assignment has an invalid value")
        first = _Part("literal", parts[0].value[len(prefix):])
        rhs = tuple(part for part in (first, *parts[1:]) if part.value)
        synthetic = _Token("word", rhs, token.line, token.column,
                           token.end_line, token.end_column)
        return name, self._assignment_value(name, synthetic)

    def _assignment_value(self, name: str, token: _Token) -> ValueIR:
        parts = token.value
        assert isinstance(parts, tuple)
        span = _span(self.path, token)
        if name == "ASM_JAR":
            # Export is handled specially because the approved source uses the
            # literal Android build-top expansion, which is not a usable test
            # path.  It becomes a toolchain path, never an executable string.
            if len(parts) != 2 or parts[0].kind != "variable" \
                    or parts[0].value != "ANDROID_BUILD_TOP" \
                    or parts[1].kind != "literal" \
                    or parts[1].value != "/prebuilts/misc/common/asm/asm-9.6.jar":
                raise self.error(token, "ASM_JAR must use the approved build-top path")
            return ValueIR("toolchain_path", ("prebuilts", "misc", "common", "asm",
                                               "asm-9.6.jar"), span)
        if name == "transformed_class":
            if "class" not in self.loop_variables:
                raise self.error(token, "output class basename is only valid inside the class loop")
            if len(parts) == 2 and parts[0].kind == "literal" \
                    and parts[0].value == "classes/" \
                    and parts[1].kind == "basename" and parts[1].value == "class":
                return ValueIR("cwd_classes_basename_path", "class", span)
            if len(parts) == 3 and parts[0].kind == "variable" \
                    and parts[0].value == "1" \
                    and parts[1].kind == "literal" and parts[1].value == "/" \
                    and parts[2].kind == "basename" and parts[2].value == "class":
                return ValueIR("argument_basename_path", "class", span)
            raise self.error(token, "output class must be confined to classes or argument directory")
        return self._value(token, path=False)

    def _transformer_from_args(self, value: ValueIR, token: _Token) -> str:
        # Assignment expressions are inspected structurally by the parser;
        # no shell string is carried into the IR.  The class name is the only
        # changing field in the two AOSP transformer argument forms.
        parts = token.value
        assert isinstance(parts, tuple)
        text = "".join(part.value if part.kind == "literal" else
                        ("$PWD" if part.kind == "variable" and part.value == "PWD" else
                         "${" + part.value + "}" if part.kind == "variable" else
                         "$(basename ${class})" if part.kind == "basename" else
                         "${JAVA:-java}") for part in parts)
        match = re.fullmatch(
            r'(?:transformer_args=)?-cp \$\{ASM_JAR\}:(?:\$PWD/)?transformer\.jar transformer\.(IndyTransformer|ConstantTransformer)',
            text)
        if match is None:
            raise self.error(token, "transformer_args is not an approved typed classpath")
        return match.group(1)

    def _paths(self, tokens: list[_Token]) -> tuple[ValueIR, ...]:
        return tuple(self._value(token, path=True) for token in tokens)

    def _command(self, command: str, first: _Token) -> OperationIR:
        args: list[_Token] = []
        while not (self.current.kind == "op" and self.current.value in {";", ">>"}):
            if self.current.kind == "eof":
                break
            args.append(self.take())
        if command == "rm":
            force = recursive = False
            while args and self._word_text(args[0]) and self._word_text(args[0]).startswith("-"):
                flag = self._word_text(args.pop(0))
                assert flag is not None
                if any(char not in "fr" for char in flag[1:]):
                    raise self.error(first, "rm supports only -f and -r flags")
                force |= "f" in flag[1:]
                recursive |= "r" in flag[1:]
            if not args:
                raise self.error(first, "rm requires a confined path")
            return self._operation("delete", first, paths=self._paths(args), force=force,
                                   recursive=recursive)
        if command == "mkdir":
            parents = False
            if args and self._word_text(args[0]) == "-p":
                parents = True
                args.pop(0)
            if len(args) != 1:
                raise self.error(first, "mkdir requires one confined path")
            return self._operation("mkdir", first, path=self._value(args[0], path=True),
                                   parents=parents)
        if command == "mv":
            if len(args) != 2:
                raise self.error(first, "mv requires source and destination paths")
            return self._operation("move", first, source=self._value(args[0], path=True),
                                   destination=self._value(args[1], path=True))
        assert command == "javap"
        if len(args) != 4 or [self._word_text(arg) for arg in args[:3]] != ["-c", "-v", "-p"]:
            raise self.error(first, "javap must use exactly -c -v -p and one class path")
        if self.current.kind != "op" or self.current.value != ">>":
            raise self.error(self.current, "javap output must use the approved append target")
        self.i += 1
        target = self.take()
        if self._word_text(target) != "/tmp/2277-javac-output":
            raise self.error(target, "javap output is not confined to the approved diagnostic")
        source = self._value(args[3], path=True)
        if source.kind != "cwd_loop_path" or source.value != "class":
            raise self.error(args[3], "javap input must be the current class")
        return self._operation("capture_javap", first, input=source,
                               output=ValueIR("diagnostic_path", "2277-javac-output",
                                              _span(self.path, target)))

    def _java(self, first: _Token) -> OperationIR:
        args: list[_Token] = []
        while not (self.current.kind == "op" and self.current.value == ";"):
            if self.current.kind == "eof":
                break
            args.append(self.take())
        if self.transformer is None or len(args) != 3:
            raise self.error(first, "transformer invocation requires configured transformer and two class paths")
        if self._word_text(args[0]) != "${transformer_args}":
            # The real token has a variable part and cannot be _word_text.
            value = self._value(args[0])
            if value.kind != "variable" or value.value != "transformer_args":
                raise self.error(args[0], "invocation must use typed transformer_args")
        source = self._value(args[1], path=True)
        destination = self._value(args[2], path=True)
        if source.kind != "cwd_loop_path" or source.value != "class":
            raise self.error(args[1], "transformer input must be the current class")
        if destination.kind not in {"cwd_basename_path", "cwd_classes_basename_path",
                                    "argument_basename_path"} \
                or destination.value != "class":
            raise self.error(args[2], "transformer output must be classes/basename(class)")
        return self._operation("transform_class", first, transformer=self.transformer,
                               input=source, destination=destination)

    def _condition(self) -> ValueIR:
        opening = self.take()
        if self._op_text(opening) not in {"[", "[["}:
            raise self.error(opening, "condition must use [ ... ] or [[ ... ]]")
        left = self.take()
        # ``[ -f path ]`` has no infix operand, unlike ``[[ left == right ]]``.
        if self._word_text(left) == "-f":
            path = self.take()
            closing = self.take()
            if self._op_text(closing) != "]":
                raise self.error(closing, "expected ']'")
            return ValueIR("file_exists", self._value(path, path=True),
                           _span(self.path, opening))
        op = self.take()
        right = self.take()
        closing = self.take()
        if self._word_text(op) != "==":
            raise self.error(op, "condition operator is not supported")
        close_text = self._op_text(closing)
        expected_close = "]" if self._op_text(opening) == "[" else "]]"
        if close_text != expected_close:
            raise self.error(closing, f"expected {expected_close!r}")
        left_value = self._value(left)
        if left_value.kind == "variable" and left_value.value == "1":
            if self._word_text(right) != "classes2":
                raise self.error(right, "argument condition may only compare classes2")
            return ValueIR("argument_equals", "classes2", _span(self.path, opening))
        if left_value.kind == "variable" and left_value.value == "class":
            if "class" not in self.loop_variables:
                raise self.error(left, "class condition is only valid inside the class loop")
            pattern = self._word_text(right)
            if pattern != "*/FooConflict.class":
                raise self.error(right, "class condition may only match FooConflict.class")
            return ValueIR("class_matches", pattern, _span(self.path, opening))
        raise self.error(left, "condition must compare the typed argument or class variable")

    def _block(self, terminators: frozenset[str]) -> tuple[OperationIR, ...]:
        result: list[OperationIR] = []
        self.separators()
        while self.current.kind != "eof":
            marker = self._word_text(self.current)
            if marker in terminators:
                break
            result.append(self._statement())
            self.separators()
        return tuple(result)

    def _statement(self) -> OperationIR:
        marker = self._word_text(self.current)
        if marker == "if":
            begin = self.take()
            condition = self._condition()
            self.separators()
            self.take("then")
            body = self._block(frozenset({"else", "fi"}))
            otherwise: tuple[OperationIR, ...] = ()
            if self._word_text(self.current) == "else":
                self.take()
                otherwise = self._block(frozenset({"fi"}))
            self.take("fi")
            return self._operation("branch", begin, condition=condition, then=body,
                                   otherwise=otherwise)
        if marker == "for":
            begin = self.take()
            variable = self.take()
            name = self._word_text(variable)
            if name != "class":
                raise self.error(variable, "only the class loop variable is supported")
            self.take("in")
            glob = self.take()
            pattern = self._glob(glob)
            self.op(";")
            self.take("do")
            self.loop_variables.add("class")
            body = self._block(frozenset({"done"}))
            self.take("done")
            self.loop_variables.remove("class")
            return self._operation("for_each_class", begin, variable=name, glob=pattern,
                                   body=body)
        return self._simple()

    def _glob(self, token: _Token) -> ValueIR:
        parts = token.value
        assert isinstance(parts, tuple)
        text = "".join(part.value for part in parts)
        if text in {"intermediate-classes/*.class", "$1-intermediate-classes/*.class"}:
            root = "argument1" if text.startswith("$1") else "cwd"
            return ValueIR("class_glob", {"root": root, "directory": "intermediate-classes",
                                           "suffix": ".class"}, _span(self.path, token))
        # Dynamic words are handled structurally rather than by interpolating.
        if (len(parts) == 2 and parts[0].kind == "variable" and parts[0].value == "1"
                and parts[1].kind == "literal" and parts[1].value == "-intermediate-classes/*.class"):
            return ValueIR("class_glob", {"root": "argument1", "directory": "intermediate-classes",
                                           "suffix": ".class"}, _span(self.path, token))
        raise self.error(token, "class loop glob is not confined to intermediate-classes")

    def parse(self) -> tuple[OperationIR, ...]:
        self.separators()
        # A shebang is metadata, not executable syntax.  The lexer sees it as
        # a comment because # is at a token boundary.
        result = self._block(frozenset())
        if self.current.kind != "eof":
            raise self.error(self.current, "unexpected trailing shell syntax")
        return result


def compile_contract(source: str | bytes, path: Path | str = "<memory>") -> JavacPostContract:
    """Compile source bytes/text without importing or executing the script."""
    target = Path(path)
    if isinstance(source, bytes):
        try:
            source = source.decode("utf-8")
        except UnicodeDecodeError as error:
            raise ContractError(target, 1, 1, f"source is not valid UTF-8: {error}") from error
    if "\x00" in source:
        raise ContractError(target, 1, 1, "NUL byte is not valid shell source")
    operations = _Parser(target, source).parse()
    return JavacPostContract(target.parent.name, str(target), operations)


def audit(root: Path, tests: Iterable[str] | None = None) -> tuple[list[JavacPostContract], list[str]]:
    """Compile every numeric ART javac_post.sh and report all failures."""
    test_root = root / "_aosp" / "art" / "test"
    requested = set(tests or ())
    contracts: list[JavacPostContract] = []
    errors: list[str] = []
    for path in sorted(test_root.glob("*/javac_post.sh")):
        if not re.match(r"^\d", path.parent.name):
            continue
        if requested and path.parent.name not in requested:
            continue
        try:
            contracts.append(compile_contract(path.read_bytes(), path))
        except (OSError, ContractError) as error:
            errors.append(str(error))
    return contracts, errors


audit_corpus = audit


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, default=Path(__file__).resolve().parent.parent)
    parser.add_argument("--test", action="append", default=[])
    parser.add_argument("--json", action="store_true", dest="as_json")
    parser.add_argument(
        "--strict",
        action="store_true",
        help="require every selected script to compile without unsupported syntax",
    )
    args = parser.parse_args(argv)
    contracts, errors = audit(args.root.resolve(), args.test)
    if args.as_json:
        print(json.dumps({"contracts": [contract_json(item) for item in contracts],
                          "errors": errors, "opaque": 0}, indent=2, sort_keys=True))
    else:
        count = sum(len(item.operations) for item in contracts)
        print(f"JAVAC-POST-CONTRACT {'PASS' if not errors else 'FAIL'} "
              f"tests={len(contracts)} operations={count} opaque=0 unsupported={len(errors)}")
        for error in errors:
            print(error, file=sys.stderr)
    # The frontend is fail-closed by default.  --strict is intentionally an
    # explicit CI spelling of the same contract, matching the other two
    # custom-script frontends.
    return 0 if not errors else 1


if __name__ == "__main__":
    raise SystemExit(main())
