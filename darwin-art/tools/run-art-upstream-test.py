#!/usr/bin/env python3
"""Run a pinned ART managed test unchanged in interpreter and ARM64 JIT modes."""

from __future__ import annotations

import argparse
from dataclasses import dataclass
import fcntl
import os
from pathlib import Path
import re
import shutil
import shlex
import subprocess
import sys
import tempfile
import zipfile

from art_run_contract import compile_path, evaluate_contract
from art_build_contract import compile_contract
from art_javac_post_contract import compile_contract as compile_javac_post_contract
from art_action_plan import (
    ActionContext as BuildActionContext,
    ActionPlan,
    evaluate_contract as evaluate_build_action_plan,
    execute_plan as execute_build_action_plan,
)
try:
    from process_group import run_process_group
except ModuleNotFoundError:  # Imported by the focused Python unit tests.
    sys.path.insert(0, str(Path(__file__).resolve().parent))
    from process_group import run_process_group


def _contract_has_action(contract: object, kind: str) -> bool:
    """Return whether a typed contract contains an action, without source scans."""
    def visit(actions: object) -> bool:
        if not isinstance(actions, (tuple, list)):
            return False
        for action in actions:
            if getattr(action, "kind", None) == kind:
                return True
            arguments = getattr(action, "arguments", {})
            if isinstance(arguments, dict):
                if visit(arguments.get("then")) or visit(arguments.get("else")):
                    return True
        return False
    actions = getattr(contract, "actions", contract)
    return visit(actions)


def _split_build_plan(actions: object) -> tuple[tuple[object, ...], tuple[object, ...]]:
    """Split evaluated build actions around the first default_build.

    Build scripts are ordered programs: source generation precedes javac, and
    file moves between two default_build calls are observable.  Branch
    conditions have already been evaluated by ActionPlan, so selecting the
    active arm here preserves the target (ART, not JVM) program while letting
    the runner compile sources between the two typed phases.
    """
    if not isinstance(actions, (tuple, list)):
        raise RuntimeError("build ActionPlan has malformed actions")
    prefix: list[object] = []
    for index, action in enumerate(actions):
        kind = getattr(action, "kind", None)
        if kind == "default_build":
            return tuple(prefix), tuple(actions[index:])
        if kind == "branch":
            arguments = getattr(action, "arguments", {})
            if not isinstance(arguments, dict):
                raise RuntimeError("build ActionPlan branch has malformed arguments")
            arm = arguments.get("then") if arguments.get("condition") else arguments.get("else")
            arm_prefix, arm_suffix = _split_build_plan(arm or ())
            prefix.extend(arm_prefix)
            if arm_suffix:
                # The branch condition is a concrete boolean at this point;
                # retaining an unselected arm would execute a different
                # build.  ActionPlan has already made the selection typed.
                return tuple(prefix), tuple((*arm_suffix, *actions[index + 1:]))
            continue
        prefix.append(action)
    return tuple(prefix), ()


def _first_build_kwargs(actions: object) -> dict[str, object]:
    for action in actions if isinstance(actions, (tuple, list)) else ():
        if getattr(action, "kind", None) == "default_build":
            arguments = getattr(action, "arguments", {})
            kwargs = arguments.get("kwargs", {}) if isinstance(arguments, dict) else {}
            if isinstance(kwargs, dict):
                return dict(kwargs)
    return {}


def _post_requires_argument(contract: object) -> bool:
    def value_has_argument(value: object) -> bool:
        kind = getattr(value, "kind", None)
        if kind in {
                "argument_path", "argument_path_suffix", "argument_basename_path",
                "argument_equals"}:
            return True
        raw = getattr(value, "value", None)
        if (kind == "class_glob" and isinstance(raw, dict)
                and raw.get("root") == "argument1"):
            return True
        if isinstance(raw, dict):
            return any(value_has_argument(item) for item in raw.values())
        if isinstance(raw, (tuple, list)):
            return any(value_has_argument(item) for item in raw)
        return False

    def operations(items: object) -> bool:
        if not isinstance(items, (tuple, list)):
            return False
        for item in items:
            arguments = getattr(item, "arguments", {})
            if isinstance(arguments, dict):
                # Branch predicates can reference $1 even when the branch
                # body only contains ordinary file operations (for example
                # AOSP's multidex javac_post scripts).  Treat the predicate
                # as part of the contract's argument requirements so the
                # caller supplies the logical classes directory.
                if value_has_argument(arguments.get("condition")):
                    return True
                if any(value_has_argument(value) for value in arguments.values()):
                    return True
                if operations(arguments.get("body")) or operations(arguments.get("then")) \
                        or operations(arguments.get("else")):
                    return True
        return False
    return operations(getattr(contract, "operations", ()))


def _contract_is_echo_only(contract: object) -> bool:
    """Identify a typed run contract that needs no VM or managed input."""
    if contract is None or not getattr(contract, "supported", False):
        return False
    capabilities = getattr(contract, "capabilities", set())
    return ("ctx.echo" in capabilities and
            set(capabilities) <= {"ctx.echo", "run_function"})


def command(arguments: list[str], *, env: dict[str, str] | None = None,
            stdout=None, stderr=None, timeout: int = 120,
            cwd: Path | None = None) -> None:
    run_process_group(arguments, check=True, env=env, stdout=stdout,
                      stderr=stderr, timeout=timeout, cwd=cwd)


def expand_aosp_test_args(test_args: tuple[str, ...]) -> tuple[str, ...]:
    """Apply the shell word boundary used by AOSP ``default_run``.

    AOSP builds its command as a shell string (``ARGS += f" {arg}"``), so a
    ``test_args`` entry may contain several argv words. Keeping each entry as
    one host argv element changes that contract; parse the complete assembled
    argument suffix with POSIX shell quoting, without performing expansion or
    execution.
    """
    if not isinstance(test_args, tuple) or not all(
            isinstance(value, str) for value in test_args):
        raise RuntimeError("AOSP test_args must be a tuple of strings")
    try:
        return tuple(shlex.split(" ".join(test_args), comments=False, posix=True))
    except ValueError as error:
        raise RuntimeError(f"invalid AOSP test_args shell syntax: {error}") from error


def expand_aosp_runtime_options(runtime_options: tuple[str, ...]) -> tuple[str, ...]:
    """Apply AOSP's shell word boundary to ``runtime_option`` entries.

    ``default_run`` appends each runtime option to a shell command string.
    Consequently an entry such as ``-Xcompiler-option --compiler-filter=verify``
    contributes two dalvikvm argv words, even though it is one Python list
    element in ``run.py``. Preserve that boundary without executing shell
    syntax or performing expansion here; callers resolve typed environment
    references before invoking this helper.
    """
    if not isinstance(runtime_options, tuple) or not all(
            isinstance(value, str) for value in runtime_options):
        raise RuntimeError("AOSP runtime_options must be a tuple of strings")
    try:
        return tuple(shlex.split(" ".join(runtime_options), comments=False, posix=True))
    except ValueError as error:
        raise RuntimeError(
            f"invalid AOSP runtime_option shell syntax: {error}") from error


def write_aligned_stored_zip(output: Path,
                             entries: list[tuple[str, Path]]) -> None:
    """Write the stored DEX container shape expected by ART's direct mmap.

    AOSP's zip-producing tools 4-byte-align stored entries.  Python's
    ``ZipFile`` does not provide that contract, and an unaligned classes.dex
    makes ART extract it instead of mapping the source container.  Keep this
    helper input-agnostic: callers provide the output entries, with no test
    name or fixture-specific behavior.
    """
    with zipfile.ZipFile(output, "w") as archive:
        for filename, source in entries:
            info = zipfile.ZipInfo(filename)
            info.compress_type = zipfile.ZIP_STORED
            local_header_size = 30 + len(filename.encode("utf-8"))
            current_offset = archive.fp.tell()
            remainder = (current_offset + local_header_size) % 4
            if remainder:
                # Extra fields are TLV encoded. Include at least one complete
                # field, while choosing its total size to reach the boundary.
                extra_size = 4 + (4 - remainder)
                info.extra = (b"\x00\x00" + (extra_size - 4).to_bytes(2, "little")
                              + b"\x00" * (extra_size - 4))
            archive.writestr(info, source.read_bytes())


def write_prebuilt_test_jar(output: Path, source: Path,
                            support_dex: Path) -> None:
    """Copy a prebuilt ART jar and add the generic launcher support DEX.

    Prebuilt corpus jars can intentionally contain malformed or odd-sized DEX
    payloads.  Rebuild only the ZIP container, copying each original entry's
    bytes and metadata unchanged; append support as the next multidex entry so
    the runtime's harness classes remain available without asking D8 to parse
    the fixture.
    """
    with zipfile.ZipFile(source) as source_archive, zipfile.ZipFile(output, "w") as archive:
        entries = source_archive.infolist()
        for info in entries:
            archive.writestr(info, source_archive.read(info))
        dex_indices = [
            int(match.group(1)) for info in entries
            if (match := re.fullmatch(r"classes([2-9][0-9]*)\.dex", info.filename))
        ]
        next_index = max(dex_indices, default=1) + 1
        info = zipfile.ZipInfo(f"classes{next_index}.dex")
        info.compress_type = zipfile.ZIP_STORED
        local_header_size = 30 + len(info.filename.encode("utf-8"))
        remainder = (archive.fp.tell() + local_header_size) % 4
        if remainder:
            extra_size = 4 + (4 - remainder)
            info.extra = (b"\x00\x00" + (extra_size - 4).to_bytes(2, "little")
                          + b"\x00" * (extra_size - 4))
        archive.writestr(info, support_dex.read_bytes())


@dataclass(frozen=True)
class RunInvocation:
    runtime_options: tuple[str, ...] = ()
    android_runtime_options: tuple[str, ...] = ()
    test_args: tuple[str, ...] = ()
    test_libraries: tuple[str, ...] = ()
    compiler_options: tuple[str, ...] = ()
    compiler_only_options: tuple[str, ...] = ()
    expected_exit_code: int = 0
    main_class: str | None = None
    add_libdir_argument: bool = False
    profile: bool = False
    app_image: bool = True
    prebuild: bool = True
    jit: bool = False
    jvmti: bool = False
    zygote: bool = False
    android_log_tags: str | None = None
    vdex: bool = False
    vdex_filter: str | None = None
    dex2oat_dm: bool = False
    runtime_dm: bool = False
    secondary: bool = False
    secondary_app_image: bool = True
    secondary_compilation: bool = True
    secondary_class_loader_context: str | None = None
    verify: bool = True
    verify_soft_fail: bool = False
    image: bool = True
    relocate: bool = False
    sync: bool = False
    lib: str | None = None
    diff_min_log_tag: str = "E"
    dex2oat_timeout: int = 300
    dex2oat_rt_timeout: int = 360


def invocation_from_action(action: dict[str, object]) -> RunInvocation:
    """Materialize one evaluated default_run action without lossy merging."""
    snapshot = action.get("args_snapshot", {})
    if not isinstance(snapshot, dict):
        raise RuntimeError("ContractIR default_run action has no args snapshot")
    values: dict[str, object] = {
        "runtime_options": tuple(snapshot.get("runtime_option", ())),
        "android_runtime_options": tuple(snapshot.get("android_runtime_option", ())),
        "test_args": tuple(snapshot.get("test_args", ())),
        "test_libraries": tuple(snapshot.get("testlib", ())),
        "compiler_options": tuple(snapshot.get("Xcompiler_option", ())),
        "compiler_only_options": tuple(snapshot.get("compiler_only_option", ())),
    }
    list_fields = {
        "runtime_option": "runtime_options",
        "android_runtime_option": "android_runtime_options",
        "test_args": "test_args",
        "testlib": "test_libraries",
        "Xcompiler_option": "compiler_options",
        "compiler_only_option": "compiler_only_options",
    }
    scalar_fields = {
        "expected_exit_code": "expected_exit_code",
        "main": "main_class",
        "add_libdir_argument": "add_libdir_argument",
        "profile": "profile",
        "app_image": "app_image",
        "prebuild": "prebuild",
        "jit": "jit",
        "jvmti": "jvmti",
        "zygote": "zygote",
        "android_log_tags": "android_log_tags",
        "vdex": "vdex",
        "vdex_filter": "vdex_filter",
        "dex2oat_dm": "dex2oat_dm",
        "runtime_dm": "runtime_dm",
        "secondary": "secondary",
        "secondary_app_image": "secondary_app_image",
        "secondary_compilation": "secondary_compilation",
        "secondary_class_loader_context": "secondary_class_loader_context",
        "verify": "verify",
        "verify_soft_fail": "verify_soft_fail",
        "image": "image",
        "relocate": "relocate",
        "sync": "sync",
        "lib": "lib",
        "diff_min_log_tag": "diff_min_log_tag",
        "dex2oat_timeout": "dex2oat_timeout",
        "dex2oat_rt_timeout": "dex2oat_rt_timeout",
    }
    for field, default in RunInvocation.__dataclass_fields__.items():
        if field not in values:
            values[field] = default.default
    for field, value in snapshot.items():
        if field in list_fields:
            if not isinstance(value, (list, tuple)) or not all(isinstance(item, str) for item in value):
                raise RuntimeError(f"default_run snapshot field {field} is not a string list")
            values[list_fields[field]] = tuple(value)
        elif field in scalar_fields:
            values[scalar_fields[field]] = value
    for item in action.get("kwargs", []):
        if not isinstance(item, dict):
            raise RuntimeError("default_run action contains malformed keyword")
        name = item.get("name")
        value = item.get("value")
        if name in list_fields:
            if not isinstance(value, (list, tuple)) or not all(isinstance(entry, str) for entry in value):
                raise RuntimeError(f"default_run keyword {name} is not a string list")
            values[list_fields[name]] = tuple(values[list_fields[name]]) + tuple(value)
        elif name in scalar_fields:
            values[scalar_fields[name]] = value
        else:
            raise RuntimeError(f"unsupported default_run keyword in ActionPlan: {name}")
    for field in ("runtime_options", "android_runtime_options", "test_args", "test_libraries", "compiler_options", "compiler_only_options"):
        if not isinstance(values[field], tuple):
            values[field] = tuple(values[field])
    return RunInvocation(**values)


def plan_invocations(plan: object) -> list[RunInvocation]:
    actions = getattr(plan, "actions", None)
    if not isinstance(actions, list):
        raise RuntimeError("run.py ActionPlan has no ordered actions")
    invocations = [
        invocation_from_action(action)
        for action in actions
        if isinstance(action, dict) and action.get("kind") == "default_run"
    ]
    return invocations or [RunInvocation()]


def implicit_default_invocation(variant: str, *, require_aot: bool = False) -> RunInvocation:
    """Reproduce default_run.py's compiler policy when run.py is absent."""
    compiler_options: tuple[str, ...] = ()
    if variant in {"interpreter", "jit"}:
        # AOSP's default_run.py pairs both explicit execution modes with a
        # verify-only prebuild.  Keeping a speed-compiled application here
        # masks background-JIT behavior: hot methods continue to execute their
        # AOT entrypoints and never become JIT candidates.
        compiler_options = (
                    "--compiler-filter=speed" if require_aot
            else "--compiler-filter=verify",
        )
    return RunInvocation(compiler_options=compiler_options)


def run_contract_context(test_name: str, temporary: Path, native_dir: Path,
                         stdout_file: Path, stderr_file: Path,
                         expected_stdout: Path) -> dict[str, object]:
    environment = dict(os.environ)
    environment.update({
        "DEX_LOCATION": str(temporary),
        "TEST_NAME": test_name,
        "ART_TEST_ON_VM": "1",
        "ANDROID_LOG_TAGS": environment.get("ANDROID_LOG_TAGS", "*:e"),
    })
    return {
        "args": {
            "jvm": False,
            "host": True,
            "prebuild": True,
            "relocate": False,
            "jvmti_redefine_stress": False,
            "switch_interpreter": False,
            "O": True,
            "runtime_option": [
                "-Xopaque-jni-ids:true",
                f"-Djava.library.path=/system/lib64:{native_dir}",
            ],
            "android_runtime_option": [],
        "test_args": [],
            "Xcompiler_option": [],
            "compiler_only_option": [],
            "testlib": ["arttest"],
            "stdout_file": str(stdout_file),
            "stderr_file": str(stderr_file),
        },
        "env": environment,
        "expected_stdout": str(expected_stdout),
        "path_root": str(temporary),
    }


def evaluate_run_plan(contract: object, context: dict[str, object],
                      variant: str) -> object:
    """Evaluate the contract, deriving only assertion-required host variants."""
    candidates = [context]
    base_args = context.get("args")
    if isinstance(base_args, dict):
        for overrides in (
                {"prebuild": False},
                {"relocate": True},
                {"prebuild": False, "relocate": True}):
            candidate = dict(context)
            candidate["args"] = {**base_args, **overrides}
            candidates.append(candidate)
    plans = [evaluate_contract(contract, candidate, variant) for candidate in candidates]
    for plan in plans:
        if getattr(plan, "supported", False):
            return plan
    return plans[0]


def _typed_sed_pattern(pattern: str) -> re.Pattern[bytes]:
    return re.compile(pattern.encode())


def _typed_sed_regex(pattern: str, extended: bool) -> re.Pattern[bytes]:
    r"""Compile a sed BRE/ERE pattern using Python's regex dialect.

    The AOSP metrics post-actions use BRE capture syntax (``\(...\)``),
    whereas Python requires unescaped parentheses for a capture group.  Only
    the BRE operators whose meaning differs here are normalized; escaped
    literals remain escaped so this does not broaden the typed shell grammar.
    """
    if not extended:
        pattern = re.sub(r"\\([(){}+?|])", r"\1", pattern)
    return _typed_sed_pattern(pattern)


