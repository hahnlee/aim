#!/usr/bin/env python3
"""Evaluate and execute the typed ART build/post-javac contracts.

The contract frontends deliberately stop before execution.  This module is
the small, separately-auditable boundary that turns their IR into an
``ActionPlan``.  Paths are resolved against an explicit test root or
toolchain root and checked again immediately before every filesystem or
subprocess operation.  Subprocesses receive argv and ``shell=False``; shell
source is never accepted by this module.
"""

from __future__ import annotations

from dataclasses import dataclass, field
import argparse
import fnmatch
import os
from pathlib import Path
import shutil
import subprocess
import sys
from typing import Any, Callable, Iterable, Mapping, Sequence


class ActionPlanError(ValueError):
    """A typed action cannot be safely evaluated or executed."""

    def __init__(self, message: str, span: Any = None) -> None:
        if span is not None:
            path = getattr(span, "path", "<contract>")
            line = getattr(span, "line", 1)
            column = getattr(span, "column", 0) + 1
            message = f"{path}:{line}:{column}: {message}"
        super().__init__(message)
        self.span = span


@dataclass
class ActionContext:
    """Concrete roots, variables, and explicitly supplied tool executables."""

    test_root: Path
    toolchain_root: Path | None = None
    argument1: Path | None = None
    jvm: bool = False
    mode: str = ""
    tools: Mapping[str, Path] = field(default_factory=dict)
    # Explicit host tools are allowed to live outside the repository
    # toolchain root.  They are still named and supplied by the caller (never
    # discovered from contract text), which keeps this escape hatch narrow
    # while allowing javac_post's approved Java transformer to use the host
    # JDK.
    external_tools: Mapping[str, Path] = field(default_factory=dict)
    environment: Mapping[str, str] | None = None
    diagnostics_root: Path | None = None
    default_build: Callable[[Mapping[str, Any]], Any] | None = None
    dry_run: bool = False

    def __post_init__(self) -> None:
        self.test_root = Path(self.test_root).absolute()
        if self.toolchain_root is not None:
            self.toolchain_root = Path(self.toolchain_root).absolute()
        if self.argument1 is not None:
            self.argument1 = Path(self.argument1).absolute()
        if self.diagnostics_root is None:
            self.diagnostics_root = self.test_root / ".diagnostics"
        else:
            self.diagnostics_root = Path(self.diagnostics_root).absolute()


ContractContext = ActionContext
ExecutionContext = ActionContext


@dataclass(frozen=True)
class PlannedOperation:
    kind: str
    arguments: Mapping[str, Any]
    span: Any


@dataclass(frozen=True)
class ActionPlan:
    source: str
    operations: tuple[PlannedOperation, ...]

    @property
    def actions(self) -> tuple[PlannedOperation, ...]:
        return self.operations

    def execute(self, context: ActionContext, *, check: bool = True) -> None:
        execute_plan(self, context, check=check)


def _span(node: Any) -> Any:
    return getattr(node, "span", None)


def _fail(node: Any, message: str) -> ActionPlanError:
    return ActionPlanError(message, _span(node))


def _as_parts(value: Any) -> tuple[Any, ...] | None:
    return tuple(value) if isinstance(value, (tuple, list)) else None


def _root(context: ActionContext, which: str, node: Any) -> Path:
    root = context.toolchain_root if which == "toolchain" else context.test_root
    if root is None:
        raise _fail(node, f"{which} root is not configured")
    return root.resolve()


def _confined(path: Path, root: Path, node: Any, *, reject_symlink: bool = True) -> Path:
    """Return a canonical path below root, rejecting symlink escapes."""
    root = root.resolve()
    candidate = path if path.is_absolute() else root / path
    try:
        resolved = candidate.resolve(strict=False)
        if os.path.commonpath((str(root), str(resolved))) != str(root):
            raise _fail(node, f"path escapes confined root: {candidate}")
        # ``resolve`` above catches symlink escapes while allowing harmless
        # system aliases such as macOS /var -> /private/var.  Do not inspect
        # lexical parents here: doing so would reject every path below such a
        # platform alias before canonical confinement can be checked.
    except OSError as error:
        raise _fail(node, f"cannot resolve confined path {candidate}: {error}") from error
    return resolved


def _relative_components(value: Any, node: Any) -> tuple[str, ...]:
    if not isinstance(value, (tuple, list)) or not value:
        raise _fail(node, "path components are not typed")
    components = tuple(value)
    if any(not isinstance(item, str) or not item or item in {".", ".."}
           or "/" in item or "\\" in item for item in components):
        raise _fail(node, "unsafe path components")
    return components


def _join_confined(base: Path, components: Sequence[str], root: Path, node: Any) -> Path:
    return _confined(base.joinpath(*_relative_components(components, node)), root, node)


def _evaluated_path(path: Path, root: Path, node: Any, *, dry_run: bool) -> Path:
    """Keep corpus-only symlink fixtures lexical; execution always rechecks."""
    if not dry_run:
        return _confined(path, root, node)
    root = root.absolute()
    candidate = path if path.is_absolute() else root / path
    normalized = Path(os.path.normpath(str(candidate)))
    try:
        if os.path.commonpath((str(root), str(normalized))) != str(root):
            raise _fail(node, f"path escapes confined root: {candidate}")
    except OSError as error:
        raise _fail(node, f"cannot resolve confined path {candidate}: {error}") from error
    return normalized


def _join_evaluated(base: Path, components: Sequence[str], root: Path, node: Any, *, dry_run: bool) -> Path:
    return _evaluated_path(base.joinpath(*_relative_components(components, node)), root, node,
                           dry_run=dry_run)