def apply_typed_file_operation(operation: dict[str, object],
                               sandbox_root: Path | None = None) -> None:
    """Apply one ActionPlan file operation without invoking a shell."""
    kind = operation.get("kind")

    if sandbox_root is None:
        configured_root = operation.get("path_root")
        sandbox_root = Path(configured_root) if isinstance(configured_root, str) else None
    if sandbox_root is None:
        raise RuntimeError("typed file action requires an explicit sandbox root")
    sandbox_root = sandbox_root.absolute().resolve()

    def confined(value: object, label: str) -> Path:
        if not isinstance(value, str) or not value:
            raise RuntimeError(f"typed file action has no concrete {label} path")
        candidate = Path(value)
        if not candidate.is_absolute():
            candidate = sandbox_root / candidate
        try:
            resolved = candidate.resolve(strict=False)
            if os.path.commonpath((str(sandbox_root), str(resolved))) != str(sandbox_root):
                raise RuntimeError(f"typed file path escapes sandbox: {candidate}")
        except ValueError as error:
            raise RuntimeError(f"typed file path is on an incompatible filesystem: {candidate}") from error
        except OSError as error:
            raise RuntimeError(f"typed file path cannot be resolved: {candidate}") from error
        return resolved

    def replacement_path(value: object, label: str) -> Path:
        """Confine a path that may currently be an escaping symlink.

        A typed `ln -sf` is allowed to replace the final directory entry, so
        validate its canonical parent without following that final entry.
        Source paths and every other filesystem action retain full canonical
        confinement.
        """
        if not isinstance(value, str) or not value:
            raise RuntimeError(f"typed file action has no concrete {label} path")
        candidate = Path(value)
        if not candidate.is_absolute():
            candidate = sandbox_root / candidate
        if candidate.name in {"", ".", ".."}:
            raise RuntimeError(f"typed replacement path is invalid: {candidate}")
        try:
            parent = candidate.parent.resolve(strict=False)
            if os.path.commonpath((str(sandbox_root), str(parent))) != str(sandbox_root):
                raise RuntimeError(f"typed file path escapes sandbox: {candidate}")
        except ValueError as error:
            raise RuntimeError(
                f"typed file path is on an incompatible filesystem: {candidate}") from error
        except OSError as error:
            raise RuntimeError(f"typed file path cannot be resolved: {candidate}") from error
        return parent / candidate.name

    def atomic_write(target: Path, data: bytes) -> None:
        target.parent.mkdir(parents=True, exist_ok=True)
        temporary_output: Path | None = None
        try:
            with tempfile.NamedTemporaryFile(dir=target.parent, prefix=f".{target.name}.",
                                             delete=False) as stream:
                temporary_output = Path(stream.name)
                stream.write(data)
                stream.flush()
                os.fsync(stream.fileno())
            os.replace(temporary_output, target)
            temporary_output = None
        finally:
            if temporary_output is not None:
                temporary_output.unlink(missing_ok=True)

    def path(name: str) -> Path:
        return confined(operation.get(name), name)

    if kind == "print":
        path("file").read_bytes()
        return
    if kind == "touch":
        for value in operation.get("files", []):
            if not isinstance(value, str):
                raise RuntimeError("typed touch action has a non-string path")
            target = confined(value, "touch")
            target.parent.mkdir(parents=True, exist_ok=True)
            target.touch()
        return
    if kind == "truncate":
        target = path("file")
        atomic_write(target, b"")
        return
    if kind == "move":
        path("source").replace(path("destination"))
        return
    if kind == "symlink":
        destination_value = operation.get("destination")
        destination_candidate = (
            Path(destination_value) if isinstance(destination_value, str) else None
        )
        if destination_candidate is not None and not destination_candidate.is_absolute():
            destination_candidate = sandbox_root / destination_candidate
        target = (
            path("destination")
            if destination_candidate is not None and destination_candidate.is_dir()
            else replacement_path(destination_value, "destination")
        )
        # ``ln -sf SOURCE DIRECTORY/.`` creates DIRECTORY/basename(SOURCE),
        # while the shell spelling is normalized away by the typed path IR.
        # Reconstruct that POSIX ln contract when the destination directory
        # already exists.
        if target.is_dir():
            source = path("source")
            target = target / source.name
        else:
            source = path("source")
        if target.exists() and not target.is_symlink() and not target.is_file():
            raise RuntimeError(f"typed symlink destination is not replaceable: {target}")
        target.unlink(missing_ok=True)
        target.symlink_to(source)
        return
    if kind in {"head", "tail"}:
        source = path("file")
        output = path("output")
        lines = source.read_bytes().splitlines(keepends=True)
        count = operation.get("count")
        if isinstance(count, dict) and count.get("kind") == "line_count":
            count_file = confined(count.get("file"), "line-count")
            count = len(count_file.read_bytes().splitlines())
        if isinstance(count, bool) or not isinstance(count, int) or count < 0:
            raise RuntimeError("typed head/tail action has no concrete count")
        atomic_write(output, b"".join(lines[:count] if kind == "head" else lines[-count:] if count else []))
        return
    if kind == "grep_filter":
        patterns_path = path("exclude_patterns")
        source = path("file")
        output = path("output")
        patterns = [line for line in patterns_path.read_bytes().splitlines() if line]
        kept: list[bytes] = []
        for line in source.read_bytes().splitlines(keepends=True):
            if not any(re.search(pattern, line) for pattern in patterns):
                kept.append(line)
        atomic_write(output, b"".join(kept))
        return
    if kind == "sed_filter":
        target = path("file")
        lines = target.read_bytes().splitlines(keepends=True)
        for script in operation.get("scripts", []):
            if not isinstance(script, dict):
                raise RuntimeError("malformed typed sed script")
            script_kind = script.get("kind")
            pattern_value = script.get("pattern")
            if not isinstance(pattern_value, str):
                raise RuntimeError("typed sed script has no concrete pattern")
            pattern = _typed_sed_regex(
                pattern_value, bool(operation.get("extended_regex")))
            if script_kind == "delete_matching":
                lines = [line for line in lines if pattern.search(line) is None]
            elif script_kind == "keep_matching":
                lines = [line for line in lines if pattern.search(line) is not None]
            elif script_kind == "delete_matching_with_following_lines":
                following = script.get("following_lines")
                if not isinstance(following, int) or following < 0:
                    raise RuntimeError("typed sed range has invalid line count")
                kept = []
                skip = 0
                for line in lines:
                    if skip:
                        skip -= 1
                    elif pattern.search(line) is not None:
                        skip = following
                    else:
                        kept.append(line)
                lines = kept
            elif script_kind == "replace":
                replacement = script.get("replacement")
                flags = script.get("flags", "")
                if not isinstance(replacement, str) or not isinstance(flags, str):
                    raise RuntimeError("typed sed replacement is malformed")
                count = 0 if "g" in flags else 1
                transformed: list[bytes] = []
                for line in lines:
                    updated, substitutions = pattern.subn(replacement.encode(), line, count=count)
                    if operation.get("print_only_matches") and substitutions == 0:
                        continue
                    transformed.append(updated)
                lines = transformed
            else:
                raise RuntimeError(f"unknown typed sed operation: {script_kind}")
        atomic_write(target, b"".join(lines))
        return
    raise RuntimeError(f"unknown typed file operation: {kind}")


def apply_typed_shell_action(action: dict[str, object],
                             sandbox_root: Path | None = None) -> None:
    operations = action.get("operations")
    if not isinstance(operations, list):
        raise RuntimeError("shell ActionPlan has no operations")
    if sandbox_root is None:
        configured_root = action.get("path_root")
        sandbox_root = Path(configured_root) if isinstance(configured_root, str) else None
    try:
        for operation in operations:
            if not isinstance(operation, dict):
                raise RuntimeError("shell ActionPlan contains malformed operation")
            apply_typed_file_operation(operation, sandbox_root)
    except RuntimeError:
        # Contract and confinement violations are security errors and are
        # never softened by ``check=False``.
        raise
    except (OSError, re.error) as error:
        if action.get("check", True):
            span = action.get("span")
            if isinstance(span, dict):
                source = span.get("path", "<run.py>")
                line = span.get("line", 1)
                column = span.get("column", 0)
                raise RuntimeError(
                    f"{source}:{line}:{int(column) + 1}: typed shell action failed: {error}") from error
            raise RuntimeError(f"typed shell action failed: {error}") from error