class _Evaluator:
    def __init__(self, context: ActionContext) -> None:
        self.context = context
        self.variables: dict[str, Any] = {
            "jvm": context.jvm,
            "mode": context.mode,
        }

    def confined(self, path: Path, root: Path, node: Any) -> Path:
        return _evaluated_path(path, root, node, dry_run=self.context.dry_run)

    def join(self, base: Path, components: Sequence[str], root: Path, node: Any) -> Path:
        return _join_evaluated(base, components, root, node, dry_run=self.context.dry_run)

    def value(self, value: Any, env: Mapping[str, Any] | None = None, *, runtime: bool = False) -> Any:
        env = self.variables if env is None else env
        kind = getattr(value, "kind", None)
        raw = getattr(value, "value", None)
        if kind is None:
            if isinstance(value, (tuple, list)):
                return tuple(self.value(item, env, runtime=runtime) for item in value)
            if isinstance(value, Mapping):
                return {self.value(key, env, runtime=runtime): self.value(item, env, runtime=runtime)
                        for key, item in value.items()}
            return value
        if kind == "literal":
            return raw
        if kind == "context_path" and raw == "test_dir":
            return _root(self.context, "test", value)
        if kind == "context_string":
            if raw != "mode":
                raise _fail(value, f"unknown context string {raw!r}")
            return self.context.mode
        if kind == "context_bool":
            if raw != "jvm":
                raise _fail(value, f"unknown context boolean {raw!r}")
            return self.context.jvm
        if kind == "variable":
            if raw not in env:
                raise _fail(value, f"unbound typed variable {raw!r}")
            return env[raw]
        if kind == "env_default":
            name, default = str(raw).split("=", 1)
            return env.get(name, default)
        if kind == "tool_variable":
            if raw not in env:
                raise _fail(value, f"unbound tool variable {raw!r}")
            return env[raw]
        if kind == "toolchain_path":
            return self.join(_root(self.context, "toolchain", value), raw,
                             _root(self.context, "toolchain", value), value)
        if kind == "diagnostic_path":
            return self.join(_root(self.context, "test", value), (".diagnostics", str(raw)),
                             _root(self.context, "test", value), value)
        if kind == "cwd_path":
            return self.join(_root(self.context, "test", value), raw,
                             _root(self.context, "test", value), value)
        if kind == "argument_path":
            if self.context.argument1 is None:
                raise _fail(value, "javac_post argument $1 is not configured")
            return self.confined(self.context.argument1, _root(self.context, "test", value), value)
        if kind == "argument_path_suffix":
            if self.context.argument1 is None:
                raise _fail(value, "javac_post argument $1 is not configured")
            arg = self.confined(self.context.argument1, _root(self.context, "test", value), value)
            suffix = str(raw)
            if not suffix.startswith("-") or "/" in suffix or "\\" in suffix:
                raise _fail(value, "unsafe argument path suffix")
            return self.confined(arg.parent / (arg.name + suffix), _root(self.context, "test", value), value)
        if kind in {"cwd_loop_path", "cwd_basename_path", "cwd_classes_basename_path", "basename_path"}:
            current = env.get(str(raw))
            if not isinstance(current, Path):
                raise _fail(value, "class loop value is not a path")
            current = self.confined(current, _root(self.context, "test", value), value)
            if kind == "cwd_loop_path":
                return current
            base = _root(self.context, "test", value)
            if kind == "cwd_classes_basename_path":
                base = base / "classes"
            return self.confined(base / current.name,
                                 _root(self.context, "test", value), value)
        if kind == "argument_basename_path":
            if self.context.argument1 is None:
                raise _fail(value, "javac_post argument $1 is not configured")
            current = env.get(str(raw))
            if not isinstance(current, Path):
                raise _fail(value, "class loop value is not a path")
            arg = self.confined(self.context.argument1, _root(self.context, "test", value), value)
            return self.confined(arg / current.name, _root(self.context, "test", value), value)
        if kind == "path_join":
            item = raw
            if not isinstance(item, Mapping):
                raise _fail(value, "malformed path join")
            left = self.value(item["left"], env)
            right = self.value(item["right"], env)
            if not isinstance(left, Path) or not isinstance(right, str):
                raise _fail(value, "path join operands are not typed")
            try:
                components = _relative_components(tuple(right.split("/")), value)
            except ActionPlanError:
                raise
            return self.confined(left.joinpath(*components), _root(self.context, "test", value), value)
        if kind == "string_concat":
            item = raw
            if not isinstance(item, Mapping):
                raise _fail(value, "malformed string concatenation")
            left, right = self.value(item["left"], env), self.value(item["right"], env)
            if not isinstance(left, str) or not isinstance(right, str):
                raise _fail(value, "string concatenation has non-string operand")
            return left + right
        if kind in {"list", "tuple"}:
            return tuple(self.value(item, env) for item in raw)
        if kind == "dict":
            return {self.value(key, env): self.value(item, env) for key, item in raw}
        if kind == "glob":
            pattern = raw.get("pattern") if isinstance(raw, Mapping) else None
            if pattern != "classes*":
                raise _fail(value, "unknown filesystem glob")
            root = _root(self.context, "test", value)
            return tuple(self.confined(item, root, value) for item in sorted(root.glob("classes*")))
        if kind == "class_glob":
            if not isinstance(raw, Mapping) or raw.get("suffix") != ".class":
                raise _fail(value, "malformed class glob")
            if raw.get("root") == "cwd":
                base = _root(self.context, "test", value) / "intermediate-classes"
            elif raw.get("root") == "argument1":
                if self.context.argument1 is None:
                    raise _fail(value, "javac_post argument $1 is not configured")
                arg = self.confined(self.context.argument1, _root(self.context, "test", value), value)
                base = arg.parent / (arg.name + "-intermediate-classes")
            else:
                raise _fail(value, "unknown class glob root")
            base = self.confined(base, _root(self.context, "test", value), value)
            return tuple(self.confined(item, _root(self.context, "test", value), value)
                         for item in sorted(base.glob("*.class")))
        if kind in {"basename", "file_read"}:
            if runtime:
                return value
            raise _fail(value, f"deferred value {kind!r} cannot be evaluated here")
        raise _fail(value, f"unknown typed value kind {kind!r}")

    def condition(self, value: Any, env: Mapping[str, Any]) -> bool:
        kind = getattr(value, "kind", None)
        raw = getattr(value, "value", None)
        if kind == "not":
            return not self.condition(raw, env)
        if kind == "equals":
            if not isinstance(raw, Mapping):
                raise _fail(value, "malformed equality condition")
            if getattr(raw["left"], "kind", None) == "file_read":
                return value
            return self.value(raw["left"], env) == self.value(raw["right"], env)
        if kind == "file_exists":
            path = self.value(raw, env)
            return path.is_file()
        if kind == "argument_equals":
            if self.context.argument1 is None:
                raise _fail(value, "javac_post argument $1 is not configured")
            return self.context.argument1.name == str(raw)
        if kind == "class_matches":
            current = env.get("class")
            if not isinstance(current, Path):
                raise _fail(value, "class condition has no current class")
            rel = current.relative_to(_root(self.context, "test", value)).as_posix()
            return fnmatch.fnmatch(rel, str(raw))
        if kind == "context_bool":
            return bool(self.value(value, env))
        raise _fail(value, f"unknown condition kind {kind!r}")

    def file_operations(self, operations: Iterable[Any], env: Mapping[str, Any]) -> tuple[PlannedOperation, ...]:
        result: list[PlannedOperation] = []
        for operation in operations:
            args = dict(getattr(operation, "arguments", {}))
            if operation.kind == "branch":
                result.append(PlannedOperation("branch", {
                        "condition": self.condition(args["condition"], env),
                    "then": self.file_operations(args["then"], env),
                    "else": self.file_operations(args.get("else", args.get("otherwise", ())), env),
                }, operation.span))
            elif operation.kind in {"assert_equals", "seek", "write"}:
                result.append(PlannedOperation(operation.kind, {
                    key: (item if getattr(item, "kind", None) == "file_read"
                          else self.value(item, env))
                    for key, item in args.items()
                }, operation.span))
            else:
                raise _fail(operation, f"unknown file operation {operation.kind!r}")
        return tuple(result)

    def operations(self, source_operations: Iterable[Any], env: Mapping[str, Any] | None = None) -> tuple[PlannedOperation, ...]:
        env = self.variables if env is None else dict(env)
        result: list[PlannedOperation] = []
        for operation in source_operations:
            kind = getattr(operation, "kind", None)
            args = dict(getattr(operation, "arguments", {}))
            if kind == "branch":
                result.append(PlannedOperation("branch", {
                    "condition": self.condition(args["condition"], env),
                    "then": self.operations(args["then"], env),
                    "else": self.operations(args.get("else", args.get("otherwise", ())), env),
                }, operation.span))
            elif kind in {"for_each_class"}:
                result.append(PlannedOperation(kind, {
                    # Class files are created by an earlier move/mkdir in the
                    # same post script.  Keep this typed glob and body
                    # deferred so execution observes that ordered state.
                    "glob": args["glob"], "variable": args["variable"],
                    "body": args["body"],
                }, operation.span))
            elif kind == "file_edit":
                result.append(PlannedOperation(kind, {
                    "path": self.value(args["path"], env),
                    "mode": self.value(args["mode"], env),
                    "operations": self.file_operations(args["operations"], env),
                }, operation.span))
            elif kind == "delete":
                if "paths" in args:
                    paths = tuple(self.value(item, env) for item in args["paths"])
                    result.append(PlannedOperation(kind, {
                        "paths": paths, "force": bool(args.get("force", False)),
                        "recursive": bool(args.get("recursive", False)),
                    }, operation.span))
                else:
                    resolved_path = self.value(args["path"], env)
                    result.append(PlannedOperation(kind, {
                        "paths": resolved_path if isinstance(resolved_path, tuple)
                        else (resolved_path,),
                        "force": True, "recursive": bool(args.get("recursive", False)),
                    }, operation.span))
            elif kind in {"mkdir", "rename", "move", "capture_javap", "transform_class",
                          "generate_sources", "soong_zip", "set_tool_path", "set_transformer",
                          "set_output_class", "set_errexit", "return", "noop", "default_build"}:
                resolved = {}
                for key, item in args.items():
                    if key == "argv":
                        argv = self.value(item, env)
                        if not isinstance(argv, tuple) or not all(isinstance(x, (str, Path)) for x in argv):
                            raise _fail(operation, "argv is not typed")
                        resolved[key] = argv
                    elif key in {"path", "source", "destination", "input", "output"}:
                        resolved[key] = self.value(item, env)
                    elif key == "classpath":
                        resolved[key] = tuple(self.value(x, env) for x in item)
                    elif key == "kwargs":
                        resolved[key] = {name: self.value(x, env) for name, x in item.items()}
                    else:
                        resolved[key] = item
                result.append(PlannedOperation(kind, resolved, operation.span))
                if kind == "set_tool_path":
                    env[args["name"]] = self.value(args["path"], env)
                elif kind == "set_output_class":
                    env["transformed_class"] = self.value(args["value"], env)
            else:
                raise _fail(operation, f"unknown action kind {kind!r}")
        return tuple(result)


def evaluate_contract(contract: Any, context: ActionContext) -> ActionPlan:
    """Resolve a typed contract into a concrete, still non-executing plan."""
    operations = getattr(contract, "operations", None)
    if operations is None:
        operations = getattr(contract, "actions", None)
    if operations is None:
        raise ActionPlanError("object is not a supported typed contract")
    evaluator = _Evaluator(context)
    return ActionPlan(str(getattr(contract, "source", "<contract>")),
                      evaluator.operations(operations))


evaluate_action_plan = evaluate_contract
build_action_plan = evaluate_contract
javac_post_action_plan = evaluate_contract
evaluate_build_contract = evaluate_contract
evaluate_javac_post_contract = evaluate_contract


class _Returned(Exception):
    pass


class _Executor:
    def __init__(self, context: ActionContext, *, check: bool = True) -> None:
        self.context = context
        self.check = check
        self.variables: dict[str, Any] = {"ASM_JAR": None, "transformer_args": None}
        self.evaluator = _Evaluator(context)

    def path(self, value: Any, node: Any, *, root: str = "test") -> Path:
        if not isinstance(value, Path):
            raise _fail(node, "executor received a non-path value")
        return _confined(value, _root(self.context, root, node), node)

    def tool(self, name: str, node: Any) -> Path:
        external = self.context.external_tools.get(name)
        if external is not None:
            path = Path(external).absolute()
            if not path.is_file() or not os.access(path, os.X_OK):
                raise _fail(node, f"external tool {name!r} is missing or not executable")
            return path
        value = self.context.tools.get(name)
        if value is None:
            raise _fail(node, f"tool {name!r} was not explicitly configured")
        path = Path(value)
        root = _root(self.context, "toolchain", node)
        return _confined(path, root, node)

    def subprocess(self, argv: Sequence[Any], node: Any, *, cwd: Path,
                   stdout: Any = None) -> None:
        if not argv or any(not isinstance(item, (str, Path)) for item in argv):
            raise _fail(node, "subprocess argv is not typed")
        concrete = [str(item) for item in argv]
        if any(any(char in item for char in "|;&<>`\n\r") for item in concrete):
            raise _fail(node, "subprocess argv contains shell metacharacters")
        try:
            subprocess.run(concrete, cwd=str(self.path(cwd, node)), stdout=stdout,
                           check=self.check, shell=False,
                           env=(dict(self.context.environment)
                                if self.context.environment is not None else None))
        except OSError as error:
            raise _fail(node, f"subprocess failed: {error}") from error

    def file_edit(self, operation: PlannedOperation) -> None:
        args = operation.arguments
        path = self.path(args["path"], operation)
        mode = args["mode"]
        if not isinstance(mode, str) or mode not in {"r", "rb", "r+", "rb+", "w", "wb", "w+", "wb+"}:
            raise _fail(operation, "unsafe file mode")
        binary = "b" in mode
        try:
            with path.open(mode) as handle:
                for item in args["operations"]:
                    if item.kind == "branch":
                        condition = self.file_condition(item.arguments["condition"], handle, item)
                        nested = item.arguments["then"] if condition else item.arguments["else"]
                        self.file_ops(nested, handle, binary, item)
                    else:
                        self.file_ops((item,), handle, binary, item)
        except OSError as error:
            raise _fail(operation, f"file operation failed: {error}") from error

    def file_ops(self, operations: Iterable[PlannedOperation], handle: Any,
                 binary: bool, node: Any) -> None:
        for item in operations:
            args = item.arguments
            if item.kind == "assert_equals":
                left = args["left"]
                if getattr(left, "kind", None) != "file_read":
                    raise _fail(item, "assertion left side is not a typed read")
                size = left.value.get("size") if isinstance(left.value, Mapping) else None
                size = size.value if hasattr(size, "value") else size
                actual = handle.read() if size is None else handle.read(int(size))
                if actual != args["right"]:
                    raise _fail(item, "file assertion failed")
            elif item.kind == "seek":
                handle.seek(args["argument"])
            elif item.kind == "write":
                data = args["argument"]
                if binary and not isinstance(data, (bytes, bytearray)):
                    raise _fail(item, "binary write requires bytes")
                handle.write(data)
            else:
                raise _fail(item, f"unknown file operation {item.kind!r}")

    @staticmethod
    def file_condition(condition: Any, handle: Any, node: Any) -> bool:
        kind = getattr(condition, "kind", None)
        raw = getattr(condition, "value", None)
        if kind == "equals" and isinstance(raw, Mapping):
            left = raw.get("left")
            if getattr(left, "kind", None) != "file_read":
                raise _fail(node, "file branch condition is not a typed read")
            payload = getattr(left, "value", {})
            size = payload.get("size") if isinstance(payload, Mapping) else None
            size = getattr(size, "value", size)
            actual = handle.read() if size is None else handle.read(int(size))
            return actual == raw.get("right").value
        if kind == "not":
            return not _Executor.file_condition(raw, handle, node)
        if isinstance(condition, bool):
            return condition
        raise _fail(node, "file branch condition cannot be evaluated")

    def operation(self, operation: PlannedOperation) -> None:
        kind, args = operation.kind, operation.arguments
        if kind in {"noop", "set_errexit", "set_transformer", "set_output_class"}:
            if kind == "set_tool_path":
                self.variables[args["name"]] = args["path"]
            return
        if kind == "set_tool_path":
            self.variables[args["name"]] = self.path(args["path"], operation, root="toolchain")
            return
        if kind == "return":
            raise _Returned
        if kind == "branch":
            selected = args["then"] if args["condition"] else args["else"]
            self.operations(selected)
            return
        if kind == "for_each_class":
            self.evaluator.variables.update(self.variables)
            items = self.evaluator.value(args["glob"], self.evaluator.variables)
            if not isinstance(items, tuple):
                raise _fail(operation, "class glob did not evaluate to paths")
            for item in items:
                child = dict(self.evaluator.variables)
                child[args["variable"]] = item
                self.operations(self.evaluator.operations(args["body"], child))
            self.variables.update(self.evaluator.variables)
            return
        if kind == "generate_sources":
            argv = list(args["argv"])
            if not argv or argv[0] != "./generate-sources":
                raise _fail(operation, "generator executable is not the approved local tool")
            executable = self.path(self.context.test_root / "generate-sources", operation)
            if not executable.is_file() or not os.access(executable, os.X_OK):
                raise _fail(operation, "generate-sources is missing or not executable")
            self.subprocess([executable, *argv[1:]], operation, cwd=self.context.test_root)
            return
        if kind == "delete":
            for candidate in args["paths"]:
                path = self.path(candidate, operation)
                if not path.exists() and not path.is_symlink():
                    if args.get("force"):
                        continue
                    raise _fail(operation, f"missing delete target: {path}")
                if args.get("recursive"):
                    shutil.rmtree(path)
                else:
                    path.unlink()
            return
        if kind == "mkdir":
            path = self.path(args["path"], operation)
            path.mkdir(parents=bool(args.get("parents", args.get("recursive", False))), exist_ok=True)
            return
        if kind in {"move", "rename"}:
            source = self.path(args["source"], operation)
            destination = self.path(args["destination"], operation)
            destination.parent.mkdir(parents=True, exist_ok=True)
            shutil.move(str(source), str(destination))
            return
        if kind == "file_edit":
            self.file_edit(operation)
            return
        if kind == "transform_class":
            java = self.tool("java", operation)
            asm = self.variables.get("ASM_JAR")
            if not isinstance(asm, Path):
                raise _fail(operation, "ASM_JAR tool path was not configured")
            asm = self.path(asm, operation, root="toolchain")
            transformer_jar = self.path(self.context.test_root / "transformer.jar", operation)
            source = self.path(args["input"], operation)
            destination = self.path(args["destination"], operation)
            destination.parent.mkdir(parents=True, exist_ok=True)
            cp = f"{asm}:{transformer_jar}"
            self.subprocess([java, "-cp", cp, f"transformer.{args['transformer']}", source, destination],
                            operation, cwd=self.context.test_root)
            return
        if kind == "capture_javap":
            javap = self.tool("javap", operation)
            source = self.path(args["input"], operation)
            output = self.path(args["output"], operation)
            output.parent.mkdir(parents=True, exist_ok=True)
            with output.open("ab") as stream:
                self.subprocess([javap, "-c", "-v", "-p", source], operation,
                                cwd=self.context.test_root, stdout=stream)
            return
        if kind == "soong_zip":
            tool = self.tool("soong_zip", operation)
            self.subprocess([tool, *args["argv"]], operation, cwd=self.context.test_root)
            return
        if kind == "default_build":
            if self.context.default_build is None:
                raise _fail(operation, "default_build requires an explicit backend callback")
            self.context.default_build(args.get("kwargs", {}))
            return
        raise _fail(operation, f"unknown planned action {kind!r}")

    def operations(self, operations: Iterable[PlannedOperation]) -> None:
        for operation in operations:
            self.operation(operation)