def append_new_host_log_bytes(host_log: Path, cursor: int,
                              destination: bytearray) -> int:
    """Append only records added to a host log since the previous invocation.

    The Darwin log adapter intentionally uses the inherited host stream, while
    AOSP's runner presents Android logcat as ``stderr_file``.  Keeping the
    cursor at the runner boundary preserves ordered multi-invocation output
    and avoids duplicating records when a post-action runs between invocations.
    """
    data = host_log.read_bytes()
    if cursor < 0 or cursor > len(data):
        cursor = 0
    destination.extend(data[cursor:])
    return len(data)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("test")
    parser.add_argument("--root", type=Path, default=Path(__file__).resolve().parent.parent)
    parser.add_argument("--keep", action="store_true")
    parser.add_argument(
        "--boot-image", type=Path,
        help="use an alternate complete ART boot image set (boot.art and siblings)")
    parser.add_argument(
        "--gcstress", action="store_true",
        help="apply AOSP run-test's gcstress runtime and heap contract")
    args = parser.parse_args()
    root = args.root.resolve()
    sdk_root = Path(os.environ.get("ANDROID_HOME", Path.home() / "Library/Android/sdk"))
    ndk_root = Path(os.environ.get(
        "ANDROID_NDK_ROOT", sdk_root / "ndk/28.2.13676358"))
    ndk_include = (
        ndk_root
        / "toolchains/llvm/prebuilt/darwin-x86_64/sysroot/usr/include"
    )
    test = root / "_aosp/art/test" / args.test
    run_script = test / "run.py"
    has_native_source = any(test.rglob("*.cc")) or any(test.rglob("*.c"))
    uses_dexter_slicer = any(
        b'"slicer/' in source.read_bytes()
        for source in (*test.rglob("*.cc"), *test.rglob("*.c"))
    )
    java_uses_argument0 = any(
        b"args[0]" in source.read_bytes()
        for source in test.rglob("*.java") if source.is_file())
    # AOSP's run-test command line may supply test-specific argv. This table is
    # launcher metadata, not build-script interpretation; the run.py contract
    # still owns the invocation order and mutable arguments.
    upstream_argument0 = {
        "826-infinite-loop": "run-test",
        "088-monitor-verification": "arttest",
        "2245-checker-smali-instance-of-comparison": "arttest",
        "543-env-long-ref": "arttest",
        "563-checker-fakestring": "arttest",
        "575-checker-string-init-alias": "arttest",
        "686-get-this": "arttest",
        # run-test passes the global libarttest basename to Main. This case
        # has no per-directory native source because it exercises loading the
        # shared ART test library itself through the boot ClassLoader.
        "150-loadlibrary": "arttest",
    }.get(args.test, "arttest" if has_native_source or java_uses_argument0 else None)
    run_contract = compile_path(run_script) if run_script.is_file() else None
    build_script = test / "build.py"
    build_contract = (
        compile_contract(build_script.read_text(encoding="utf-8"), str(build_script))
        if build_script.is_file() else None
    )
    echo_only_contract = _contract_is_echo_only(run_contract)
    for required in (test / "expected-stdout.txt", test / "expected-stderr.txt"):
        if not required.exists():
            raise SystemExit(f"unsupported ART test layout: missing {required}")
    generate_sources = test / "generate-sources"
    generated_build = (
        generate_sources.is_file()
        and build_contract is not None
        and _contract_has_action(build_contract, "generate_sources")
    )
    has_java = any(test.rglob("*.java")) or generated_build
    has_smali = any((test / "smali").rglob("*.smali")) or generated_build
    # run_test_build.py selects Jasmin for the host JVM and Smali for ART in
    # malformed-bytecode cases. Respect an explicit target-side disable so D8
    # never validates and rejects a classfile that AOSP intentionally emits as
    # invalid DEX through Smali.
    has_jasmin = any((test / "jasmin").rglob("*.j"))
    has_prebuilt_dex = (test / "classes.dex").is_file()
    has_prebuilt_jar = (test / f"{args.test}.jar").is_file()
    if (not has_java and not has_smali and not has_jasmin and
            not has_prebuilt_dex and not has_prebuilt_jar and
            not echo_only_contract):
        raise SystemExit("test has no managed source or prebuilt DEX input")

    sdk = Path.home() / "Library/Android/sdk"
    platform = sdk / "platforms/android-36"
    # core-libart is a pinned DEX-only boot input. Keep its javac view in a
    # separate compiler companion generated by the generic framework/libcore
    # build step; never synthesize hidden APIs from an individual test.
    libcore_compiler_api = (
        root / "_build/android16-libcore-compiler-api/core-libart-compiler.jar"
    )
    if not libcore_compiler_api.is_file():
        command([str(root / "tools/build-android16-framework-compat.sh")])
    if not libcore_compiler_api.is_file():
        raise SystemExit(
            f"missing generic libcore compiler classpath: {libcore_compiler_api}"
        )
    temporary = Path(tempfile.mkdtemp(prefix=f"darwin-art-{args.test}-"))
    classes = temporary / "classes"
    dex = temporary / "dex"
    # AOSP run-test stages both DEX_LOCATION and the emulated app-private
    # filesystem inside one disposable process sandbox.
    private_data = temporary / "private-seed"
    android_tmp = private_data / "local/tmp"
    classes.mkdir()
    dex.mkdir()
    private_data.mkdir()
    android_tmp.mkdir(parents=True)
    try:
        if run_contract is not None:
            bootstrap_context = run_contract_context(
                args.test, temporary, temporary / "native",
                temporary / "contract.stdout", temporary / "contract.stderr",
                temporary / "expected-stdout.txt")
            bootstrap_plan = evaluate_run_plan(
                run_contract, bootstrap_context, "host")
            if not bootstrap_plan.supported:
                raise RuntimeError(
                    f"run.py ActionPlan unsupported: {bootstrap_plan.as_dict()}")
            upstream_run_invocations = plan_invocations(bootstrap_plan)
        else:
            upstream_run_invocations = [RunInvocation()]
        # AOSP's non-debuggable known-failure entries are represented by the
        # typed run.py compiler option when one exists.  Older tests have no
        # run.py, so derive the same requirement from the native stack/vreg or
        # full-fragment-deoptimization capability they exercise.  This keeps
        # compiler policy tied to the source contract rather than a test-name
        # allowlist.
        test_capability_sources = tuple(
            source for source in test.rglob("*")
            if source.is_file() and source.suffix in {".cc", ".c", ".java"}
        )
        test_capability_text = b"\n".join(
            source.read_bytes() for source in test_capability_sources)
        # Every JVMTI redefinition test carries the shared Redefinition.java
        # helper, whose declarations intentionally mention all operation
        # kinds.  Capability selection must inspect the test's own call sites
        # so that the typed source contract remains discriminating.
        test_behavior_sources = tuple(
            source for source in test_capability_sources
            if source.name != "Redefinition.java")
        test_behavior_text = b"\n".join(
            source.read_bytes() for source in test_behavior_sources)
        upstream_debuggable = (
            any(
                "--debuggable" in (*invocation.compiler_options,
                                    *invocation.compiler_only_options)
                for invocation in upstream_run_invocations
            )
            or b"GetVReg(" in test_capability_text
            or b"GetVRegPair(" in test_capability_text
            or b"deoptimizeAll(" in test_capability_text
        )
        native_bridge_libraries = tuple(dict.fromkeys(
            option.split("=", 1)[1]
            for invocation in upstream_run_invocations
            for option in (*invocation.runtime_options,
                           *invocation.android_runtime_options)
            if option.startswith("-XX:NativeBridge=") and
            option.split("=", 1)[1]
        ))
        native_bridge_source = test / "nativebridge.cc"
        uses_native_bridge = bool(native_bridge_libraries) and native_bridge_source.is_file()
        upstream_agentpath = any(
            option.startswith("-agentpath:")
            for invocation in upstream_run_invocations
            for option in (*invocation.runtime_options,
                           *invocation.android_runtime_options))
        upstream_jvmti = upstream_agentpath or any(
            invocation.jvmti for invocation in upstream_run_invocations)
        # Some unchanged ART tests attach a JVMTI agent from Java via
        # VMDebug.attachAgent rather than requesting the default_run jvmti
        # variant.  Their typed test arguments carry the same agent contract;
        # detect it generically so the pinned libtiagent is built and placed
        # in the native fixture search path without enabling startup JVMTI.
        upstream_agent_attach = any(
            argument.startswith("agent:")
            for invocation in upstream_run_invocations
            for argument in invocation.test_args)
        deferred_zygote_agent = any(
            source.name == "native-wait.cc"
            and b"SetAsZygoteChild" in source.read_bytes()
            for source in test.rglob("*.cc"))
        native_attach_agent = any(
            source.name == "native_attach_agent.cc"
            and b"AttachAgent(" in source.read_bytes()
            for source in test.rglob("*.cc"))
        trace_v2_native = any(
            source.name == "dump_trace.cc"
            and b"TRACE_OUTPUT_V2_FLAG" in source.read_bytes()
            for source in test.rglob("*.cc"))
        inherits_pop_frame_base = b"extends Test1953" in test_capability_text
        hello_tiagent_capability = b"Test901HelloTi" in test_capability_text
        common_redefine_capability = (
            b"doCommonClassRedefinition" in test_behavior_text)
        common_retransform_capability = (
            b"enableCommonRetransformation" in test_behavior_text)
        common_transform_capability = (
            b"popTransformationFor" in test_behavior_text)
        cfi_contract = any(
            "--test-local" in invocation.test_args
            and "--test-remote" in invocation.test_args
            for invocation in upstream_run_invocations)
        metrics_contract = any(
            option.startswith("-Xmetrics-")
            for invocation in upstream_run_invocations
            for option in (*invocation.runtime_options,
                           *invocation.android_runtime_options))
        swappable_jni_contract = any(
            option == "-Xopaque-jni-ids:swapable"
            for invocation in upstream_run_invocations
            for option in (*invocation.runtime_options,
                           *invocation.android_runtime_options))
        upstream_profile = any(
            invocation.profile for invocation in upstream_run_invocations)
        upstream_vdex = any(
            invocation.vdex for invocation in upstream_run_invocations)
        upstream_target_sdk = next(
            (option.split(":", 1)[1]
             for invocation in upstream_run_invocations
             for option in (*invocation.runtime_options,
                            *invocation.android_runtime_options)
             if option.startswith("-Xtarget-sdk-version:") and ":" in option),
            None)
        finalizer_timeout = next(
            (option.split("=", 1)[1]
             for invocation in upstream_run_invocations
             for option in (*invocation.runtime_options,
                            *invocation.android_runtime_options)
             if option.startswith("-XX:FinalizerTimeoutMs=") and "=" in option),
            None)
        upstream_finalizer_timeout = finalizer_timeout
        source_test_root = test
        generator_environment: dict[str, str] | None = None
        generated_build_top: Path | None = None
        if generated_build:
            command([str(root / "tools/sync-android16-art-testgen.sh")])
            generator_inputs = [
                source for source in source_test_root.rglob("*")
                if source.is_file()
            ]
            generator_uses_asm = any(
                b"prebuilts/misc/common/asm/" in source.read_bytes()
                for source in generator_inputs
            )
            generator_uses_smali = any(
                b"${SMALI}" in source.read_bytes()
                for source in generator_inputs
            )
            if generator_uses_asm:
                command([str(root / "tools/sync-android16-asm.sh")])
            if generator_uses_smali:
                command([str(root / "tools/sync-android16-smali.sh")])
            command([str(root / "tools/sync-android16-r8.sh")])
            source_test_root = temporary / "generated-test"
            shutil.copytree(
                test, source_test_root, ignore_dangling_symlinks=True)
            # The ART test checkout records a few source links into its
            # sibling platform/libcore project. Our pinned checkout stores
            # that independently as _aosp/libcore-full; materialize only the
            # linked source into the disposable test tree before running the
            # untouched generator.
            art_libcore = root / "_aosp/libcore"
            pinned_libcore = root / "_aosp/libcore-full"
            for linked_source in test.rglob("*"):
                if not linked_source.is_symlink():
                    continue
                if linked_source.exists():
                    pinned_source = linked_source.resolve()
                else:
                    unresolved = (linked_source.parent /
                                  os.readlink(linked_source)).resolve()
                    try:
                        libcore_relative = unresolved.relative_to(art_libcore)
                    except ValueError as error:
                        raise RuntimeError(
                            f"unsupported dangling AOSP test source link: {linked_source}"
                        ) from error
                    pinned_source = pinned_libcore / libcore_relative
                if not pinned_source.is_file():
                    raise RuntimeError(
                        f"missing pinned source for AOSP test link: {pinned_source}")
                copied_source = source_test_root / linked_source.relative_to(test)
                copied_source.parent.mkdir(parents=True, exist_ok=True)
                shutil.copyfile(pinned_source, copied_source)
            generated_build_top = temporary / "generated-build-top"
            generated_build_top.mkdir()
            (generated_build_top / "art").symlink_to(root / "_aosp/art")
            copyright_templates = (
                generated_build_top / "development/docs/copyright-templates")
            copyright_templates.mkdir(parents=True)
            shutil.copyfile(
                root / "_prebuilt/android-16/testgen/java.txt",
                copyright_templates / "java.txt",
            )
            if generator_uses_asm:
                asm_directory = (
                    generated_build_top / "prebuilts/misc/common/asm")
                asm_directory.mkdir(parents=True)
                shutil.copyfile(
                    root / "_prebuilt/android-16/tools/asm-9.6.jar",
                    asm_directory / "asm-9.6.jar")
            generator_environment = os.environ.copy()
            generator_environment["ANDROID_BUILD_TOP"] = str(generated_build_top)
            generator_environment.setdefault("JAVAC", "javac")
            generator_environment.setdefault("JAVAC_ARGS", "")
            generator_environment.setdefault("JAVA", "java")
            generator_environment.setdefault(
                "D8", str(root / "tools/d8-jar-compat.sh"))
            generator_environment.setdefault(
                "SOONG_ZIP", str(root / "tools/soong-zip-jar-compat.sh"))
            generator_environment.setdefault(
                "SMALI", str(root / "tools/smali-cli-compat.sh"))
            generator_environment.setdefault("TEST_NAME", args.test)
        # Compile and evaluate the build contract before discovering sources.
        # Execute only the typed prefix here (normally generate-sources); the
        # suffix is resumed after javac has produced classes so default_build
        # and its ordered post-actions retain AOSP's lifecycle.
        build_plan_suffix: object = ()
        build_settings: dict[str, object] = {}
        if build_contract is not None:
            build_root = source_test_root if generated_build else temporary
            build_context = BuildActionContext(
                test_root=build_root,
                toolchain_root=root,
                jvm=False,
                mode="art",
                environment=generator_environment or os.environ,
                tools={"soong_zip": root / "tools/soong-zip-jar-compat.sh"},
            )
            build_plan = evaluate_build_action_plan(build_contract, build_context)
            if (not getattr(build_plan, "actions", None)
                    and not echo_only_contract):
                raise RuntimeError("build.py ActionPlan is empty")
            build_prefix, build_plan_suffix = _split_build_plan(
                getattr(build_plan, "actions", ()))
            if build_prefix:
                execute_build_action_plan(
                    ActionPlan(build_plan.source, tuple(build_prefix)), build_context)
            build_settings = _first_build_kwargs(build_plan_suffix)

        named_api_levels = {
            "default-methods": "24", "parameter-annotations": "25",
            "agents": "26", "method-handles": "26", "var-handles": "28",
            "const-method-type": "28",
        }
        upstream_api_value = build_settings.get("api_level", 26)
        if isinstance(upstream_api_value, int):
            upstream_api_level = str(upstream_api_value)
        elif isinstance(upstream_api_value, str):
            upstream_api_level = named_api_levels.get(upstream_api_value, upstream_api_value)
        else:
            raise RuntimeError("typed build api_level is not a string or integer")
        if not upstream_api_level.isdigit():
            raise RuntimeError(f"unknown typed build API level: {upstream_api_level}")
        d8_flags = build_settings.get("d8_flags", ())
        if not isinstance(d8_flags, (tuple, list)) or not all(
                isinstance(flag, str) for flag in d8_flags):
            raise RuntimeError("typed build d8_flags is not a string list")
        # AOSP's d8 launcher treats ``-J<arg>`` as a JVM argument, while the
        # direct R8 invocation below bypasses that launcher. Keep the build
        # contract unchanged and route the two argument classes explicitly.
        d8_jvm_flags = ["-" + flag[2:] for flag in d8_flags
                        if flag.startswith("-J") and len(flag) > 2]
        d8_tool_flags = [flag for flag in d8_flags if not flag.startswith("-J")]
        upstream_d8_dex_container = build_settings.get("d8_dex_container", True) is not False
        upstream_d8_intermediate = "--intermediate" in d8_flags
        upstream_no_desugar = build_settings.get("use_desugar", True) is False
        upstream_use_jasmin = build_settings.get("use_jasmin", True) is not False
        upstream_use_smali = build_settings.get("use_smali", True) is not False
        upstream_javac_source = str(build_settings.get("javac_source_arg", "8"))
        upstream_javac_target = str(build_settings.get("javac_target_arg", "8"))
        if not upstream_javac_source.isdigit() or not upstream_javac_target.isdigit():
            raise RuntimeError("typed build javac source/target is not numeric")
        support_in_primary_dex = int(upstream_api_level) >= 26
        # AOSP's run-test build places a test's auxiliary resource tree below
        # DEX_LOCATION.  Generated tests build those files in the disposable
        # source tree (and ordinary tests may ship the tree directly), while
        # this runner keeps DEX_LOCATION at the staging root.  Materialize
        # the complete resource tree there before the runtime is launched;
        # this is deliberately independent of any individual test name.
        resources = source_test_root / "res"
        # Typed build actions may create the AOSP resource tree in the
        # runner's staging root (for example a build.py move into res/)
        # rather than under the immutable source directory. Carry either
        # spelling into each per-mode process sandbox.
        if not resources.is_dir():
            built_resources = temporary / "res"
            if built_resources.is_dir():
                resources = built_resources
        if resources.is_dir():
            staged_resources = private_data / "res"
            shutil.copytree(resources, staged_resources, dirs_exist_ok=True)
            # The pinned generators use soong_zip's -j (junk paths) contract
            # for explicit resource archives.  The small host compatibility
            # wrapper used by this runner preserves the input path instead,
            # which leaves entries such as test-jar/classes.dex and changes
            # the archive's observable DEX layout.  Normalize only archives
            # whose non-META-INF entries are all uniquely flattenable; this
            # keeps intentional directory trees and ambiguous archives intact
            # while reproducing the AOSP staging contract generically.
            for archive in sorted(staged_resources.rglob("*")):
                if not archive.is_file() or archive.suffix.lower() not in {
                        ".jar", ".zip"}:
                    continue
                try:
                    with zipfile.ZipFile(archive) as source_archive:
                        entries = source_archive.infolist()
                        payload = [
                            entry for entry in entries
                            if not entry.is_dir()
                            and not entry.filename.startswith("META-INF/")
                        ]
                        payload_names = [entry.filename for entry in payload]
                        flattened_names = [name.rsplit("/", 1)[-1]
                                           for name in payload_names]
                        if (not payload or
                                not all("/" in name for name in payload_names)
                                or len(set(flattened_names)) != len(payload)):
                            continue
                        contents = {
                            entry.filename: source_archive.read(entry)
                            for entry in entries if not entry.is_dir()
                        }
                except (OSError, zipfile.BadZipFile):
                    continue
                temporary_archive = archive.with_name(archive.name + ".flat")
                with zipfile.ZipFile(temporary_archive, "w") as target_archive:
                    for entry in entries:
                        if entry.is_dir():
                            continue
                        original_name = entry.filename
                        entry.filename = (
                            original_name.rsplit("/", 1)[-1]
                            if original_name in payload_names else original_name
                        )
                        target_archive.writestr(entry, contents[original_name])
                temporary_archive.replace(archive)
        if echo_only_contract:
            # 000-nop is a real AOSP harness test: its typed run contract only
            # emits the expected line and intentionally has no DEX/VM input.
            # Execute that contract in both differential modes so it remains a
            # first-class corpus result without fabricating a test artifact.
            expected_stdout = (test / "expected-stdout.txt").read_bytes()
            expected_stderr = (test / "expected-stderr.txt").read_bytes()
            for mode in ("interpreter", "jit"):
                mode_root = temporary / f"{mode}-sandbox"
                mode_root.mkdir()
                stdout_file = mode_root / f"{mode}.stdout"
                stderr_file = mode_root / f"{mode}.stderr"
                mode_context = run_contract_context(
                    args.test, mode_root, mode_root / "native",
                    stdout_file, stderr_file,
                    mode_root / "expected-stdout.txt")
                mode_plan = evaluate_run_plan(run_contract, mode_context, mode)
                if not mode_plan.supported:
                    raise RuntimeError(
                        f"run.py ActionPlan unsupported: {mode_plan.as_dict()}")
                output = bytearray()
                for action in mode_plan.actions:
                    if action.get("kind") != "echo":
                        raise RuntimeError(
                            "echo-only run contract emitted a non-echo action")
                    values = action.get("args", [])
                    if len(values) != 1 or not isinstance(values[0], str):
                        raise RuntimeError(
                            "ctx.echo ActionPlan value is not a string")
                    output.extend(values[0].encode() + b"\n")
                if output != expected_stdout or expected_stderr:
                    raise RuntimeError(
                        f"{mode} output mismatch: stdout={len(output)}/"
                        f"{len(expected_stdout)} stderr=0/{len(expected_stderr)}")
                print(f"ART upstream {args.test}: {mode} expected-output PASS")
            return 0
        source_groups = {
            name: sorted((source_test_root / name).rglob("*.java"))
            for name in (
                "src", "src2", "src-art", "src-multidex", "src-aotex",
                "src-bcpex", "src-ex", "src-ex2",
            )
            if (source_test_root / name).is_dir()
        }
        all_sources = [source for group in source_groups.values() for source in group]
        uses_hprof_conv = any(
            b"hprof-conv" in source.read_bytes() for source in all_sources
        )
        uses_legacy_unsafe = any(
            b"sun.misc.Unsafe" in source.read_bytes() for source in all_sources
        )
        uses_jdk_internal = any(
            b"jdk.internal." in source.read_bytes() for source in all_sources
        )
        command([str(root / "tools/sync-android16-r8.sh")])
        command([str(root / "tools/sync-android16-smali.sh")])
        command([str(root / "tools/sync-android16-jasmin.sh")])
        uses_indy_transformer = (
            (test / "util-src/transformer/IndyTransformer.java").is_file()
            and (test / "javac_post.sh").is_file()
        )
        if uses_indy_transformer:
            command([str(root / "tools/sync-android16-asm.sh")])
        r8 = root / "_prebuilt/android-16/tools/r8.jar"
        smali = root / "_aosp/google-smali/smali/build/libs/smali-3.0.7-dev-fat.jar"
        jasmin = root / "_prebuilt/android-16/tools/jasmin.jar"
        asm = root / "_prebuilt/android-16/tools/asm-9.6.jar"
        if uses_hprof_conv:
            hprof_conv = sdk / "platform-tools/hprof-conv"
            if not hprof_conv.is_file():
                raise RuntimeError(
                    f"Android platform hprof-conv is missing: {hprof_conv}")
            test_bin = temporary / "bin"
            test_bin.mkdir()
            hprof_wrapper = test_bin / "hprof-conv"
            hprof_wrapper.write_text(
                "#!/bin/sh\n"
                "set -eu\n"
                "private_root=${DARWIN_ART_ANDROID_PRIVATE_DATA_ROOT:?}\n"
                "case $1 in /data/*) input=$private_root/${1#/data/};; *) input=$1;; esac\n"
                "case $2 in /data/*) output=$private_root/${2#/data/};; *) output=$2;; esac\n"
                f"if {shlex.quote(str(hprof_conv))} \"$input\" \"$output\" "
                " >\"$private_root/hprof-conv.log\" 2>&1; then\n"
                "  rm -f \"$private_root/hprof-conv.log\"\n"
                "  exit 0\n"
                "else\n"
                "  status=$?\n"
                "  cp \"$input\" \"$private_root/hprof-conv.failed.hprof\"\n"
                "  exit $status\n"
                "fi\n"
            )
            hprof_wrapper.chmod(0o755)

        def javac_api(classpath: list[Path]) -> list[str]:
            extra = list(map(str, classpath))
            compiler_boot_overrides = [
                str(path) for path in classpath
                if path.name in {"hidden-compiler-classes", "core-libart-compiler.jar"}
            ]
            if float(upstream_javac_target) >= 17.0:
                return ["-classpath", ":".join(extra)] if extra else []
            if uses_jdk_internal:
                result = ["-XDignore.symbol.file"]
                # Host JDK modules otherwise win over Android's boot classes,
                # even with -XDignore.symbol.file. Prepend only our signature
                # view to javac's boot search; it is never emitted to app DEX.
                if compiler_boot_overrides:
                    result += ["-Xbootclasspath/p:" + ":".join(compiler_boot_overrides)]
                return result + (["-classpath", ":".join(extra)] if extra else [])
            if uses_legacy_unsafe:
                result = ["-XDignore.symbol.file"]
                if compiler_boot_overrides:
                    result += ["-Xbootclasspath/p:" + ":".join(compiler_boot_overrides)]
                return result + [
                    "-classpath", ":".join([str(platform / "android.jar"), *extra])
                ]
            result = [
                "-bootclasspath",
                ":".join([
                    *compiler_boot_overrides,
                    str(platform / "core-for-system-modules.jar"),
                    str(platform / "android.jar"),
                ]),
            ]
            return result + (["-classpath", ":".join(extra)] if extra else [])

        def compile_java(output: Path, group: list[Path], classpath: list[Path]) -> None:
            if not group:
                return
            output.mkdir(parents=True, exist_ok=True)
            source_arguments = list(map(str, group))
            if sum(map(len, source_arguments)) > 128 * 1024:
                argfile = output.parent / f"{output.name}.javac.args"
                argfile.write_text("".join(
                    f'"{source.replace(chr(92), chr(92) * 2).replace(chr(34), chr(92) + chr(34))}"\n'
                    for source in source_arguments
                ))
                source_arguments = [f"@{argfile}"]
            javac_extra = build_settings.get("javac_args", ())
            if not isinstance(javac_extra, (tuple, list)) or not all(
                    isinstance(value, str) for value in javac_extra):
                raise RuntimeError("typed build javac_args is not a string list")
            configured_classpath = build_settings.get("javac_classpath", ())
            if not isinstance(configured_classpath, (tuple, list)) or not all(
                    isinstance(value, Path) for value in configured_classpath):
                raise RuntimeError("typed build javac_classpath is not typed paths")
            effective_classpath = [
                libcore_compiler_api, *classpath, *configured_classpath
            ]
            command([
                "javac", "-g", "-Xlint:-options", "-implicit:none",
                "-source", upstream_javac_source,
                "-target", upstream_javac_target, "-encoding", "UTF-8",
                *javac_extra, *javac_api(effective_classpath),
                "-d", str(output), *source_arguments,
            ])

        transformer_jar: Path | None = None
        if uses_indy_transformer:
            transformer_classes = temporary / "transformer-classes"
            compile_java(
                transformer_classes,
                sorted((test / "util-src").rglob("*.java")),
                [asm],
            )
            transformer_jar = temporary / "transformer.jar"
            command([
                "jar", "cf", str(transformer_jar), "-C",
                str(transformer_classes), ".",
            ])

        def assemble_jasmin(output: Path, source_root: Path) -> Path | None:
            jasmin_sources = sorted(source_root.rglob("*.j")) \
                if source_root.is_dir() else []
            if not jasmin_sources:
                return None
            output.mkdir(parents=True, exist_ok=True)
            command([
                "java", "-jar", str(jasmin), "-d", str(output),
                *map(str, jasmin_sources),
            ])
            return output

        primary_jasmin_classes = assemble_jasmin(
            temporary / "jasmin-classes", source_test_root / "jasmin") \
            if upstream_use_jasmin else None
        secondary_jasmin_classes = assemble_jasmin(
            temporary / "jasmin-classes2", source_test_root / "jasmin-multidex") \
            if upstream_use_jasmin else None
        jasmin_classpath = [
            value for value in
            (primary_jasmin_classes, secondary_jasmin_classes)
            if value is not None
        ]

        # Preserve the source/DexFile boundaries implemented by AOSP's
        # run_test_build.py. The combined tree is javac symbols only;
        # replacement and auxiliary classes are emitted into their own groups.
        symbol_classes = temporary / "classes-tmp-all"
        symbol_sources = [
            source
            for name in ("src", "src-multidex", "src-aotex", "src-bcpex", "src-ex")
            for source in source_groups.get(name, [])
        ]
        # ART builds src-art against the full platform bootclasspath, not the
        # public SDK stubs. Supply signature-only compiler inputs for hidden
        # boot classes used by this queue; none of these files enters app DEX.
        java_sources_for_capabilities = [
            source for source in source_test_root.rglob("*.java") if source.is_file()
        ]
        java_capability_text = b"\n".join(
            source.read_bytes() for source in java_sources_for_capabilities)
        for marker, capability_source in (
                (b"AnnotatedStackTraceElement", root /
                 "probes/compiler-stubs/dalvik/system/AnnotatedStackTraceElement.java"),
                (b"sun.util.calendar.CalendarUtils", root /
                 "_aosp/libcore-full/ojluni/src/main/java/sun/util/calendar/CalendarUtils.java"),
        ):
            if marker in java_capability_text and capability_source.is_file():
                symbol_sources.append(capability_source)
        hidden_compiler_sources: list[Path] = [
            root / "probes/compiler-stubs/dalvik/system/DexFile.java",
            root / "probes/compiler-stubs/dalvik/system/BaseDexClassLoader.java",
            root / "probes/compiler-stubs/dalvik/system/VMRuntime.java",
            root / "probes/compiler-stubs/dalvik/system/VMDebug.java",
            root / "probes/compiler-stubs/dalvik/system/PathClassLoader.java",
            root / "probes/compiler-stubs/dalvik/system/DelegateLastClassLoader.java",
            root / "probes/compiler-stubs/dalvik/system/ZygoteHooks.java",
            # InputChannel is a hidden framework API absent from the public
            # SDK jar but required by the endpoint/Parcel compatibility smoke.
            root / "probes/compile-stubs/android/view/InputChannel.java",
            root / "probes/compiler-stubs/android/os/Parcelable.java",
            root / "probes/compiler-stubs/android/os/IBinder.java",
            root / "probes/compiler-stubs/android/os/Binder.java",
            root / "probes/compiler-stubs/android/os/Parcel.java",
            root / "probes/compiler-stubs/android/os/ParcelFileDescriptor.java",
            root / "probes/compiler-stubs/libcore/util/EmptyArray.java",
            root / "probes/compiler-stubs/dalvik/annotation/optimization/DeadReferenceSafe.java",
            root / "probes/compiler-stubs/dalvik/annotation/optimization/NeverInline.java",
            root / "probes/compiler-stubs/dalvik/annotation/optimization/ReachabilitySensitive.java",
        ]
        if uses_jdk_internal:
            hidden_compiler_sources.append(
                root / "probes/compiler-stubs/jdk/internal/misc/Unsafe.java"
            )
        if uses_legacy_unsafe:
            hidden_compiler_sources += [
                root / "probes/compiler-stubs/android/compat/annotation/UnsupportedAppUsage.java",
                root / "probes/compiler-stubs/dalvik/annotation/compat/VersionCodes.java",
                root / "_aosp/libcore-full/ojluni/annotations/hiddenapi/sun/misc/Unsafe.java",
            ]
        hidden_capabilities = (
            (b"sun.misc.Cleaner", root / "probes/compiler-stubs/sun/misc/Cleaner.java"),
            (b"NativeAllocationRegistry", root /
             "probes/compiler-stubs/libcore/util/NativeAllocationRegistry.java"),
            (b"com.android.libcore.Flags", root / "probes/compiler-stubs/com/android/libcore/Flags.java"),
            (b"com.android.art.flags.Flags", root / "probes/compiler-stubs/com/android/art/flags/Flags.java"),
            (b"EmulatedStackFrame", root / "probes/compiler-stubs/dalvik/system/EmulatedStackFrame.java"),
            (b"java.lang.invoke.Transformers", root / "probes/compiler-stubs/java/lang/invoke/Transformers.java"),
            (b"dalvik.system.ClassExt", root / "_aosp/libcore-full/libart/src/main/java/dalvik/system/ClassExt.java"),
        )
        for marker, capability_source in hidden_capabilities:
            if marker in java_capability_text and capability_source.is_file():
                hidden_compiler_sources.append(capability_source)
        ddmc_sources = sorted(
            (root / "probes/compiler-stubs/org/apache/harmony/dalvik/ddmc").glob("*.java"))
        if b"org.apache.harmony.dalvik.ddmc" in java_capability_text:
            hidden_compiler_sources.extend(ddmc_sources)
        hidden_compiler_classes = temporary / "hidden-compiler-classes"
        compile_java(hidden_compiler_classes, hidden_compiler_sources, [])
        if len(source_groups) > 1:
            # The symbol pass compiles every source group together before the
            # real per-Dex compilation.  It must see the same compile-only
            # declarations as those later passes (for example
            # dalvik.annotation.optimization.NeverInline in a secondary
            # source group); otherwise javac rejects an otherwise valid
            # multidex test before D8/ART gets a chance to run it.
            symbol_classpath = jasmin_classpath + [hidden_compiler_classes]
            compile_java(symbol_classes, symbol_sources, symbol_classpath)
        symbol_path = symbol_classes if symbol_classes.is_dir() else None
        compile_classpath = (
            jasmin_classpath
            + ([] if transformer_jar is None else [transformer_jar])
            + ([hidden_compiler_classes]
               if hidden_compiler_classes.is_dir() else [])
            + ([source_test_root / "classes"]
               if (source_test_root / "classes").is_dir() else [])
            + ([] if symbol_path is None else [symbol_path])
        )
        compile_java(classes, source_groups.get("src", []), compile_classpath)
        compile_java(classes, source_groups.get("src-art", []), compile_classpath)
        compile_java(classes, source_groups.get("src2", []), compile_classpath)
        # AOSP's default build compiles all javac source groups before sourcing
        # javac_post.sh. Several unchanged tests move or remove classes across
        # `classes`, `classes2`, and `classes-ex`; converting any group to DEX
        # before this phase changes the test itself.
        grouped_class_roots: dict[str, Path] = {}
        for name in ("src-multidex", "src-aotex", "src-bcpex"):
            group_classes = temporary / (
                "classes2" if name == "src-multidex" else f"classes-{name}"
            )
            compile_java(group_classes, source_groups.get(name, []), compile_classpath)
            grouped_class_roots[name] = group_classes
        ex_classes = temporary / "classes-ex"
        compile_java(ex_classes, source_groups.get("src-ex", []), compile_classpath)
        compile_java(ex_classes, source_groups.get("src-ex2", []), compile_classpath)
        generated_classes = source_test_root / "classes"
        if generated_build and generated_classes.is_dir():
            # AOSP generators may emit their final, bytecode-transformed class
            # files directly into classes/ rather than creating src/. Preserve
            # that output as primary DEX input (for example test 1948's
            # CONSTANT_MethodHandle constants and embedded replacement DEX).
            shutil.copytree(generated_classes, classes, dirs_exist_ok=True)
        generated_javac_post = source_test_root / "javac_post.sh"
        javac_post_contract = (
            compile_javac_post_contract(generated_javac_post.read_bytes(), generated_javac_post)
            if generated_javac_post.is_file() else None
        )
        javac_post_takes_output = (
            javac_post_contract is not None and _post_requires_argument(javac_post_contract)
        )
        generated_transformer = source_test_root / "transformer.jar"
        if transformer_jar is not None:
            intermediate_classes = temporary / "intermediate-classes"
            classes.rename(intermediate_classes)
            classes.mkdir()
            transformer_cp = f"{asm}:{transformer_jar}"
            for class_file in sorted(intermediate_classes.glob("*.class")):
                command([
                    "java", "-cp", transformer_cp,
                    "transformer.IndyTransformer", str(class_file),
                    str(classes / class_file.name),
                ])

        def compile_dex(output: Path, class_root: Path, *, support: bool = False) -> Path | None:
            class_files = sorted(class_root.rglob("*.class")) if class_root.is_dir() else []
            if not class_files and not support:
                return None
            output.mkdir(parents=True, exist_ok=True)
            inputs = ([str(root / "_build/button-dex/dex/classes.dex")] if support else [])
            if sum(len(str(path)) for path in class_files) > 128 * 1024:
                class_jar = output.parent / f"{output.name}-javac-input.jar"
                command([
                    "jar", "cf", str(class_jar), "-C", str(class_root), ".",
                ])
                inputs.append(str(class_jar))
            else:
                inputs.extend(map(str, class_files))
            d8_arguments = [
                "java",
                *d8_jvm_flags,
                *(["-Dcom.android.tools.r8.dexContainerExperiment"]
                  if upstream_d8_dex_container else []),
                "-cp", str(r8), "com.android.tools.r8.D8",
                *(["--no-desugaring"] if upstream_no_desugar else []),
                *(["--intermediate"] if upstream_d8_intermediate else []),
                *[flag for flag in d8_tool_flags if flag != "--intermediate"],
                "--min-api", upstream_api_level,
                "--lib", str(platform / "android.jar"), "--output", str(output),
                *inputs,
            ]
            command(d8_arguments)
            return output / "classes.dex"

        def execute_javac_post(argument: Path | None) -> None:
            if javac_post_contract is None:
                return
            java_tool = shutil.which("java")
            javap_tool = shutil.which("javap")
            if java_tool is None or javap_tool is None:
                raise RuntimeError("typed javac_post requires java and javap executables")
            post_context = BuildActionContext(
                test_root=temporary,
                toolchain_root=(generated_build_top if generated_build_top is not None
                                 else root),
                argument1=argument,
                external_tools={"java": Path(java_tool), "javap": Path(javap_tool)},
                environment=os.environ,
            )
            post_plan = evaluate_build_action_plan(javac_post_contract, post_context)
            execute_build_action_plan(post_plan, post_context)

        if generated_transformer.is_file():
            shutil.copyfile(generated_transformer, temporary / "transformer.jar")
        if javac_post_contract is not None:
            # Sourced javac_post scripts without $1 run once.  Scripts using
            # the output argument run once for the primary classes directory;
            # secondary/ex groups are replayed at their original build phase.
            execute_javac_post(classes if javac_post_takes_output else None)

        generated_test_jar = (
            source_test_root / f"{args.test}.jar"
            if generated_build else None
        )
        # AOSP's prebuilt test jars are already the complete input container.
        # Preserve their entry ordering, compression, and intentionally odd
        # DEX payload sizes instead of routing them through the generated-DEX
        # assembly path (which also requires a build.py default_build action).
        prebuilt_test_jar = (
            source_test_root / f"{args.test}.jar"
            if not generated_build and has_prebuilt_jar else None
        )
        support_dex = root / "_build/button-dex/dex/classes.dex"
        generated_primary_dex: Path | None = None
        generated_extra_dexes: list[Path] = []
        if generated_test_jar is not None and generated_test_jar.is_file():
            generated_primary_dex = temporary / "generated-primary.dex"
            with zipfile.ZipFile(generated_test_jar) as archive:
                generated_primary_dex.write_bytes(archive.read("classes.dex"))
                dex_entries = sorted(
                    (name for name in archive.namelist()
                     if re.fullmatch(r"classes[2-9][0-9]*\.dex", name)),
                    key=lambda name: int(name[7:-4]),
                )
                for index, entry in enumerate(dex_entries, start=2):
                    extra = temporary / f"generated-classes{index}.dex"
                    extra.write_bytes(archive.read(entry))
                    generated_extra_dexes.append(extra)
        prebuilt_primary_dex: Path | None = None
        if prebuilt_test_jar is not None:
            prebuilt_primary_dex = temporary / "prebuilt-jar-primary.dex"
            with zipfile.ZipFile(prebuilt_test_jar) as archive:
                prebuilt_primary_dex.write_bytes(archive.read("classes.dex"))
        if (source_test_root / "classes.dex").is_file():
            prebuilt_primary_dex = temporary / "prebuilt-primary.dex"
            # These fixtures deliberately contain bytecode D8 rejects or
            # normalizes (duplicate methods, odd file sizes, invalid casts).
            # Preserve the pinned input exactly as AOSP run_test_build does.
            shutil.copyfile(source_test_root / "classes.dex", prebuilt_primary_dex)
        def assemble_smali(output: Path, source_root: Path) -> Path | None:
            smali_sources = sorted(source_root.rglob("*.smali")) \
                if source_root.is_dir() else []
            if not smali_sources:
                return None
            command([
                "java", "-Xmx512m", "-jar", str(smali), "assemble",
                # AOSP run_test_build.py defaults ordinary run-tests to API 26.
                "--api", upstream_api_level, "--output", str(output),
                *map(str, smali_sources),
            ])
            if not output.is_file():
                raise RuntimeError(f"smali assembler did not create {output}")
            return output

        # Smali-only tests can select an API level below the launcher support
        # DEX threshold (notably the generated default-method suites). Build
        # their primary handwritten DEX before choosing the primary payload so
        # an empty javac output does not trip the no-input assertion.
        primary_smali = None if not upstream_use_smali else assemble_smali(
            temporary / "smali-classes.dex", source_test_root / "smali")
        primary_dex = generated_primary_dex or prebuilt_primary_dex or compile_dex(
            dex, classes, support=support_in_primary_dex)
        if primary_dex is None and primary_smali is not None:
            primary_dex, primary_smali = primary_smali, None
        assert primary_dex is not None

        def merge_dex(output: Path, *inputs: Path | None) -> Path:
            existing = [value for value in inputs if value is not None and value.is_file()]
            if not existing:
                raise RuntimeError(f"no DEX inputs for {output}")
            merged = temporary / f"merge-{output.stem}-{len(existing)}"
            merged.mkdir()
            command([
                "java",
                *d8_jvm_flags,
                *(["-Dcom.android.tools.r8.dexContainerExperiment"]
                  if upstream_d8_dex_container else []),
                "-cp", str(r8), "com.android.tools.r8.D8",
                *( ["--no-desugaring"] if upstream_no_desugar else []),
                *[flag for flag in d8_tool_flags if flag != "--intermediate"],
                "--min-api", upstream_api_level, "--output", str(merged),
                *map(str, existing),
            ])
            result = merged / "classes.dex"
            if not result.is_file() or (merged / "classes2.dex").exists():
                raise RuntimeError(f"DEX merge did not produce one output: {merged}")
            shutil.copyfile(result, output)
            return output

        primary_jasmin_dex = compile_dex(
            temporary / "dex-jasmin", primary_jasmin_classes
            if primary_jasmin_classes is not None else temporary / "no-jasmin")
        if primary_jasmin_dex is not None:
            primary_dex = merge_dex(dex / "classes-with-jasmin.dex",
                                    primary_dex, primary_jasmin_dex)
        if generated_primary_dex is not None:
            primary_smali = None
        if primary_smali is not None:
            # Match AOSP run_test_build.py: handwritten bytecode is merged
            # after javac/D8 output into the primary application DexFile.
            primary_dex = merge_dex(dex / "classes-with-smali.dex",
                                    primary_dex, primary_smali)
        grouped_extra_dexes: dict[str, Path] = {}
        for name in ("src-multidex", "src-aotex", "src-bcpex"):
            # AOSP's default builder names the secondary javac output
            # `classes2`. javac_post scripts consume that logical name and may
            # move already-compiled primary classes across the DEX boundary.
            group_classes = grouped_class_roots[name]
            if (javac_post_takes_output
                    and group_classes.is_dir()
                    and any(group_classes.glob("*.class"))):
                execute_javac_post(group_classes)
            group_dex = compile_dex(temporary / f"dex-{name}", group_classes)
            if group_dex is not None:
                grouped_extra_dexes[name] = group_dex

        secondary_smali = assemble_smali(
            temporary / "smali-classes2.dex", source_test_root / "smali-multidex")
        if not upstream_use_smali:
            secondary_smali = None
        secondary_java = grouped_extra_dexes.pop("src-multidex", None)
        secondary_jasmin = compile_dex(
            temporary / "dex-jasmin2", secondary_jasmin_classes
            if secondary_jasmin_classes is not None else temporary / "no-jasmin2")
        # A generator may intentionally emit a DEX that D8 must not parse
        # (for example a cyclic class definition in DEX 035). Keep that DEX
        # byte-for-byte as classes.dex and place our launcher support in the
        # next multidex entry instead of attempting an invalid merge.
        extra_dexes: list[Path] = list(generated_extra_dexes)
        if (generated_primary_dex is not None or
                prebuilt_primary_dex is not None or
                not support_in_primary_dex):
            extra_dexes.append(root / "_build/button-dex/dex/classes.dex")
        if (secondary_smali is not None or secondary_java is not None or
                secondary_jasmin is not None):
            extra_dexes.append(merge_dex(
                temporary / "classes2-with-smali.dex",
                secondary_java, secondary_jasmin, secondary_smali))
        for name in ("src-aotex", "src-bcpex"):
            if name in grouped_extra_dexes:
                extra_dexes.append(grouped_extra_dexes[name])

        if (javac_post_takes_output
                and ex_classes.is_dir() and any(ex_classes.glob("*.class"))):
            execute_javac_post(ex_classes)
        ex_dex = compile_dex(temporary / "dex-ex", ex_classes)
        ex_smali = assemble_smali(
            temporary / "smali-classes-ex.dex", source_test_root / "smali-ex")
        if not upstream_use_smali:
            ex_smali = None
        if ex_smali is not None:
            ex_dex = merge_dex(temporary / "classes-ex-with-smali.dex",
                               ex_dex, ex_smali)
        dex_payloads = [primary_dex, *extra_dexes]
        if ex_dex is not None:
            dex_payloads.append(ex_dex)
        # A multi-stage AOSP build may mutate the first DEX while producing a
        # boot-only jar, then call default_build again for the normal app jar.
        # Keep a private byte-for-byte snapshot so each typed default_build
        # starts from the same javac/DEX inputs.
        raw_dex_snapshots: list[tuple[Path, Path]] = []
        for index, dex_payload in enumerate(dex_payloads):
            snapshot = temporary / f"raw-dex-{index}.dex"
            shutil.copyfile(dex_payload, snapshot)
            raw_dex_snapshots.append((dex_payload, snapshot))

        def restore_raw_dex() -> None:
            for dex_payload, snapshot in raw_dex_snapshots:
                shutil.copyfile(snapshot, dex_payload)

        def write_test_jars() -> None:
            if prebuilt_test_jar is not None:
                write_prebuilt_test_jar(
                    temporary / f"{args.test}.jar", prebuilt_test_jar,
                    support_dex)
                return
            if ex_dex is not None:
                write_aligned_stored_zip(
                    temporary / f"{args.test}-ex.jar",
                    [("classes.dex", ex_dex)],
                )
            # ART run-test exposes the test's primary dex both as the
            # application class path and as $DEX_LOCATION/<test>.jar.
            write_aligned_stored_zip(
                temporary / f"{args.test}.jar",
                [("classes.dex", primary_dex),
                 *[(f"classes{index}.dex", extra_dex)
                   for index, extra_dex in enumerate(extra_dexes, start=2)]],
            )

        def backend_default_build(kwargs: dict[str, object]) -> None:
            # The typed build contract can invoke the default action in
            # multiple corpus workers at once. That action rebuilds shared
            # boot-support artifacts (notably unsafe-boot-dex); without a
            # cross-process lock one worker can remove/replace the artifact
            # while another ART instance is resolving its boot class path.
            # Keep per-test DEX restoration/jar writes inside the same short
            # critical section, preserving deterministic AOSP build ordering.
            bootstrap_lock = root / "_build/art-upstream-test/bootstrap.lock"
            bootstrap_lock.parent.mkdir(parents=True, exist_ok=True)
            with bootstrap_lock.open("a+b") as lock_file:
                fcntl.flock(lock_file.fileno(), fcntl.LOCK_EX)
                restore_raw_dex()
                if kwargs.get("use_hiddenapi", False):
                    hiddenapi_flags = source_test_root / "hiddenapi-flags.csv"
                    if not hiddenapi_flags.is_file():
                        raise RuntimeError(
                            f"hidden-api build is missing flags: {hiddenapi_flags}")
                    command([
                        "python3", str(root / "tools/encode-hiddenapi-dex.py"),
                        "--api-flags", str(hiddenapi_flags),
                        *map(str, dex_payloads),
                    ])
                write_test_jars()

        if prebuilt_test_jar is not None:
            # ``build.py`` is intentionally a no-op for these fixtures; the
            # checked-in jar itself is the AOSP build output.
            write_test_jars()
        elif build_contract is not None:
            # The typed prefix was already consumed before source discovery.
            # Resume at the first default_build only after DEX inputs exist;
            # this preserves build.py's ordered file moves and rebuild calls.
            if _contract_has_action(build_plan_suffix, "file_edit"):
                # build.py addresses the AOSP staging name classes.dex, while
                # the host compiler keeps its output in dex/classes.dex.
                # Materialize that typed boundary for file edits (not for
                # arbitrary paths) before executing the suffix.
                shutil.copyfile(primary_dex, temporary / "classes.dex")
            build_context.default_build = backend_default_build
            execute_build_action_plan(
                ActionPlan(build_plan.source, tuple(build_plan_suffix)),
                build_context)
        else:
            backend_default_build({})

        boot_image_class_path = [
            root / "_build/android16-core-oj-compat/core-oj-compat.jar",
            root / "_prebuilt/android-16/bootclasspath/core-libart.jar",
            root / "_build/android16-framework-compat/framework-compat.jar",
            root / "_prebuilt/android-16/bootclasspath/framework-location.jar",
            root / "_build/android16-ps16k-r07/extracted/conscrypt/javalib/conscrypt.jar",
            root / "_build/android16-ps16k-r07/extracted/bt/javalib/framework-bluetooth.jar",
            root / "_build/android16-ps16k-r07/extracted/mediaprovider/javalib/framework-mediaprovider.jar",
            root / "_build/android16-ps16k-r07/extracted/permission/javalib/framework-permission.jar",
            root / "_build/android16-ps16k-r07/extracted/permission/javalib/framework-permission-s.jar",
            root / "_build/android16-ps16k-r07/extracted/art/javalib/okhttp.jar",
            root / "_build/bootclasspath/core-icu4j-api36.jar",
        ]
        boot_tail = [
            *boot_image_class_path[3:],
            root / "_build/dex-probe/unsafe-boot-dex/unsafe-boot.jar",
        ]
        # The detached host exposes the complete Android boot class path to
        # the runtime, including the support DEX appended to the fourth CLI
        # class-path argument.  App/vdex compilation must use that same
        # identity; omitting the tail makes ART reject the freshly compiled
        # OAT as kOatBootImageOutOfDate and silently fall back to VDEX.
        dex2oat_boot_class_path = [
            *boot_image_class_path,
            root / "_build/dex-probe/unsafe-boot-dex/unsafe-boot.jar",
        ]
        boot_image = (args.boot_image.resolve() if args.boot_image is not None
                      else root / "_build/android16-boot-image-darwin/boot.art")
        # Accept the same image-directory spelling used by ART's build and
        # installation tooling while retaining the canonical boot.art path in
        # every runtime and dex2oat argument.
        if boot_image.is_dir():
            boot_image = boot_image / "boot.art"
        runtime_dylib = (
            root / "_build/runtime-graphics-link-probe/"
            "libdarwin_art_runtime_graphics.dylib"
        )
        profile: Path | None = None
        if upstream_profile:
            profile_source = source_test_root / "profile"
            if not profile_source.is_file():
                raise RuntimeError(
                    f"profile=True test has no generated profile: {profile_source}")
            if not boot_image.is_file():
                raise RuntimeError(
                    f"profile=True test requires Darwin boot image: {boot_image}")
            profile = temporary / f"{args.test}.prof"
            test_jar = temporary / f"{args.test}.jar"
            command([
                str(root / "target/debug/darwin-art-host"), "--profman",
                str(runtime_dylib),
                f"--create-profile-from={profile_source}",
                f"--apk={test_jar}", f"--dex-location={test_jar}",
                *([
                    f"--apk={temporary / f'{args.test}-ex.jar'}",
                    f"--dex-location={temporary / f'{args.test}-ex.jar'}",
                ] if (temporary / f"{args.test}-ex.jar").is_file() else []),
                f"--reference-profile-file={profile}",
            ])
        test_jar = temporary / f"{args.test}.jar"
        app_oat_dir = temporary / "oat/arm64"
        app_oat_dir.mkdir(parents=True, exist_ok=True)
        app_image = app_oat_dir / f"{args.test}.art"
        app_oat = app_oat_dir / f"{args.test}.odex"
        app_vdex = app_oat.with_suffix(".vdex")
        secondary_jar = temporary / f"{args.test}-ex.jar"
        secondary_image = app_oat_dir / f"{args.test}-ex.art"
        secondary_oat = app_oat_dir / f"{args.test}-ex.odex"
        secondary_vdex = app_oat_dir / f"{args.test}-ex.vdex"

        def build_app_contract(invocation: RunInvocation, *,
                               dex_input: Path | None = None,
                               oat_directory: Path | None = None,
                               secondary_input: Path | None = None,
                               staging_root: Path | None = None) -> None:
            # Build artifacts must be keyed to the exact DEX location passed to
            # the VM.  In differential mode each process has a private
            # DEX_LOCATION; compiling only against the outer temporary JAR
            # makes ART reject the adjacent oat/vdex and silently interpret.
            dex_input = test_jar if dex_input is None else dex_input
            oat_directory = app_oat_dir if oat_directory is None else oat_directory
            secondary_input = secondary_jar if secondary_input is None else secondary_input
            staging_root = temporary if staging_root is None else staging_root
            local_app_image = oat_directory / f"{args.test}.art"
            local_app_oat = oat_directory / f"{args.test}.odex"
            local_app_vdex = local_app_oat.with_suffix(".vdex")
            local_secondary_image = oat_directory / f"{args.test}-ex.art"
            local_secondary_oat = oat_directory / f"{args.test}-ex.odex"
            local_secondary_vdex = local_secondary_oat.with_suffix(".vdex")
            for output in (
                    local_app_image, local_app_oat, local_app_vdex,
                    local_secondary_image, local_secondary_oat,
                    local_secondary_vdex):
                output.unlink(missing_ok=True)
            if not invocation.prebuild:
                return
            compiler_options = list(dict.fromkeys((
                *invocation.compiler_options,
                *invocation.compiler_only_options,
            )))
            if not any(option.startswith("--compiler-filter=")
                       for option in compiler_options):
                compiler_options.append(
                    "--compiler-filter=speed-profile"
                    if invocation.profile else "--compiler-filter=speed")
            image_arguments = []
            if invocation.image and invocation.app_image:
                image_arguments = [
                    f"--app-image-file={local_app_image}",
                    "--resolve-startup-const-strings=true",
                ]
            profile_arguments = []
            if invocation.profile:
                if profile is None:
                    raise RuntimeError("profile invocation has no profile artifact")
                profile_arguments = [f"--profile-file={profile}"]
            command([
                str(root / "target/debug/darwin-art-host"), "--dex2oat",
                str(runtime_dylib), "--android-root=/",
                f"--boot-image={boot_image}", f"--dex-file={dex_input}",
                f"--dex-location={dex_input}", f"--oat-file={local_app_oat}",
                *image_arguments, *profile_arguments, *compiler_options,
                "--compile-art-test", "--generate-mini-debug-info",
                f"--watchdog-timeout={invocation.dex2oat_timeout}000",
                "--instruction-set=arm64",
                "--runtime-arg",
                "-Xbootclasspath:"
                + ":".join(str(path)
                           for path in dex2oat_boot_class_path),
                "--runtime-arg",
                "-Xbootclasspath-locations:"
                + ":".join(str(path.relative_to(root))
                           for path in dex2oat_boot_class_path),
            ], timeout=invocation.dex2oat_rt_timeout, cwd=root)
            required = [local_app_oat, local_app_vdex]
            if invocation.image and invocation.app_image:
                required.append(local_app_image)
            if not all(output.is_file() for output in required):
                raise RuntimeError("dex2oat did not create the requested app artifacts")
            if secondary_input.is_file() and invocation.secondary_compilation:
                secondary_image_arguments = []
                if (invocation.image and invocation.app_image and
                        invocation.secondary_app_image):
                    secondary_image_arguments = [
                        f"--app-image-file={local_secondary_image}",
                        "--resolve-startup-const-strings=true",
                    ]
                class_loader_context = (
                    invocation.secondary_class_loader_context or
                    (f"PCL[{dex_input}]" if invocation.secondary else
                     f"PCL[];PCL[{dex_input}]")
                )
                class_loader_context = class_loader_context.format(
                    DEX_LOCATION=staging_root, TEST_NAME=args.test)
                command([
                    str(root / "target/debug/darwin-art-host"), "--dex2oat",
                    str(runtime_dylib), "--android-root=/",
                    f"--boot-image={boot_image}",
                    f"--dex-file={secondary_input}",
                    f"--dex-location={secondary_input}",
                    f"--oat-file={local_secondary_oat}",
                    *secondary_image_arguments,
                    *profile_arguments, *compiler_options,
                    "--compile-art-test", "--generate-mini-debug-info",
                    f"--watchdog-timeout={invocation.dex2oat_timeout}000",
                    f"--class-loader-context={class_loader_context}",
                    "--instruction-set=arm64",
                    "--runtime-arg",
                    "-Xbootclasspath:"
                    + ":".join(str(path)
                               for path in dex2oat_boot_class_path),
                    "--runtime-arg",
                    "-Xbootclasspath-locations:"
                    + ":".join(str(path.relative_to(root))
                               for path in dex2oat_boot_class_path),
                ], timeout=invocation.dex2oat_rt_timeout, cwd=root)
                secondary_required = [local_secondary_oat, local_secondary_vdex]
                if (invocation.image and invocation.app_image and
                        invocation.secondary_app_image):
                    secondary_required.append(local_secondary_image)
                if not all(output.is_file() for output in secondary_required):
                    raise RuntimeError(
                        "dex2oat did not create requested secondary artifacts")

        first_app_invocation = upstream_run_invocations[0]
        if not (first_app_invocation.vdex or
                first_app_invocation.dex2oat_dm or
                first_app_invocation.runtime_dm):
            build_app_contract(first_app_invocation)
        current_app_contract = (
            first_app_invocation.prebuild,
            first_app_invocation.app_image,
            first_app_invocation.profile,
            first_app_invocation.compiler_options,
            first_app_invocation.compiler_only_options,
            first_app_invocation.secondary,
            first_app_invocation.secondary_app_image,
            first_app_invocation.secondary_compilation,
            first_app_invocation.secondary_class_loader_context,
            first_app_invocation.verify,
            first_app_invocation.verify_soft_fail,
            first_app_invocation.image,
            first_app_invocation.relocate,
            first_app_invocation.dex2oat_timeout,
            first_app_invocation.dex2oat_rt_timeout,
        )
        app_image_outputs: list[Path] = [
            app_oat, app_vdex, secondary_oat, secondary_vdex,
        ]
        if first_app_invocation.image and first_app_invocation.app_image:
            app_image_outputs.append(app_image)
        if profile is not None:
            app_image_outputs.append(profile)
        if first_app_invocation.image and first_app_invocation.app_image and \
                secondary_jar.is_file() and \
                first_app_invocation.secondary_app_image:
            app_image_outputs.append(secondary_image)
        vdex_outputs: list[Path] = []
        if upstream_vdex:
            if not boot_image.is_file():
                raise RuntimeError(
                    f"vdex=True test requires Darwin boot image: {boot_image}")
            test_jar = temporary / f"{args.test}.jar"
            vdex_oat_dir = temporary / "oat/arm64"
            vdex_oat_dir.mkdir(parents=True, exist_ok=True)
            vdex_oat = vdex_oat_dir / f"{args.test}.odex"
            vdex_file = vdex_oat.with_suffix(".vdex")

            def build_vdex_contract(
                    invocation: RunInvocation, *,
                    dex_input: Path | None = None,
                    oat_directory: Path | None = None) -> None:
                # VDEX/OAT identity includes the exact DEX location. Each
                # differential mode uses a private copy of the test JAR, so
                # compile beside that mode-local copy.
                dex_input = test_jar if dex_input is None else dex_input
                oat_directory = app_oat_dir if oat_directory is None else oat_directory
                local_app_image = oat_directory / f"{args.test}.art"
                local_vdex_oat = oat_directory / f"{args.test}.odex"
                local_vdex_file = local_vdex_oat.with_suffix(".vdex")
                local_vdex_oat.unlink(missing_ok=True)
                local_vdex_file.unlink(missing_ok=True)
                local_app_image.unlink(missing_ok=True)
                vdex_image_arguments = []
                if invocation.image and invocation.app_image:
                    vdex_image_arguments = [
                        f"--app-image-file={local_app_image}",
                        "--resolve-startup-const-strings=true",
                    ]
                base_arguments = [
                    str(root / "target/debug/darwin-art-host"), "--dex2oat",
                    str(runtime_dylib), "--android-root=/",
                    f"--boot-image={boot_image}", f"--dex-file={dex_input}",
                    f"--dex-location={dex_input}",
                    f"--oat-file={local_vdex_oat}",
                    *vdex_image_arguments,
                    "--compile-art-test", "--generate-mini-debug-info",
                    *invocation.compiler_options,
                    *invocation.compiler_only_options,
                    "--instruction-set=arm64",
                    "--runtime-arg",
                "-Xbootclasspath:"
                + ":".join(str(path)
                           for path in dex2oat_boot_class_path),
                    "--runtime-arg",
                "-Xbootclasspath-locations:"
                + ":".join(str(path.relative_to(root))
                           for path in dex2oat_boot_class_path),
                ]
                command(base_arguments, timeout=300, cwd=root)
                if not local_vdex_file.is_file():
                    raise RuntimeError("initial dex2oat did not create vdex")
                if (invocation.image and invocation.app_image and
                        not local_app_image.is_file()):
                    raise RuntimeError("vdex dex2oat did not create app image")
                if invocation.vdex_filter is None:
                    local_vdex_oat.unlink()
                else:
                    command([
                        *base_arguments,
                        f"--compiler-filter={invocation.vdex_filter}",
                        f"--input-vdex={local_vdex_file}",
                    ], timeout=300, cwd=root)
                    if not local_vdex_oat.is_file():
                        raise RuntimeError("vdex recompilation did not create odex")

            first_vdex_invocation = next(
                invocation for invocation in upstream_run_invocations
                if invocation.vdex)
            build_vdex_contract(first_vdex_invocation)
            current_vdex_contract = (
                first_vdex_invocation.compiler_options,
                first_vdex_invocation.compiler_only_options,
                first_vdex_invocation.vdex_filter,
            )
            vdex_outputs = [vdex_file]
            if first_vdex_invocation.vdex_filter is not None:
                vdex_outputs.append(vdex_oat)
        dm_primary_vdex = app_oat_dir / "primary.vdex"
        dex2oat_dm_file = app_oat_dir / f"{args.test}.dm"
        runtime_dm_file = temporary / f"{args.test}.dm"

        def build_dm_contract(invocation: RunInvocation) -> None:
            for output in (
                    app_image, app_oat, app_vdex, dm_primary_vdex,
                    dex2oat_dm_file, runtime_dm_file):
                output.unlink(missing_ok=True)
            compiler_options = list(dict.fromkeys((
                *invocation.compiler_options,
                *invocation.compiler_only_options,
            )))
            if not any(option.startswith("--compiler-filter=")
                       for option in compiler_options):
                compiler_options.append("--compiler-filter=verify")
            image_arguments = []
            if invocation.app_image:
                image_arguments = [
                    f"--app-image-file={app_image}",
                    "--resolve-startup-const-strings=true",
                ]
            base_arguments = [
                str(root / "target/debug/darwin-art-host"), "--dex2oat",
                str(runtime_dylib), "--android-root=/",
                f"--boot-image={boot_image}", f"--dex-file={test_jar}",
                f"--dex-location={test_jar}", f"--oat-file={app_oat}",
                *image_arguments, *compiler_options,
                "--compile-art-test", "--generate-mini-debug-info",
                "--instruction-set=arm64",
                "--runtime-arg",
                "-Xbootclasspath:"
                + ":".join(str(path)
                           for path in dex2oat_boot_class_path),
                "--runtime-arg",
                "-Xbootclasspath-locations:"
                + ":".join(str(path.relative_to(root))
                           for path in dex2oat_boot_class_path),
            ]
            command([
                *base_arguments, "--copy-dex-files=false",
                f"--output-vdex={dm_primary_vdex}",
            ], timeout=300, cwd=root)
            if not dm_primary_vdex.is_file():
                raise RuntimeError("dex2oat did not create DM primary.vdex")
            dm_file = (dex2oat_dm_file if invocation.dex2oat_dm
                       else runtime_dm_file)
            with zipfile.ZipFile(dm_file, "w") as archive:
                archive.write(dm_primary_vdex, "primary.vdex",
                              compress_type=zipfile.ZIP_DEFLATED)
            if invocation.dex2oat_dm:
                command([
                    *base_arguments, "--dump-timings",
                    f"--dm-file={dex2oat_dm_file}",
                ], timeout=300, cwd=root)

        dm_invocations = [
            invocation for invocation in upstream_run_invocations
            if invocation.dex2oat_dm or invocation.runtime_dm
        ]
        current_dm_contract = None
        if dm_invocations:
            first_dm_invocation = dm_invocations[0]
            build_dm_contract(first_dm_invocation)
            current_dm_contract = (
                first_dm_invocation.dex2oat_dm,
                first_dm_invocation.runtime_dm,
                first_dm_invocation.app_image,
                first_dm_invocation.compiler_options,
                first_dm_invocation.compiler_only_options,
            )
        dm_outputs = [
            dm_primary_vdex, dex2oat_dm_file, runtime_dm_file,
        ] if dm_invocations else []
        host_arguments = [
            str(root / "target/debug/darwin-art-fixture-runner"),
            "--fixture-image", str(root / "_build/native-fixtures/graphics/libdarwin_art_fixture.dylib"),
            "--window-seconds", "0",
            str(root / "_build/runtime-graphics-link-probe/libdarwin_art_runtime_graphics.dylib"),
            str(root / "_build/android16-core-oj-compat/core-oj-compat.jar"),
            str(root / "_prebuilt/android-16/bootclasspath/core-libart.jar"),
            str(root / "_build/android16-framework-compat/framework-compat.jar"),
            ":".join(map(str, boot_tail)), str(temporary / f"{args.test}.jar"),
        ]
        if cfi_contract:
            # Both debuggerd-side task_for_pid and the exec'd helper must be
            # explicit debug targets under macOS task-port policy.
            command([
                "codesign", "--force", "--sign", "-", "--entitlements",
                str(root / "tools/unwindstack-task-access.entitlements.plist"),
                host_arguments[0],
            ])
        runtime_host_files = [
            temporary / f"{args.test}.jar", *dex2oat_boot_class_path,
            *app_image_outputs,
            *vdex_outputs, *dm_outputs,
        ]
        ex_jar = temporary / f"{args.test}-ex.jar"
        if ex_jar.is_file():
            runtime_host_files.append(ex_jar)
        native_fixture_dir = temporary / "native"
        native_fixture_dir.mkdir()
        loader_source = root / "probes/runtime_upstream_arttest_loader.cc"
        # AOSP links every native run-test against one shared libarttest. Keep
        # common JNI helpers that are consumed across test directories in the
        # Darwin module as well; 2262 intentionally obtains GetMethodId from
        # 1972's library member rather than defining it locally.
        shared_native_sources = [
            root / "_aosp/art/test/466-get-live-vreg/get_live_vreg_jni.cc",
            root / "_aosp/art/test/570-checker-osr/osr.cc",
            root / "_aosp/art/test/597-deopt-new-string/deopt.cc",
            root / "_aosp/art/test/626-const-class-linking/clear_dex_cache_types.cc",
            root / "_aosp/art/test/1972-jni-id-swap-indices/jni_id.cc",
            root / "_aosp/art/test/2262-miranda-methods/jni_invoke.cc",
        ]

        def compile_arttest(sources: list[Path], output: Path,
                            *, include_loader: bool = True,
                            install_name: str = "libarttest.so",
                            dependencies: tuple[Path, ...] = (),
                            rpaths: tuple[str, ...] = ()) -> None:
            platform_compat = []
            selected_sources = ([loader_source] if include_loader else []) + sources
            source_include_dirs = sorted({
                str(source.parent) for source in selected_sources
                if source.is_file()
            })
            native_capability_text = b"\n".join(
                source.read_bytes() for source in sources if source.is_file())
            if uses_native_bridge or b"ucontext_t" in native_capability_text:
                platform_compat = [
                    "-include", str(root / "compat/darwin_art_test_ucontext.h")
                ]
            elif b"kReferenceVReg" in native_capability_text:
                platform_compat = [
                    "-include",
                    str(root / "compat/darwin_art_test_reference_cast.h"),
                ]
            # Android's libarttest target exports JNI entry points even when
            # an old AOSP source omits JNIEXPORT (004-JniTest's
            # testZeroLengthByteBuffers is one such declaration).  Compile
            # with normal visibility, then constrain the Mach-O dynamic
            # symbol table to the JNI ABI surface.  This preserves the
            # Android namespace boundary for test-local C++ state while
            # keeping dlsym() compatible with the platform's name lookup.
            exported_symbols: set[bytes] = set()
            definition_pattern = re.compile(
                rb"\b((?:Java_[A-Za-z0-9_]+|JNI_OnLoad|JNI_OnUnload))"
                rb"\s*\([^;{}]*\)\s*(?:const\s*)?\{")
            for source in selected_sources:
                if not source.is_file():
                    continue
                source_text = source.read_bytes()
                for match in definition_pattern.finditer(source_text):
                    line_start = source_text.rfind(b"\n", 0, match.start()) + 1
                    # A static helper can use a Java-style name internally
                    # (004-JniTest's FastNative callbacks do); it is not part
                    # of the DSO lookup ABI and must not appear in the export
                    # list or ld will reject the image.
                    if re.search(rb"\bstatic\b", source_text[line_start:match.start()]):
                        continue
                    exported_symbols.add(match.group(1))
            export_file = output.with_name(output.name + ".exports")
            owned_hooks = {
                match.group(1) for match in definition_pattern.finditer(native_capability_text)
            }
            loader_flags = [
                f"-DDARWIN_ART_TEST_HAS_{hook.decode().upper()}"
                for hook in (b"JNI_OnLoad", b"JNI_OnUnload") if hook in owned_hooks
            ]
            export_file.write_text(
                "\n".join(f"_{name}" for name in sorted(
                    symbol.decode() for symbol in exported_symbols)) + "\n",
                encoding="utf-8",
            )
            try:
                command([
                "clang++", "-std=c++20", "-O2", "-dynamiclib",
                # Android's linker namespaces keep each test DSO's private
                # C++ state local. Darwin otherwise permits default-visible
                # data symbols from a copied test library to interpose on the
                # original image (for example hiddenapi's dex vector).
                "-fvisibility=default",
                "-Wno-invalid-offsetof", "-Wno-unsupported-visibility",
                f"-Wl,-install_name,@rpath/{install_name}",
                "-Wl,-exported_symbols_list," + str(export_file),
                *(f"-Wl,-rpath,{path}" for path in rpaths),
                "-pthread",
                *loader_flags,
                "-DART_PAGE_SIZE_AGNOSTIC", "-DBUILDING_LIBART",
                "-DNDEBUG",
                "-DART_DEFAULT_GC_TYPE_IS_CMS", "-DART_USE_READ_BARRIER",
                "-DART_READ_BARRIER_TYPE_IS_BAKER=1",
                "-DART_FORCE_USE_READ_BARRIER",
                "-DART_STACK_OVERFLOW_GAP_arm=8192",
                "-DART_STACK_OVERFLOW_GAP_arm64=8192",
                "-DART_STACK_OVERFLOW_GAP_riscv64=8192",
                "-DART_STACK_OVERFLOW_GAP_x86=8192",
                "-DART_STACK_OVERFLOW_GAP_x86_64=8192",
                "-include", "base/globals.h",
                "-include", str(root / "_build/runtime-common/patched-source/runtime/mirror/object_reference.h"),
                "-include", str(root / "compat/darwin_art_pthread_name_compat.h"),
                *platform_compat,
                *sum((["-I", directory] for directory in source_include_dirs), []),
                "-I", str(test),
                "-I", str(root / "_aosp/art/test"),
                "-I", str(root / "_aosp/art/test/common"),
                # ART's native run-test defaults expose the JVMTI helper
                # headers to every test DSO, including sources selected from
                # a sibling test directory.
                "-I", str(root / "_aosp/art/test/ti-agent"),
                "-I", str(root / "_build/runtime-arm64/generated"),
                "-I", str(root / "_build/runtime-common/patched-source/runtime"),
                "-I", str(root / "_build/runtime-common/patched-source/runtime/oat"),
                "-I", str(root / "_aosp/art/runtime/oat"),
                "-I", str(root / "_build/foundation/patched-source/libartbase"),
                "-I", str(root / "_aosp/art/libartbase"),
                "-I", str(root / "_aosp/art/runtime"),
                "-I", str(root / "_aosp/art/runtime/base"),
                "-I", str(root / "_aosp/art/runtime/arch/arm64"),
                # Structural-redefine stack-scope fixtures include ART's
                # allocator wrapper, which in turn includes the pinned
                # AOSP dlmalloc header. Match the platform run-test include
                # surface instead of relying on the source directory.
                "-I", str(root / "_aosp/external/dlmalloc"),
                "-I", str(root / "_aosp/art/libdexfile"),
                "-I", str(root / "_aosp/art/libprofile"),
                "-I", str(root / "_aosp/art/libnativebridge/include"),
                # AOSP's generic native run-test target exposes the pinned
                # OpenJDK JVMTI headers to every test-library compile. Keep
                # this Android header surface separate from the host JDK so
                # native agents use the same ABI as the boot runtime.
                "-I", str(root / "_aosp/art/openjdkjvmti/include"),
                "-I", str(root / "_aosp/system/unwinding/libunwindstack/include"),
                "-I", str(root / "_aosp/system/libbase/include"),
                "-I", str(root / "_aosp/system/logging/liblog/include"),
                "-I", str(root / "_aosp/external/fmtlib/include"),
                "-I", str(root / "_aosp/external/tinyxml2"),
                "-I", str(root / "_aosp/libnativehelper/include_jni"),
                "-I", str(root / "_aosp/libnativehelper/header_only_include"),
                "-I", str(root / "_aosp/libnativehelper-full/include"),
                # libunwindstack's public API includes Android's ELF ABI. Keep
                # the pinned NDK headers behind Darwin's native headers so they
                # satisfy only Linux/ELF names unavailable in the host SDK.
                "-idirafter", str(ndk_include / "aarch64-linux-android"),
                "-idirafter", str(ndk_include),
                *([str(loader_source)] if include_loader else []),
                *map(str, sources),
                *map(str, dependencies),
                "-Wl,-rpath," + str(root / "_build/native-fixtures/graphics"),
                str(root / "_build/native-fixtures/graphics/libdarwin_art_fixture.dylib"),
                str(root / "_build/runtime-graphics-link-probe/libdarwin_art_runtime_graphics.dylib"),
                "-o", str(output),
                ])
            finally:
                export_file.unlink(missing_ok=True)

        def compile_native_bridge(source: Path, output: Path) -> None:
            command([
                "clang++", "-std=c++20", "-O2", "-dynamiclib", "-pthread",
                "-Wno-invalid-offsetof", "-Wno-unsupported-visibility",
                "-Wl,-install_name,@rpath/libnativebridgetest.so",
                "-DNDEBUG", "-DART_PAGE_SIZE_AGNOSTIC", "-DBUILDING_LIBART",
                "-DDARWIN_ART_TEST_SYNCHRONOUS_SELF_KILL",
                "-DDARWIN_ART_TEST_NATIVE_BRIDGE_SIGCHAIN",
                "-include", "base/globals.h",
                "-include", str(root / "compat/darwin_art_test_ucontext.h"),
                "-I", str(root / "_build/runtime-arm64/generated"),
                "-I", str(root / "_build/runtime-common/patched-source/runtime"),
                "-I", str(root / "_build/runtime-common/patched-source/runtime/oat"),
                "-I", str(root / "_aosp/art/runtime/oat"),
                "-I", str(root / "_build/foundation/patched-source/libartbase"),
                "-I", str(root / "_aosp/art/libartbase"),
                "-I", str(root / "_aosp/art/libnativebridge/include"),
                "-I", str(root / "_aosp/system/libbase/include"),
                "-I", str(root / "_aosp/external/fmtlib/include"),
                "-I", str(root / "_aosp/libnativehelper/include_jni"),
                "-I", str(root / "_aosp/libnativehelper/header_only_include"),
                "-idirafter", str(ndk_include / "aarch64-linux-android"),
                "-idirafter", str(ndk_include),
                str(source),
                str(root / "_build/runtime-graphics-link-probe/libdarwin_art_runtime_graphics.dylib"),
                "-o", str(output),
            ])

        def compile_tiagent(test_name: str, output: Path,
                            *, use_common_load: bool = False,
                            deferred_zygote: bool = False,
                            limited_capabilities: bool = False,
                            hello_agent: bool = False) -> None:
            """Build a pinned AOSP JVMTI test agent as its own native DSO."""
            ti_agent = root / "_aosp/art/test/ti-agent"
            # Keep this list aligned with libtiagent-base-defaults in the
            # pinned test/Android.bp.  These are the ART-independent JNI and
            # JVMTI helpers shared by Android's run-test agents; omitting them
            # makes otherwise generic tests look like per-test runtime gaps.
            base_sources = [
                ti_agent / name for name in (
                    "agent_common.cc",
                    "agent_startup.cc",
                    "jni_binder.cc",
                    "jvmti_helper.cc",
                    "test_env.cc",
                    "breakpoint_helper.cc",
                    "common_helper.cc",
                    "early_return_helper.cc",
                    "frame_pop_helper.cc",
                    "locals_helper.cc",
                    "monitors_helper.cc",
                    "redefinition_helper.cc",
                    "suspension_helper.cc",
                    "suspend_event_helper.cc",
                    "stack_trace_helper.cc",
                    "threads_helper.cc",
                    "trace_helper.cc",
                    "exceptions_helper.cc",
                )
            ]
            # AOSP's libtiagent-base-defaults exports these search helpers to
            # every JVMTI run-test. 980 intentionally reuses 929's JNI entry
            # to put its listener DEX on the bootstrap loader search path.
            base_sources.append(root / "_aosp/art/test/929-search/search.cc")
            if use_common_load:
                # common_load.cc is the single Android dispatcher. Link the
                # callback closure for its table (the shared helpers above
                # provide the common implementation) while keeping dormant
                # test bodies out of this per-test DSO.
                common_load_sources = (
                    "901-hello-ti-agent/basics.cc",
                    "909-attach-agent/attach.cc",
                    "936-search-onload/search_onload.cc",
                    "993-breakpoints-non-debuggable/onload.cc",
                    "1919-vminit-thread-start-timing/vminit.cc",
                    "ti-agent/redefinition_helper.cc",
                )
                base_sources.extend(
                    root / "_aosp/art/test" / source
                    for source in common_load_sources)
            definitions: list[str]
            sources: list[Path]
            if limited_capabilities:
                definitions = ["-DDARWIN_ART_TI_AGENT_993"]
                sources = [
                    ti_agent / "common_helper.cc",
                    ti_agent / "breakpoint_helper.cc",
                    root / "_aosp/art/test/993-breakpoints/breakpoints.cc",
                    root / "_aosp/art/test/993-breakpoints-non-debuggable/onload.cc",
                ]
            elif hello_agent:
                definitions = ["-DDARWIN_ART_TI_AGENT_901"]
                sources = [
                    source for source in
                    (root / "_aosp/art/test" / test_name).rglob("*.cc")
                    if b"OnLoad" in source.read_bytes()
                ]
            else:
                # This is AOSP common_load.cc's default: initialize one JVMTI
                # environment with the standard capability set, while the
                # test directory contributes its JNI/event callbacks.
                definitions = [] if use_common_load else [
                    "-DDARWIN_ART_TI_AGENT_MINIMAL"
                ]
                sources = sorted(
                    list((root / "_aosp/art/test" / test_name).rglob("*.cc")) +
                    list((root / "_aosp/art/test" / test_name).rglob("*.c"))
                )
                if use_common_load:
                    # Android's libtiagent target owns the common dispatcher
                    # and only the test's agent callback sources.  In
                    # particular, a test directory may also contain JNI
                    # sources for libarttest; do not duplicate those in the
                    # separately loaded agent DSO.
                    sources = [
                        source for source in sources
                        if b"OnAttach" in source.read_bytes()
                    ]
                if inherits_pop_frame_base:
                    # These tests extend Test1953 and intentionally reuse its
                    # native PopFrame owner. AOSP exposes the owner through
                    # the shared test native library; include that unchanged
                    # owner in this Darwin JVMTI agent as 1953 itself does.
                    sources.append(
                        root / "_aosp/art/test/1953-pop-frame/pop_frame.cc")
                if any(source.name == "source_transform_art.cc"
                       for source in sources):
                    # The pinned test defines two equivalent native verifier
                    # owners. Darwin uses AOSP's host-independent Slicer owner
                    # (also used by libctstiagent); the ART owner depends on
                    # libdexfile implementation symbols that are deliberately
                    # private to the runtime dylib. Never co-link both owners.
                    sources = [
                        source for source in sources
                        if source.name != "source_transform_art.cc"
                    ]
                if any(source.name == "check_deopt.cc" for source in sources):
                    # These two private instrumentation queries live in the
                    # in-runtime test bridge; the untouched JVMTI breakpoint
                    # and redefine implementation remains in this agent.
                    sources = [
                        source for source in sources
                        if source.name != "check_deopt.cc"
                    ]
                if any(source.name == "stack_scope.cc" for source in sources):
                    # This owner allocates ART mirrors and maintains reflective
                    # handles across structural redefinition. Keep it inside
                    # the runtime bridge rather than exporting private GC and
                    # ClassLinker C++ ABI through the Darwin dylib.
                    sources = [
                        source for source in sources
                        if source.name != "stack_scope.cc"
                    ]
                if any(source.name == "set-jni-id-used.cc"
                       for source in sources):
                    # This helper deliberately mutates ClassExt's private JNI
                    # ID marker. Execute it in the runtime bridge without
                    # exporting JniIdManager or mirror internals from ART.
                    sources = [
                        source for source in sources
                        if source.name != "set-jni-id-used.cc"
                    ]
                if deferred_zygote:
                    sources = [
                        source for source in sources
                        if source.name != "native-wait.cc"
                    ]
                if common_transform_capability:
                    definitions = ["-DDARWIN_ART_TI_AGENT_COMMON_TRANSFORM"]
                elif common_retransform_capability:
                    definitions = ["-DDARWIN_ART_TI_AGENT_COMMON_RETRANSFORM"]
                elif common_redefine_capability:
                    definitions = ["-DDARWIN_ART_TI_AGENT_COMMON_REDEFINE"]
                elif any(source.name == "search_onload.cc"
                         for source in sources):
                    definitions = ["-DDARWIN_ART_TI_AGENT_936"]
                elif any(source.name == "vminit.cc" for source in sources):
                    definitions = ["-DDARWIN_ART_TI_AGENT_1919"]
            sources = sorted(set(base_sources + sources))
            slicer_root = root / "_aosp/tools/dexter/slicer"
            slicer_sources: list[Path] = []
            slicer_arguments: list[str] = []
            slicer_libraries: list[str] = []
            agent_uses_dexter_slicer = any(
                b'"slicer/' in source.read_bytes() for source in sources)
            if agent_uses_dexter_slicer:
                command([str(root / "tools/sync-android16-dexter-slicer.sh")])
                slicer_sources = [
                    slicer_root / name for name in (
                        "bytecode_encoder.cc", "code_ir.cc", "common.cc",
                        "control_flow_graph.cc", "debuginfo_encoder.cc",
                        "dex_bytecode.cc", "dex_format.cc", "dex_ir.cc",
                        "dex_ir_builder.cc", "dex_utf8.cc",
                        "instrumentation.cc", "reader.cc",
                        "tryblocks_encoder.cc", "writer.cc",
                    )
                ]
                slicer_arguments = [
                    "-fno-rtti", "-Wno-sign-compare",
                    "-Wno-unused-parameter", "-Wno-shift-count-overflow",
                    "-Wno-missing-braces", "-I", str(slicer_root / "export"),
                ]
                slicer_libraries = ["-lz"]
            platform_compat = []
            agent_uses_pthread_barrier = any(
                b"pthread_barrier_t" in source.read_bytes()
                for source in sources)
            if agent_uses_pthread_barrier or use_common_load:
                platform_compat = [
                    "-include",
                    str(root / "compat/darwin_art_test_pthread_barrier.h"),
                ]
            # Some AOSP plugin fixtures provide their own exported
            # Agent_OnLoad (for example 900-hello-plugin). In that case the
            # test DSO is the agent entrypoint itself; adding the generic
            # common dispatcher would create duplicate symbols. JVMTI tests
            # whose callback is only a namespaced OnLoad method still use the
            # dispatcher below.
            has_native_agent_entry = any(
                re.search(rb"\bAgent_OnLoad\s*\(", source.read_bytes())
                for source in sources if source.is_file())
            agent_dispatch_sources = (
                [str(ti_agent / "common_load.cc")]
                if use_common_load else
                ([] if has_native_agent_entry else
                 [str(root / "probes/runtime_upstream_tiagent_dispatch.cc")])
            )
            command([
                "clang++", "-std=c++20", "-O2", "-dynamiclib",
                "-Wno-invalid-offsetof", "-Wno-unsupported-visibility",
                "-Wl,-install_name,@rpath/libtiagent.so", "-pthread",
                "-DNDEBUG", "-DART_PAGE_SIZE_AGNOSTIC", "-DBUILDING_LIBART",
                "-DART_DEFAULT_GC_TYPE_IS_CMS", "-DART_USE_READ_BARRIER",
                "-DART_READ_BARRIER_TYPE_IS_BAKER=1",
                "-DART_FORCE_USE_READ_BARRIER",
                "-DART_STACK_OVERFLOW_GAP_arm=8192",
                "-DART_STACK_OVERFLOW_GAP_arm64=8192",
                "-DART_STACK_OVERFLOW_GAP_riscv64=8192",
                "-DART_STACK_OVERFLOW_GAP_x86=8192",
                "-DART_STACK_OVERFLOW_GAP_x86_64=8192",
                "-include", "base/globals.h",
                "-include", str(root / "_build/runtime-common/patched-source/runtime/mirror/object_reference.h"),
                *definitions, *platform_compat, *slicer_arguments,
                "-I", str(root / "_aosp/art/openjdkjvmti/include"),
                "-I", str(root / "_aosp/art/test"),
                "-I", str(ti_agent),
                "-I", str(root / "_aosp/libnativehelper/include_jni"),
                "-I", str(root / "_aosp/libnativehelper/header_only_include"),
                "-I", str(root / "_aosp/libnativehelper-full/include"),
                "-I", str(root / "_aosp/system/libbase/include"),
                "-I", str(root / "_aosp/system/logging/liblog/include"),
                "-I", str(root / "_aosp/external/fmtlib/include"),
                "-I", str(root / "_build/runtime-arm64/generated"),
                "-I", str(root / "_build/runtime-common/patched-source/runtime"),
                "-I", str(root / "_build/foundation/patched-source/libartbase"),
                "-I", str(root / "_aosp/art/libartbase"),
                "-I", str(root / "_aosp/art/runtime"),
                "-I", str(root / "_aosp/art/runtime/base"),
                "-I", str(root / "_aosp/art/runtime/arch/arm64"),
                "-I", str(root / "_aosp/art/libdexfile"),
                "-I", str(root / "_aosp/art/libprofile"),
                "-I", str(root / "_aosp/art/libnativebridge/include"),
                "-I", str(root / "_aosp/external/tinyxml2"),
                "-I", str(root / "_aosp/external/dlmalloc"),
                *agent_dispatch_sources,
                *map(str, sources), *map(str, slicer_sources),
                str(root / "_build/foundation/libandroid-base-darwin.a"),
                str(root / "_build/runtime-graphics-link-probe/libdarwin_art_runtime_graphics.dylib"),
                *slicer_libraries,
                "-o", str(output),
            ])

        native_sources = sorted(set(
            list(test.rglob("*.cc")) + list(test.rglob("*.c")) +
            shared_native_sources
        )) if has_native_source else []
        if not has_native_source:
            # Some AOSP test directories (for example the secondary VDEX
            # variants) reuse a shared libarttest source owned by a sibling
            # directory. Discover that owner from the test's Java native
            # declarations and the actual Java_Main_* exports, rather than
            # maintaining a directory-name exception or changing inputs.
            declared_main_methods = set()
            for source in test.rglob("*.java"):
                if not source.is_file():
                    continue
                declared_main_methods.update(re.findall(
                    r"\bnative\s+[^;()\n]+?\s+([A-Za-z_$][A-Za-z0-9_$]*)\s*\(",
                    source.read_text(encoding="utf-8", errors="ignore"),
                ))
            bridge_source = root / "probes/runtime_upstream_arttest.cc"
            bridge_methods = {
                match.decode().split("__", 1)[0].replace("_1", "_")
                for match in re.findall(
                    rb"\bJava_Main_([A-Za-z0-9_]+)\s*\(",
                    bridge_source.read_bytes(),
                )
            }
            owner_candidates = []
            for source in (root / "_aosp/art/test").rglob("*.cc"):
                encoded = {
                    match.decode().split("__", 1)[0].replace("_1", "_")
                    for match in re.findall(
                        rb"\bJava_Main_([A-Za-z0-9_]+)\s*\(",
                        source.read_bytes(),
                    )
                }
                # A shared ART bridge already owns common Main methods such as
                # hasJit/ensureJitCompiled. They cannot identify a sibling
                # native target; require at least one declaration outside
                # that bridge-owned method set (for example the VDEX loader
                # helpers) before selecting an owner.
                overlap = len((declared_main_methods - bridge_methods) & encoded)
                if overlap:
                    owner_candidates.append((overlap, -len(encoded), str(source), source))
            if owner_candidates:
                owner_candidates.sort(reverse=True)
                # A target may consume JNI entry points from multiple sibling
                # members of AOSP's shared libarttest archive. Select every
                # owner that contributes a declared Main method instead of
                # arbitrarily keeping only the highest-overlap source.
                for _, _, _, owner in owner_candidates:
                    native_sources.append(owner)

        # AOSP native test targets may list helper implementations separately
        # from a selected source (for example classes_art.cc includes the
        # shared JVMTI helper headers). Resolve only same-basename .cc owners
        # for quoted headers, preserving the target's source ownership without
        # maintaining per-test dependency tables.
        if native_sources:
            test_source_root = root / "_aosp/art/test"
            header_owners = {
                header.name: header.with_suffix(".cc")
                for header in test_source_root.rglob("*.h")
                if header.with_suffix(".cc").is_file()
            }
            discovered_dependencies: set[Path] = set(native_sources)
            pending_sources = list(native_sources)
            while pending_sources:
                source = pending_sources.pop()
                # Native AOSP targets sometimes split one JNI owner across
                # sibling translation units in the same test directory (the
                # 912 classes target is one such target). Include those
                # target-local units before resolving their helper headers.
                # ``common`` and ``ti-agent`` are shared source pools rather
                # than one translation-unit-per-target directories; their
                # target membership is resolved through the explicit shared
                # source list/header closure below.
                if source.parent.name not in {"common", "ti-agent"}:
                    for sibling in (*source.parent.glob("*.cc"),
                                    *source.parent.glob("*.c")):
                        if sibling not in discovered_dependencies:
                            discovered_dependencies.add(sibling)
                            pending_sources.append(sibling)
                for header_name in re.findall(
                        rb"^\s*#\s*include\s*\"([^\"]+)\"",
                source.read_bytes(), re.MULTILINE):
                    owner = header_owners.get(Path(header_name.decode()).name)
                    if owner is not None and owner not in discovered_dependencies:
                        discovered_dependencies.add(owner)
                        pending_sources.append(owner)
            native_sources = sorted(discovered_dependencies)
        if cfi_contract:
            # The pinned test's JNI body is Linux process plumbing around the
            # platform-neutral AndroidUnwinder contract. Keep its Java call
            # graph and three run.py invocations unchanged while binding the
            # same contract to Mach task/thread control on Darwin.
            native_sources = [
                source for source in native_sources if source.name != "cfi.cc"
            ]
        runtime_owned_native_classes: set[str] = set()
        runtime_owned_methods = (
            rb"performHomogeneousSpaceCompact|supportHomogeneousSpaceCompact|"
            rb"objectAddress|suspendAndResume|testVisitLocks|debugPrintClass"
        )
        for source in native_sources:
            if source.name not in {
                    "debug_print_class.cc", "gc_coverage.cc",
                    "suspend_all.cc", "visit_locks.cc"}:
                continue
            # These helpers call ART's private C++ ABI. Android links them
            # beside libart; keep their implementation in the runtime image
            # on Darwin and derive the declaring Java classes from unchanged
            # JNI exports instead of keying ownership on a run-test name.
            runtime_owned_native_classes.update(
                match.decode().replace("_1", "_").replace("_", "/")
                for match in re.findall(
                    rb"Java_([A-Za-z0-9_]+)_(?:" + runtime_owned_methods + rb")",
                    source.read_bytes(),
                )
            )
        native_sources = [
            source for source in native_sources
            if source.name not in {
                "debug_print_class.cc", "gc_coverage.cc", "suspend_all.cc",
                "visit_locks.cc", "native_methods.cc", "startup_interface.cc",
                "native_shutdown.cc",
            }
        ]
        # These source owners are unique in the pinned AOSP corpus and call
        # private runtime startup/shutdown ABI.  Keep them in the runtime
        # bridge for every test that declares the capability, without a
        # directory-name exception.
        if native_attach_agent:
            # onload.cc belongs to libtiagent on Android; libarttest contains
            # only the JNI method that asks Runtime to attach that agent.
            native_sources = [
                source for source in native_sources
                if source.name != "onload.cc"
            ]
            command([str(root / "tools/sync-android16-openjdkjvmti.sh")])
            compile_tiagent(
                args.test, native_fixture_dir / "libtiagent.so",
                limited_capabilities=True,
            )
        elif deferred_zygote_agent:
            # This test attaches its JVMTI agent only after simulating zygote
            # specialization. Build it now but do not pass it as a startup
            # agent; native-wait.cc's ART-private portion lives in-runtime.
            native_sources = [
                source for source in native_sources
                if source.name != "native-wait.cc"
            ]
            command([str(root / "tools/sync-android16-openjdkjvmti.sh")])
            compile_tiagent(args.test, native_fixture_dir / "libtiagent.so",
                            deferred_zygote=True)
        elif upstream_jvmti:
            # AOSP's libtiagent owns the test's JVMTI callbacks and JNI entry
            # points. Keep libarttest limited to its separate shared helpers.
            native_sources = [
                source for source in native_sources
                if test not in source.parents
            ]
            command([str(root / "tools/sync-android16-openjdkjvmti.sh")])
            compile_tiagent(
                args.test, native_fixture_dir / "libtiagent.so",
                hello_agent=hello_tiagent_capability,
            )
        elif upstream_agent_attach:
            # Java-side VMDebug.attachAgent loads this AOSP agent after the
            # test library is initialized. Android's libtiagent target owns
            # the test's agent callback, while ordinary JNI entry points stay
            # in libarttest. Derive that ownership from the callback contract
            # rather than from a test-name exception.
            native_sources = [
                source for source in native_sources
                if not (
                    source.parent == test and
                    b"OnAttach" in source.read_bytes()
                )
            ]
            command([str(root / "tools/sync-android16-openjdkjvmti.sh")])
            compile_tiagent(
                args.test, native_fixture_dir / "libtiagent.so",
                use_common_load=True,
            )
        if uses_native_bridge:
            bridge_source = native_bridge_source
            native_sources = [
                source for source in native_sources if source != bridge_source
            ]
            native_sources.append(
                root / "_aosp/art/test/004-JniTest/jni_test.cc"
            )
            # The bridge test reuses the signal entry point from AOSP's
            # 004-SignalTest shared test library. Keep that unchanged native
            # implementation in the bridged target so the bridge's
            # getTrampoline contract resolves Java_Main_testSignal exactly
            # as it does on Android.
            native_sources.append(
                root / "_aosp/art/test/004-SignalTest/signaltest.cc"
            )
        dso_owned_main_native_methods: set[str] = set()
        if native_sources:
            if trace_v2_native:
                # dump_trace.cc consumes two private libartbase methods. The
                # Android run-test gets them from libart; keep that ownership
                # instead of linking another libartbase copy into libarttest.
                native_sources.append(
                    root / "probes/runtime_upstream_trace_file_bridge.cc"
                )
            # The runtime-side libarttest bridge supplies common ART-private
            # JNI helpers, but an unchanged test DSO may deliberately own a
            # same-named method together with module-local state. Derive
            # ownership from the final source set for this DSO, after all
            # agent/runtime ownership moves and source additions, so the
            # entire method family stays in one image.
            for source in native_sources:
                for encoded_method in re.findall(
                        rb"\bJava_Main_([A-Za-z0-9_]+)\s*\(",
                        source.read_bytes()):
                    method = encoded_method.decode().split("__", 1)[0]
                    dso_owned_main_native_methods.add(
                        method.replace("_1", "_"))
            arttest_loader = native_fixture_dir / (
                "libarttest2.so" if uses_native_bridge else "libarttest.so"
            )
            compile_arttest(
                native_sources,
                arttest_loader,
                include_loader=not uses_native_bridge,
            )
        else:
            arttest_loader = root / "_build/art-upstream-test/libarttest.so"
            arttest_loader.parent.mkdir(parents=True, exist_ok=True)
            shared_inputs = [Path(__file__).resolve(), loader_source,
                             *shared_native_sources,
                             root / "_build/foundation/libandroid-base-darwin.a"]
            # Multiple unchanged corpus tests can prepare their private
            # sandboxes concurrently, but this source-identical loader is a
            # shared cache entry. Serialize only its rebuild/atomic publish;
            # test compilation and execution remain independent.
            loader_lock = arttest_loader.with_suffix(".so.lock")
            with loader_lock.open("a+b") as lock_file:
                fcntl.flock(lock_file.fileno(), fcntl.LOCK_EX)
                if (not arttest_loader.is_file() or any(
                        arttest_loader.stat().st_mtime_ns < source.stat().st_mtime_ns
                        for source in shared_inputs)):
                    loader_temporary = arttest_loader.with_name(
                        f"{arttest_loader.name}.{os.getpid()}.tmp")
                    try:
                        compile_arttest(shared_native_sources, loader_temporary)
                        loader_temporary.replace(arttest_loader)
                    finally:
                        loader_temporary.unlink(missing_ok=True)
            (native_fixture_dir / "libarttest.so").symlink_to(arttest_loader)
        # AOSP's default_run appends every --testlib value to Main.main's
        # argv.  External test libraries are separate DSOs (not aliases of
        # the primary libarttest), so build them from the same typed library
        # list before launching the process.
        external_test_libraries = tuple(dict.fromkeys(
            library for invocation in upstream_run_invocations
            for library in invocation.test_libraries
            if library.endswith("_external")
        ))
        if external_test_libraries:
            # External test DSOs use the same target-local source ownership as
            # the primary test DSO.  AOSP targets may split their JNI entry
            # point and the ART-facing helper ABI across several translation
            # units (for example test_native.cc + libart_api.cc).  Building
            # only the conventional entry-point file leaves those helpers
            # undefined and makes the external library differ from the
            # unchanged Android target.  Derive the local source set from the
            # already-resolved native_sources list; this stays independent of
            # test names and does not pull shared runtime owners into the DSO.
            external_sources = sorted({
                source for source in native_sources if test in source.parents
            })
            if not external_sources:
                test_native_source = test / "test_native.cc"
                if test_native_source.is_file():
                    external_sources.append(test_native_source)
            external_sources.append(
                root / "_aosp/art/test/common/libarttest_external.cc")
            for library in external_test_libraries:
                compile_arttest(
                    external_sources,
                    native_fixture_dir / f"lib{library}.so",
                    include_loader=False,
                    install_name=f"lib{library}.so",
                    dependencies=(arttest_loader,),
                    rpaths=("@loader_path/../native",),
                )
            # Keep a regular private-root copy for Java's reflective
            # getLibPaths/Files.copy contract.  This is a disposable
            # per-process staging tree; the loader still uses the canonical
            # fixture image above for System.loadLibrary.
            for library in external_test_libraries:
                shutil.copyfile(
                    native_fixture_dir / f"lib{library}.so",
                    private_data / f"lib{library}.so",
                )
        if uses_native_bridge:
            # Compile the unchanged bridge implementation. Filesystem
            # placeholders and aliases are staged below in DEX_LOCATION,
            # matching run.py's pre-launch `touch`/`ln -sf` operations.
            bridge = native_fixture_dir / "libnativebridgetest.so"
            compile_native_bridge(test / "nativebridge.cc", bridge)
            (native_fixture_dir / "libnativebridgetestd.so").symlink_to(
                bridge.name)
            # Keep the loader-facing SONAME invalid in the fixture directory
            # too: otherwise Darwin's native loader sees the host dylib and
            # bypasses NativeBridge before the bridge can rewrite the path.
            (native_fixture_dir / "libarttest.so").write_bytes(b"")
        if native_bridge_libraries:
            # AOSP's run.py makes NativeBridge and the invalid library names
            # visible from the process-private DEX_LOCATION. Do this from the
            # parsed -XX:NativeBridge= contract, not from a test-name branch.
            for bridge_name in native_bridge_libraries:
                if (Path(bridge_name).name != bridge_name or
                        bridge_name in {".", ".."}):
                    raise RuntimeError(
                        "NativeBridge library must be a private basename: "
                        f"{bridge_name}")
                source_bridge = native_fixture_dir / bridge_name
                if not source_bridge.is_file():
                    raise RuntimeError(
                        "NativeBridge library was not staged: "
                        f"{source_bridge}")
                (private_data / bridge_name).symlink_to(source_bridge)
            for placeholder in (
                    "libarttest.so", "libarttestd.so", "libinvalid.so"):
                (private_data / placeholder).touch()
            # nativebridge.cc transforms the requested pathname suffix from
            # libarttest.so to libarttest2.so. Keep the process-side names in
            # the Android run.py namespace and symlink them to the compiled
            # fixture image rather than copying it.
            if not arttest_loader.is_file():
                raise RuntimeError(
                    "NativeBridge target image was not staged: "
                    f"{arttest_loader}")
            (private_data / "libarttest2.so").symlink_to(arttest_loader)
            (private_data / "libarttestd2.so").symlink_to(arttest_loader)
        expected_root = source_test_root if generated_build else test
        expected_stdout_path = expected_root / "expected-stdout.txt"
        expected_stdout = expected_stdout_path.read_bytes()
        expected_stderr = (expected_root / "expected-stderr.txt").read_bytes()
        expected_stdout_baseline = expected_stdout
        expected_stderr_baseline = expected_stderr
        inherits_process_stdout = upstream_argument0 == "arttest"
        modes = ("jit",) if cfi_contract else ("interpreter", "jit")
        for mode in modes:
            # Each differential variant is a separate Android process family.
            # Keep its writable DEX_LOCATION/cwd and typed-action root private
            # so an unchanged test that creates a fixed-name file (for
            # example Main.createNativeLibCopy) cannot collide with the other
            # variant. Immutable jars and compiled native fixtures remain
            # shared outside this root.
            mode_private_data = temporary / f"{mode}-sandbox"
            mode_private_data.mkdir()
            for seed in private_data.iterdir():
                destination = mode_private_data / seed.name
                if seed.is_dir() and not seed.is_symlink():
                    shutil.copytree(seed, destination, dirs_exist_ok=True)
                elif seed.is_symlink():
                    destination.symlink_to(seed.readlink())
                else:
                    shutil.copy2(seed, destination)
            mode_native_fixture_dir = native_fixture_dir
            if native_bridge_libraries:
                # NativeBridge run contracts create process-private aliases
                # from the Android native-test directory into DEX_LOCATION.
                # Keep both ends of those typed filesystem actions within the
                # mode sandbox, while retaining the canonical compiled fixture
                # directory as the immutable build output shared by modes.
                mode_native_fixture_dir = mode_private_data / "native-fixtures"
                mode_native_fixture_dir.mkdir()
                for fixture in native_fixture_dir.iterdir():
                    destination = mode_native_fixture_dir / fixture.name
                    if fixture.is_dir() and not fixture.is_symlink():
                        shutil.copytree(fixture, destination)
                    else:
                        shutil.copy2(fixture, destination)
                # The host fixture directory reserves libarttest.so as the
                # invalid process-side name so the ordinary loader falls
                # through to NativeBridge. AOSP's native-test directory still
                # exposes the real library under that basename; run.py then
                # creates the translated libarttest2 alias in DEX_LOCATION.
                shutil.copy2(
                    arttest_loader, mode_native_fixture_dir / "libarttest.so")
                shutil.copy2(
                    arttest_loader, mode_native_fixture_dir / "libarttestd.so")
            # Main.java and other unchanged AOSP tests address auxiliary
            # DEX/JAR inputs through their DEX_LOCATION basename. Stage
            # private copies for that Android pathname contract; dex2oat and
            # host loading continue to use the shared immutable artifacts.
            for staged_jar in (test_jar, secondary_jar):
                if staged_jar.is_file():
                    shutil.copy2(staged_jar, mode_private_data / staged_jar.name)
            # The second/default build phase can create the resource tree
            # after the initial source-resource staging pass. Materialize
            # that build-owned tree directly into this mode's DEX_LOCATION.
            built_resources = temporary / "res"
            if built_resources.is_dir():
                shutil.copytree(
                    built_resources, mode_private_data / "res",
                    dirs_exist_ok=True)
            # Differential modes are independent Android process families.
            # Preserve a runtime image between repeated invocations inside one
            # mode (generate, then reload), but never let the interpreter run
            # satisfy the JIT run's generation contract.
            runtime_image = temporary / "arm64" / f"{args.test}.art"
            runtime_image.unlink(missing_ok=True)
            stdout_path = mode_private_data / f"{mode}.stdout"
            stderr_path = mode_private_data / f"{mode}.stderr"
            mode_staged_expected_stdout = mode_private_data / "expected-stdout.txt"
            mode_staged_expected_stderr = mode_private_data / "expected-stderr.txt"
            mode_staged_expected_stdout.write_bytes(expected_stdout_baseline)
            mode_staged_expected_stderr.write_bytes(expected_stderr_baseline)
            # Keep immutable AOSP expected-stdout siblings addressable under
            # this private root for typed ctx.expected_stdout.with_suffix().
            for expected_candidate in expected_root.glob("expected-stdout*"):
                if expected_candidate.is_file():
                    (mode_private_data / expected_candidate.name).write_bytes(
                        expected_candidate.read_bytes())
            if run_contract is not None:
                mode_context = run_contract_context(
                    args.test, mode_private_data, mode_native_fixture_dir, stdout_path,
                    stderr_path, mode_staged_expected_stdout)
                mode_plan = evaluate_run_plan(run_contract, mode_context, mode)
                if not mode_plan.supported:
                    raise RuntimeError(
                        f"run.py ActionPlan unsupported: {mode_plan.as_dict()}")
                run_invocations = plan_invocations(mode_plan)
                ordered_actions = mode_plan.actions
                selected_expected_stdout = getattr(
                    mode_plan, "final_expected_stdout", None)
                if selected_expected_stdout is None:
                    selected_expected_path = mode_staged_expected_stdout
                else:
                    selected_expected_path = Path(selected_expected_stdout)
                    try:
                        selected_expected_path.relative_to(mode_private_data)
                    except ValueError as error:
                        raise RuntimeError(
                            "run.py selected expected stdout outside its "
                            f"staging root: {selected_expected_path}") from error
                    if not selected_expected_path.is_file():
                        raise RuntimeError(
                            "run.py selected an expected stdout file that "
                            f"was not staged: {selected_expected_path}")
            else:
                # Native verifier tests that inspect an OAT quick entrypoint
                # require the normal speed prebuild.  Infer this from the
                # test's native contract rather than from a corpus name.
                requires_aot = (
                    b"GetOatMethodQuickCode(" in test_capability_text
                    # Reference-map visitors inspect compiled Java frames;
                    # running this contract against a verify-only VDEX leaves
                    # those frames interpreted and makes the visitor
                    # incorrectly report no map.  Infer the normal AOSP
                    # speed prebuild from the ABI the native test consumes,
                    # never from a test-directory name.
                    or b"CheckReferenceMapVisitor" in test_capability_text
                )
                run_invocations = [implicit_default_invocation(
                    mode, require_aot=requires_aot)]
                ordered_actions = [{"kind": "default_run", "args_snapshot": {}, "kwargs": []}]
                selected_expected_path = mode_staged_expected_stdout
            # Keep the oat/vdex beside the mode-local JAR and compile it with
            # that exact textual dex-location.  ART's OatFile identity
            # includes the location, so copying the outer build outputs
            # (compiled against ``temporary/...jar``) is insufficient.
            mode_oat_dir = mode_private_data / "oat/arm64"
            mode_oat_dir.mkdir(parents=True, exist_ok=True)
            mode_first_invocation = run_invocations[0]

            def enforce_secondary_compilation_boundary(
                    invocation: RunInvocation) -> None:
                """Keep disabled secondary compilation out of the VM view.

                AOSP's default_run does not invoke dex2oat for ``-ex.jar``
                when ``secondary_compilation`` is false.  The detached
                launcher exposes the mode oat directory through its narrow
                runtime-file capability, so stale secondary outputs must not
                leak through that boundary between typed actions.
                """
                if invocation.secondary_compilation:
                    return
                for suffix in (".art", ".odex", ".vdex"):
                    (mode_oat_dir / f"{args.test}-ex{suffix}").unlink(
                        missing_ok=True)

            enforce_secondary_compilation_boundary(mode_first_invocation)
            if mode_first_invocation.vdex:
                build_vdex_contract(
                    mode_first_invocation,
                    dex_input=mode_private_data / test_jar.name,
                    oat_directory=mode_oat_dir)
                current_vdex_contract = (
                    mode_first_invocation.compiler_options,
                    mode_first_invocation.compiler_only_options,
                    mode_first_invocation.vdex_filter,
                )
            elif not (mode_first_invocation.dex2oat_dm or
                      mode_first_invocation.runtime_dm):
                build_app_contract(
                    mode_first_invocation,
                    dex_input=mode_private_data / test_jar.name,
                    oat_directory=mode_oat_dir,
                    secondary_input=(mode_private_data / secondary_jar.name
                                     if secondary_jar.is_file() else None),
                    staging_root=mode_private_data)
                current_app_contract = (
                    mode_first_invocation.prebuild,
                    mode_first_invocation.app_image,
                    mode_first_invocation.profile,
                    mode_first_invocation.compiler_options,
                    mode_first_invocation.compiler_only_options,
                    mode_first_invocation.secondary,
                    mode_first_invocation.secondary_app_image,
                    mode_first_invocation.secondary_compilation,
                    mode_first_invocation.secondary_class_loader_context,
                    mode_first_invocation.verify,
                    mode_first_invocation.verify_soft_fail,
                    mode_first_invocation.image,
                    mode_first_invocation.relocate,
                    mode_first_invocation.dex2oat_timeout,
                    mode_first_invocation.dex2oat_rt_timeout,
                )
            host_log = temporary / f"{mode}.host.log"
            environment = os.environ.copy()
            existing_dyld_path = environment.get("DYLD_LIBRARY_PATH")
            def visible_mode_oat_files(invocation: RunInvocation) -> list[Path]:
                # AOSP's secondary_compilation=false contract deliberately
                # exposes no prebuilt secondary VDEX to the verifier. Keep
                # the mode-local staging directory honest even when a later
                # invocation in the same run will create that artifact.
                files = list(mode_oat_dir.iterdir())
                if invocation.secondary_compilation:
                    return files
                return [path for path in files if "-ex." not in path.name]
            environment.update({
                "DARWIN_ART_UPSTREAM_MAIN": "Main",
                "DARWIN_ART_UPSTREAM_STDOUT": str(stdout_path),
                "DARWIN_ART_UPSTREAM_STDERR": str(stderr_path),
                # The upstream run-test corpus is trusted AOSP input. Give its
                # Android process an explicit host-root authority so libcore
                # can open the absolute boot-class-path URLs supplied by this
                # harness. Production APK launchers instead provide their
                # private staged Android filesystem root.
                "DARWIN_ART_ANDROID_FILESYSTEM_ROOT": "/",
                # AOSP run-test owns the complete DEX_LOCATION staging tree:
                # generated profiles, runtime app images and /data scratch all
                # live under this per-test sandbox. Production launchers still
                # pass their package-specific private data directory instead.
                "DARWIN_ART_ANDROID_PRIVATE_DATA_ROOT": str(mode_private_data),
                # Android's installed APK/JAR paths are visible to both Java
                # File and ART's DexFile loader. These test artifacts live in
                # a host temp directory, so grant the exact immutable files to
                # the same narrow runtime-file capability used by installed
                # packages; do not expose the containing host directory.
                "DARWIN_ART_RUNTIME_HOST_FILES":
                    ":".join(map(str, [
                        *runtime_host_files,
                        # The Android run-test contract names its primary
                        # DexFile through DEX_LOCATION. The staged copy is
                        # therefore also a permitted immutable runtime input.
                        mode_private_data / test_jar.name,
                    ])),
                "DARWIN_ART_APK_APP_NATIVE_PATH": str(native_fixture_dir),
                "DARWIN_ART_APK_APP_NATIVE_DIR": str(native_fixture_dir),
                # ART run-test's shared libarttest is a platform test library
                # visible from the BootClassLoader namespace on Android.
                "DARWIN_ART_ANDROID_SYSTEM_NATIVE_DIR": str(native_fixture_dir),
                "DARWIN_ART_APK_MANAGED_NATIVE_LOAD": "1",
                # NativeBridge's run.py rewrites java.library.path to
                # DEX_LOCATION after creating its aliases. Preserve the
                # ordinary fixture search path for all other tests.
                "DARWIN_ART_JAVA_LIBRARY_PATH": str(
                    mode_private_data if (native_bridge_libraries or
                                     external_test_libraries)
                    else native_fixture_dir),
                # Android's device run-test launcher supplies this property at
                # VM creation time. Keep the guest pathname and back it with
                # the test process's private writable /data capability.
                "DARWIN_ART_JAVA_IO_TMPDIR": "/data/local/tmp",
                "DEX_LOCATION": str(mode_private_data),
                "ANDROID_I18N_ROOT": str(root / "_build/icu-runtime-adapters/runtime/i18n"),
                "ANDROID_DATA": str(root / "_build/icu-runtime-adapters/runtime/data"),
                "ANDROID_TZDATA_ROOT": str(root / "_build/icu-runtime-adapters/runtime/tzdata"),
                "DARWIN_ART_UPSTREAM_TEST_NAME": args.test,
                # AOSP run-test exposes its native test directory as the
                # process java.library.path. The Darwin libcore property
                # derives that value from DYLD_LIBRARY_PATH; keeping the test
                # directory first also gives platform helpers such as
                # ../bin/hprof-conv the same layout they have on-device.
                "DYLD_LIBRARY_PATH": str(native_fixture_dir) + (
                    f":{existing_dyld_path}" if existing_dyld_path else ""),
                "DARWIN_ART_BOOT_CLASSPATH_LOCATIONS": ":".join(
                    # BootClassPathLocations are consumed by libcore's
                    # ClassPathURLStreamHandler and are also the logical names
                    # recorded by the pinned boot image. Keep these relative
                    # to Android's root while BootClassPath itself retains
                    # the absolute host paths needed by OpenBootDexFiles.
                    str(path.relative_to(root)) for path in [
                        *boot_image_class_path,
                        root / "_build/dex-probe/unsafe-boot-dex/unsafe-boot.jar",
                    ]
                ),
                # Zygote startup keeps ART's trusted-oat invariant keyed to
                # Android's logical image location. The detached host opens
                # the exact generated component files through the runtime's
                # BCP FD contract; keep the complete BCP aligned with those
                # component indices, including the trailing unsafe test DEX.
                "DARWIN_ART_BOOT_CLASSPATH": ":".join(
                    str(path) for path in [
                        *boot_image_class_path,
                        root / "_build/dex-probe/unsafe-boot-dex/unsafe-boot.jar",
                    ]
                ),
                "DARWIN_ART_BOOT_IMAGE_FD_ROOT": str(boot_image.parent / "arm64"),
            })
            # Corpus prebuilt jars receive the generic launcher as an appended
            # multidex entry. They are not installed APK applications, so the
            # APK identity environment must not be activated merely because
            # the support DEX is present; that mode requires package/activity
            # and resource descriptors that run-test does not provide.
            if prebuilt_test_jar is not None:
                environment.pop("DARWIN_ART_APK_APP_SUPPORT_DEX", None)
            environment["DARWIN_ART_RUNTIME_HOST_FILES"] = ":".join(
                map(str, [
                    # libcore reopens BootClassPathLocations for resource
                    # lookup. Authorize every physical BCP entry so the
                    # aligned logical-to-physical contract remains complete.
                    mode_private_data / test_jar.name,
                    *dex2oat_boot_class_path,
                        *visible_mode_oat_files(mode_first_invocation),
                ]))
            if runtime_owned_native_classes:
                environment["DARWIN_ART_UPSTREAM_RUNTIME_NATIVE_CLASSES"] = \
                    ":".join(sorted(runtime_owned_native_classes))
            if dso_owned_main_native_methods:
                environment["DARWIN_ART_UPSTREAM_DSO_MAIN_NATIVE_METHODS"] = \
                    ":".join(sorted(dso_owned_main_native_methods))
            if inherits_process_stdout:
                environment["DARWIN_ART_UPSTREAM_INHERIT_STDOUT"] = "1"
            if upstream_argument0 is not None:
                environment["DARWIN_ART_UPSTREAM_ARG0"] = upstream_argument0
            if deferred_zygote_agent:
                environment["DARWIN_ART_UPSTREAM_ZYGOTE"] = "1"
                environment["DARWIN_ART_UPSTREAM_DEFERRED_JVMTI_AGENT"] = \
                    f"{native_fixture_dir / 'libtiagent.so'}={args.test},art"
            if metrics_contract:
                environment["DARWIN_ART_UPSTREAM_METRICS"] = "1"
            if upstream_target_sdk is not None:
                environment["DARWIN_ART_RUNTIME_TARGET_SDK_VERSION"] = \
                    upstream_target_sdk
            if upstream_finalizer_timeout is not None:
                environment["DARWIN_ART_RUNTIME_FINALIZER_TIMEOUT_MS"] = \
                    upstream_finalizer_timeout
            if swappable_jni_contract:
                environment["DARWIN_ART_UPSTREAM_SWAPPABLE_JNI_IDS"] = "1"
            if native_attach_agent:
                environment["DARWIN_ART_UPSTREAM_JVMTI"] = "1"
            combined_stdout = bytearray()
            combined_stderr = bytearray()
            current_output = False
            current_host_log_is_stderr = False
            # Darwin's Android logging facade writes logcat records to the
            # process host stream.  AOSP's default_run, however, exposes that
            # stream as stderr_file to its post-action (for example the
            # metrics tests filter it with sed).  Keep a byte cursor so each
            # invocation contributes its records exactly once; this matters
            # for ordered multi-invocation contracts and for background
            # reporting threads whose final records arrive at process exit.
            host_log_cursor = 0

            def flush_current_output() -> None:
                nonlocal current_output, current_host_log_is_stderr, host_log_cursor
                if not current_output:
                    return
                combined_stdout.extend(stdout_path.read_bytes())
                if stderr_path.is_file():
                    combined_stderr.extend(stderr_path.read_bytes())
                if metrics_contract or current_host_log_is_stderr:
                    host_log_cursor = append_new_host_log_bytes(
                        host_log, host_log_cursor, combined_stderr)
                else:
                    # Do not let records from an ordinary invocation leak
                    # into a later invocation that explicitly switches ART's
                    # logger to stderr.
                    host_log_cursor = len(host_log.read_bytes())
                current_output = False
                current_host_log_is_stderr = False

            with host_log.open("wb") as log:
                run_index = 0
                for action in ordered_actions:
                    if action.get("kind") == "echo":
                        flush_current_output()
                        values = action.get("args", [])
                        if len(values) != 1 or not isinstance(values[0], str):
                            raise RuntimeError("ctx.echo ActionPlan value is not a string")
                        combined_stdout.extend(values[0].encode() + b"\n")
                        continue
                    if action.get("kind") == "shell":
                        # AOSP's post-processing commands address the shared
                        # stdout/stderr files assembled from all invocations
                        # seen so far. Materialize that aggregate before the
                        # typed action, then carry any edits back into the
                        # aggregate so later invocations append after them.
                        flush_current_output()
                        stdout_path.write_bytes(combined_stdout)
                        stderr_path.write_bytes(combined_stderr)
                        apply_typed_shell_action(action, mode_private_data)
                        if stdout_path.is_file():
                            combined_stdout = bytearray(stdout_path.read_bytes())
                        if stderr_path.is_file():
                            combined_stderr = bytearray(stderr_path.read_bytes())
                        continue
                    if action.get("kind") == "export":
                        export_name = action.get("name")
                        export_value = action.get("value")
                        if (not isinstance(export_name, str) or
                                not isinstance(export_value, str)):
                            raise RuntimeError(
                                "export ActionPlan requires string name/value")
                        environment[export_name] = export_value
                        continue
                    if action.get("kind") != "default_run":
                        raise RuntimeError(
                            f"unsupported ActionPlan action: {action.get('kind')}")
                    flush_current_output()
                    # Use the already-materialized invocation for this action.
                    # This is also the source of truth for the synthesized
                    # no-run.py default, whose AOSP verify-only policy is not
                    # encoded in an on-disk ActionPlan.
                    run_invocation = run_invocations[run_index]
                    env_snapshot = action.get("env_snapshot", {})
                    if not isinstance(env_snapshot, dict):
                        raise RuntimeError(
                            "default_run ActionPlan has malformed env snapshot")
                    for env_name, env_value in env_snapshot.items():
                        if (not isinstance(env_name, str) or
                                not isinstance(env_value, str)):
                            raise RuntimeError(
                                "default_run ActionPlan env snapshot must be "
                                "string-valued")
                        environment[env_name] = env_value
                    if run_invocation.lib not in {None, "", "libart.so"}:
                        raise RuntimeError(
                            "unsupported ART runtime library selection: "
                            f"{run_invocation.lib}")
                    if run_invocation.jvmti:
                        environment["DARWIN_ART_UPSTREAM_JVMTI_AGENT"] = \
                            f"{native_fixture_dir / 'libtiagent.so'}={args.test},art"
                    else:
                        environment.pop(
                            "DARWIN_ART_UPSTREAM_JVMTI_AGENT", None)
                    if run_invocation.vdex:
                        environment["DARWIN_ART_UPSTREAM_VDEX"] = "1"
                    else:
                        environment.pop("DARWIN_ART_UPSTREAM_VDEX", None)
                    if (upstream_debuggable or run_invocation.jvmti):
                        environment["DARWIN_ART_RUNTIME_JAVA_DEBUGGABLE"] = "1"
                    else:
                        environment.pop(
                            "DARWIN_ART_RUNTIME_JAVA_DEBUGGABLE", None)
                    if run_invocation.vdex:
                        requested_vdex_contract = (
                            run_invocation.compiler_options,
                            run_invocation.compiler_only_options,
                            run_invocation.vdex_filter,
                        )
                        if requested_vdex_contract != current_vdex_contract:
                            build_vdex_contract(
                                run_invocation,
                                dex_input=mode_private_data / test_jar.name,
                                oat_directory=mode_oat_dir)
                            current_vdex_contract = requested_vdex_contract
                    elif (run_invocation.dex2oat_dm or
                          run_invocation.runtime_dm):
                        requested_dm_contract = (
                            run_invocation.dex2oat_dm,
                            run_invocation.runtime_dm,
                            run_invocation.app_image,
                            run_invocation.compiler_options,
                            run_invocation.compiler_only_options,
                        )
                        if requested_dm_contract != current_dm_contract:
                            build_dm_contract(run_invocation)
                            current_dm_contract = requested_dm_contract
                    elif not (run_invocation.dex2oat_dm or
                              run_invocation.runtime_dm):
                        requested_app_contract = (
                            run_invocation.prebuild,
                            run_invocation.app_image,
                            run_invocation.profile,
                            run_invocation.compiler_options,
                            run_invocation.compiler_only_options,
                            run_invocation.secondary,
                            run_invocation.secondary_app_image,
                            run_invocation.secondary_compilation,
                            run_invocation.secondary_class_loader_context,
                            run_invocation.verify,
                            run_invocation.verify_soft_fail,
                            run_invocation.image,
                            run_invocation.relocate,
                            run_invocation.dex2oat_timeout,
                            run_invocation.dex2oat_rt_timeout,
                        )
                        if requested_app_contract != current_app_contract:
                            build_app_contract(
                                run_invocation,
                                dex_input=mode_private_data / test_jar.name,
                                oat_directory=mode_oat_dir,
                                secondary_input=(
                                    mode_private_data / secondary_jar.name
                                    if secondary_jar.is_file() else None),
                                staging_root=mode_private_data)
                            environment["DARWIN_ART_RUNTIME_HOST_FILES"] = ":".join(
                                map(str, [
                                    mode_private_data / test_jar.name,
                                    *dex2oat_boot_class_path,
                                    *visible_mode_oat_files(run_invocation),
                                ]))
                            current_app_contract = requested_app_contract
                    # Reassert the per-action staging boundary after all build
                    # transitions, including actions whose app contract did
                    # not change. This keeps disabled secondary compilation
                    # from inheriting a prior action's secondary artifacts.
                    enforce_secondary_compilation_boundary(run_invocation)
                    environment["DARWIN_ART_RUNTIME_HOST_FILES"] = ":".join(
                        map(str, [
                            mode_private_data / test_jar.name,
                            *dex2oat_boot_class_path,
                            *mode_oat_dir.iterdir(),
                        ]))
                    invocation_environment = environment.copy()
                    # Deferred zygote tests (for example 2031) do not encode
                    # -Xzygote in the typed invocation itself: they enter the
                    # same ART zygote state through the native specialization
                    # contract. Keep the logical boot-image/physical-FD
                    # namespace for that process as well, otherwise ART sees
                    # a trusted zygote but registers the host backing oat
                    # location and rejects it. The same namespace is required
                    # by the explicit system-oat-only contract: host backing
                    # files must not make the boot oat's guest location appear
                    # to live outside Android's /system tree.
                    invocation_is_zygote = (
                        run_invocation.zygote or deferred_zygote_agent)
                    invocation_requires_system_oat_locations = (
                        "-Xonly-use-system-oat-files" in
                        expand_aosp_runtime_options(tuple([
                            *run_invocation.runtime_options,
                            *run_invocation.android_runtime_options,
                        ]))
                    )
                    invocation_uses_logical_boot_namespace = (
                        invocation_is_zygote or
                        invocation_requires_system_oat_locations)
                    if not invocation_uses_logical_boot_namespace:
                        # Keep the physical BCP paths absolute for libcore's
                        # resource opens.  The logical locations were staged
                        # above relative to the runner root and must remain
                        # so: boot.art and app OAT record those Android
                        # locations, and replacing them with host paths makes
                        # ART reject both images as boot-image-out-of-date.
                        invocation_environment.pop("DARWIN_ART_BOOT_IMAGE_FD_ROOT", None)
                    # AOSP's diff-min-log-tag filters logcat mixed into the
                    # dalvikvm stderr stream. Darwin keeps Android logging in
                    # host_log and captures Java stderr independently, so the
                    # requested threshold is already stronger by construction.
                    invocation_environment[
                        "DARWIN_ART_UPSTREAM_DIFF_MIN_LOG_TAG"] = \
                        run_invocation.diff_min_log_tag
                    enable_jit = mode == "jit" or run_invocation.jit
                    invocation_environment["DARWIN_ART_JIT"] = \
                        "1" if enable_jit else "0"
                    if enable_jit:
                        invocation_environment["DARWIN_ART_JIT_TRACE"] = "1"
                    else:
                        invocation_environment.pop("DARWIN_ART_JIT_TRACE", None)
                    if run_invocation.android_log_tags is not None:
                        invocation_environment["ANDROID_LOG_TAGS"] = \
                            run_invocation.android_log_tags
                    invocation_environment["DARWIN_ART_UPSTREAM_MAIN"] = \
                        run_invocation.main_class or "Main"
                    runtime_options = [
                        option for option in run_invocation.runtime_options
                        # Differential mode owns JIT enablement explicitly.
                        # Preserve every other literal option per invocation.
                        if not option.startswith("-Xusejit:") and
                        option != "-Xzygote"
                    ]
                    # Keep AOSP's runtime_option and android_runtime_option
                    # distinct in the ActionPlan/RunInvocation.  This host
                    # consumes both at the final ART runtime boundary.
                    runtime_options.extend(run_invocation.android_runtime_options)
                    # AOSP's shell launcher expands environment references in
                    # runtime options while constructing the dalvikvm command.
                    # ActionPlan carries the original literal, so perform the
                    # same expansion at this typed boundary.  Keep this
                    # generic for the staging variable rather than teaching
                    # individual tests about paths.
                    dex_location = invocation_environment["DEX_LOCATION"]
                    runtime_options = [
                        option.replace("${DEX_LOCATION}", dex_location)
                        .replace("$DEX_LOCATION", dex_location)
                        for option in runtime_options
                    ]
                    # AOSP's shell launcher expands each runtime_option into
                    # the assembled dalvikvm command. Apply the same typed
                    # word boundary so an option containing multiple words
                    # (for example -Xcompiler-option plus its value) reaches
                    # ART as separate argv elements.
                    runtime_options = list(expand_aosp_runtime_options(
                        tuple(runtime_options)))
                    # Android's run-test names the test JVMTI plugin
                    # libartagent(.d).so. Darwin builds that unchanged test
                    # plugin as libtiagent.so; preserve the plugin lifecycle
                    # (ArtPlugin_Initialize/Deinitialize) instead of merely
                    # dropping the Android alias.
                    runtime_options = [
                        ("-Xplugin:" + str(native_fixture_dir / "libtiagent.so"))
                        if option.startswith("-Xplugin:libartagent") else option
                        for option in runtime_options
                    ]
                    runtime_options = [
                        ("-agentpath:" + str(native_fixture_dir / "libtiagent.so") +
                         option[len("-agentpath:libartagent.so"):])
                        if option.startswith("-agentpath:libartagent.so") else
                        ("-agentpath:" + str(native_fixture_dir / "libtiagent.so") +
                         option[len("-agentpath:libartagentd.so"):])
                        if option.startswith("-agentpath:libartagentd.so") else
                        option
                        for option in runtime_options
                    ]
                    if args.gcstress:
                        runtime_options.extend((
                            "-Xgc:gcstress",
                            "-Xms2m",
                            "-Xmx16m",
                        ))
                    if run_invocation.zygote:
                        runtime_options.append("-Xzygote")
                    runtime_options.append(
                        "-Xrelocate" if run_invocation.relocate
                        else "-Xnorelocate")
                    if run_invocation.verify_soft_fail:
                        runtime_options.append("-Xverify:softfail")
                    elif not run_invocation.verify:
                        runtime_options.append("-Xverify:none")
                    if os.environ.get("DARWIN_ART_UPSTREAM_AOT_TRACE") == "1":
                        runtime_options.append("-verbose:oat")
                    for compiler_option in run_invocation.compiler_options:
                        runtime_options.extend((
                            "-Xcompiler-option",
                            compiler_option,
                        ))
                    # AOSP's default_run always passes its selected boot
                    # image path to dalvikvm.  The image-dex2oat runtime
                    # switch decides whether that image may be used or
                    # generated; it is not a reason to omit -Ximage when an
                    # invocation has prebuild disabled.
                    if (run_invocation.image and
                            "-Xnoimage-dex2oat" not in runtime_options):
                        image_location = (
                            "/system/framework/boot.art"
                            if invocation_uses_logical_boot_namespace
                            else str(boot_image)
                        )
                        runtime_options.append(f"-Ximage:{image_location}")
                    if runtime_options:
                        invocation_environment["DARWIN_ART_RUNTIME_OPTIONS"] = \
                            "\n".join(runtime_options)
                    else:
                        invocation_environment.pop(
                            "DARWIN_ART_RUNTIME_OPTIONS", None)
                    run_arguments = ([upstream_argument0]
                                     if upstream_argument0 is not None else [])
                    if upstream_argument0 is not None:
                        # default_run.py appends testlib entries after the
                        # launcher-supplied first library argument.  Preserve
                        # their order and duplicates from the ActionPlan.
                        run_arguments.extend(
                            run_invocation.test_libraries[1:])
                    if run_invocation.add_libdir_argument:
                        run_arguments.append(str(native_fixture_dir))
                    run_arguments.extend(
                        expand_aosp_test_args(run_invocation.test_args))
                    invocation_stdout = stdout_path
                    invocation_stderr = stderr_path
                    invocation_environment["DARWIN_ART_UPSTREAM_STDOUT"] = \
                        str(invocation_stdout)
                    invocation_environment["DARWIN_ART_UPSTREAM_STDERR"] = \
                        str(invocation_stderr)
                    invocation_host_arguments = list(host_arguments)
                    # Match AOSP's DALVIKVM_CLASSPATH, which is rooted at
                    # DEX_LOCATION. This keeps VMRuntime.registerAppInfo's
                    # code path equal to the loaded DexFile location (needed
                    # by ProfileSaver), while retaining the immutable build
                    # artifacts in the outer staging directory.
                    invocation_host_arguments[-1] = str(
                        mode_private_data / test_jar.name)
                    if run_invocation.secondary and secondary_jar.is_file():
                        invocation_host_arguments[-1] = (
                            f"{mode_private_data / test_jar.name}:"
                            f"{mode_private_data / secondary_jar.name}")
                    if run_arguments:
                        for argument_index in range(64):
                            invocation_environment.pop(
                                f"DARWIN_ART_UPSTREAM_ARG{argument_index}", None)
                        for argument_index, value in enumerate(run_arguments):
                            invocation_environment[
                                f"DARWIN_ART_UPSTREAM_ARG{argument_index}"
                            ] = value
                    if run_invocation.sync:
                        os.sync()
                    if inherits_process_stdout:
                        # ART run-test captures the process stdout stream,
                        # including Java System.out and native std::cout.
                        with invocation_stdout.open(
                                "wb", buffering=0) as native_stdout:
                            completed = run_process_group(
                                invocation_host_arguments,
                                check=False,
                                env=invocation_environment,
                                stdout=native_stdout, stderr=log,
                                timeout=1200 if args.gcstress else 120,
                                cwd=root)
                    else:
                        completed = run_process_group(
                            invocation_host_arguments,
                            check=False,
                            env=invocation_environment,
                            stdout=log, stderr=subprocess.STDOUT,
                            timeout=1200 if args.gcstress else 120,
                            cwd=root)
                    if completed.returncode != run_invocation.expected_exit_code:
                        # Keep native crash/JVMTI diagnostics visible. The
                        # detached host writes its signal report to this log,
                        # and a bare exit status is otherwise insufficient
                        # once the temporary invocation directory is removed.
                        log.flush()
                        diagnostic_tail = host_log.read_bytes().splitlines()[-40:]
                        if diagnostic_tail:
                            print(
                                "ART upstream host diagnostics (tail):\n"
                                + b"\n".join(diagnostic_tail).decode(
                                    "utf-8", errors="replace"),
                                file=sys.stderr,
                            )
                        raise subprocess.CalledProcessError(
                            completed.returncode, invocation_host_arguments)
                    current_host_log_is_stderr = (
                        "-Xuse-stderr-logger" in runtime_options)
                    current_output = True
                    run_index += 1
                flush_current_output()
            stdout_path.write_bytes(combined_stdout)
            stderr_path.write_bytes(combined_stderr)
            actual_stdout = stdout_path.read_bytes()
            actual_stderr = stderr_path.read_bytes()
            # The expected file is part of evaluated run.py state. Read it
            # after ordered typed file actions have completed; no text-based
            # post-processing or test-name exception is applied here.
            expected_stdout = selected_expected_path.read_bytes()
            expected_stderr = mode_staged_expected_stderr.read_bytes()
            if actual_stdout != expected_stdout or actual_stderr != expected_stderr:
                raise RuntimeError(
                    f"{mode} output mismatch: stdout={len(actual_stdout)}/"
                    f"{len(expected_stdout)} stderr={len(actual_stderr)}/"
                    f"{len(expected_stderr)} artifacts={temporary}")
            if (
                mode == "jit"
                and not any(marker in host_log.read_bytes() for marker in (
                    b"optimized application method installed",
                    b"AOT application method selected",
                ))
            ):
                raise RuntimeError(f"optimized Main installation missing: {host_log}")
            print(f"ART upstream {args.test}: {mode} expected-output PASS")
        if cfi_contract:
            print("ART upstream 137-cfi: pinned Java call graph and three JIT CFI runs PASS")
        else:
            print(f"ART upstream {args.test}: unmodified source interpreter+optimized PASS")
        return 0
    finally:
        if args.keep:
            print(f"artifacts={temporary}")
        else:
            shutil.rmtree(temporary)


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (subprocess.CalledProcessError, subprocess.TimeoutExpired, RuntimeError) as error:
        print(f"ART upstream test failed: {error}", file=sys.stderr)
        raise SystemExit(1)