def execute_plan(plan: ActionPlan, context: ActionContext, *, check: bool = True) -> None:
    """Execute a previously evaluated plan using only safe APIs."""
    try:
        _Executor(context, check=check).operations(plan.operations)
    except _Returned:
        return


execute_action_plan = execute_plan
execute = execute_plan


def dry_run_corpus(root: Path) -> tuple[int, list[str]]:
    """Evaluate every numeric build and javac contract without running tools."""
    import importlib.util
    import sys

    def load(name: str, filename: str) -> Any:
        path = Path(__file__).with_name(filename)
        spec = importlib.util.spec_from_file_location(name, path)
        if spec is None or spec.loader is None:
            raise RuntimeError(f"cannot load {filename}")
        module = importlib.util.module_from_spec(spec)
        sys.modules[name] = module
        spec.loader.exec_module(module)
        return module

    build = load("art_build_contract_for_action_plan", "art_build_contract.py")
    javac = load("art_javac_post_contract_for_action_plan", "art_javac_post_contract.py")
    contracts, errors = build.audit(root)
    post_contracts, post_errors = javac.audit(root)
    errors = list(errors) + list(post_errors)
    for contract in (*contracts, *post_contracts):
        source = Path(contract.source)
        test_root = source.parent
        argument1 = test_root / "classes"
        try:
            evaluate_contract(contract, ActionContext(
                test_root=test_root, toolchain_root=root, argument1=argument1,
                dry_run=True))
        except ActionPlanError as error:
            errors.append(str(error))
    return len(contracts) + len(post_contracts), errors


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, default=Path(__file__).resolve().parent.parent)
    args = parser.parse_args(argv)
    count, errors = dry_run_corpus(args.root.resolve())
    print(f"ACTION-PLAN {'PASS' if not errors else 'FAIL'} "
          f"contracts={count} unsupported={len(errors)} generators=0")
    for error in errors:
        print(error, file=sys.stderr)
    return 0 if not errors else 1


if __name__ == "__main__":
    raise SystemExit(main())
