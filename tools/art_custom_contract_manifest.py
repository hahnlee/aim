#!/usr/bin/env python3
"""Audit the complete numeric ART custom-script contract corpus.

The three script frontends are deliberately kept separate: this module is
only their deterministic manifest/audit boundary.  It reads source bytes,
compiles them into typed IR, and never imports or executes an AOSP script.
Unknown entrypoints, unsupported syntax, omitted directories, and unowned
script bytes are errors.  The JSON output is consequently suitable as a
content-addressed CI artifact.
"""

from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path
import sys
from typing import Any, Iterable

try:
    from tools import art_build_contract as build_frontend
    from tools import art_javac_post_contract as javac_frontend
    from tools import art_run_contract as run_frontend
except ModuleNotFoundError:  # Direct execution from the tools directory.
    import art_build_contract as build_frontend
    import art_javac_post_contract as javac_frontend
    import art_run_contract as run_frontend


ENTRYPOINTS = {
    "run.py": "run",
    "build.py": "build",
    "javac_post.sh": "javac_post",
}
KNOWN_SCRIPT_NAMES = frozenset((*ENTRYPOINTS, "run.sh", "build.sh"))
VARIANTS = ("host", "jvm")

# These are source generators/helpers used by otherwise-owned build.py or
# run.py files in the pinned AOSP corpus.  They are not contract entrypoints;
# keeping the exact allowlist lets an unknown helper remain an audit error
# without rejecting the real corpus.
KNOWN_AUXILIARY_SCRIPTS = frozenset({
    "_aosp/art/test/661-oat-writer-layout/parse_oatdump_offsets.sh",
    "_aosp/art/test/701-easy-div-rem/genMain.py",
    "_aosp/art/test/939-hello-transformation-bcp/convert-to-base64.sh",
    "_aosp/art/test/988-method-trace/gen_srcs.py",
})


class ManifestError(ValueError):
    """The custom-contract corpus cannot be represented fail-closed."""


def _sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def _canonical(value: Any) -> bytes:
    return json.dumps(value, sort_keys=True, separators=(",", ":"),
                      ensure_ascii=True).encode("utf-8")


def _normalize_paths(value: Any, root: Path) -> Any:
    """Make frontend IR independent of the checkout's absolute path."""
    if isinstance(value, dict):
        normalized = {}
        for key, item in value.items():
            if key in {"path", "source"} and isinstance(item, str):
                candidate = Path(item)
                if candidate.is_absolute():
                    try:
                        item = candidate.resolve().relative_to(root).as_posix()
                    except ValueError:
                        pass
            normalized[key] = _normalize_paths(item, root)
        return normalized
    if isinstance(value, (list, tuple)):
        return [_normalize_paths(item, root) for item in value]
    return value


def _ir_hash(value: Any, root: Path) -> str:
    return _sha256(_canonical(_normalize_paths(value, root)))


def _numeric_custom_directories(root: Path) -> list[Path]:
    test_root = root / "_aosp" / "art" / "test"
    if not test_root.is_dir():
        return []
    result = []
    for directory in test_root.iterdir():
        if not directory.is_dir() or not directory.name[:1].isdigit():
            continue
        # 000-nop is a harness-only test, not one of the 419 custom-script
        # directories even though it carries historical script files.
        if directory.name == "000-nop":
            continue
        if any((directory / filename).is_file() for filename in KNOWN_SCRIPT_NAMES):
            result.append(directory)
    return sorted(result, key=lambda path: path.name)


def _relative_source(root: Path, path: Path) -> str:
    return path.resolve().relative_to(root).as_posix()


def _condition_truth(value: build_frontend.ValueIR, variant: str) -> bool:
    if value.kind == "context_bool":
        return (value.value == "jvm") == (variant == "jvm")
    if value.kind == "not":
        return not _condition_truth(value.value, variant)
    if value.kind == "equals":
        left = value.value["left"]
        right = value.value["right"]
        if left.kind == right.kind == "literal":
            return left.value == right.value
    # Build frontend rejects conditions whose truth cannot be represented; a
    # conservative false value keeps this metric from inventing an action.
    return False


def _build_metrics(actions: Iterable[build_frontend.ActionIR], variant: str) -> tuple[int, int]:
    action_count = 0
    invocation_count = 0
    for action in actions:
        action_count += 1
        if action.kind not in {"branch", "return", "noop"}:
            invocation_count += 1
        if action.kind == "branch":
            selected = action.arguments["then"] if _condition_truth(
                action.arguments["condition"], variant) else action.arguments["else"]
            nested_actions, nested_invocations = _build_metrics(selected, variant)
            action_count += nested_actions
            invocation_count += nested_invocations
    return action_count, invocation_count


def _run_metrics(plan: run_frontend.ActionPlan) -> tuple[int, int]:
    # Evaluator plans are already flattened to the actions that execute for a
    # representative context.  Every resulting action is an invocation of a
    # typed context effect or shell operation.
    count = len(plan.actions)
    return count, count


def _javac_metrics(contract: javac_frontend.JavacPostContract) -> tuple[int, int]:
    operations = contract.operations
    action_count = len(operations)
    invocation_count = sum(
        operation.kind not in {"branch", "for_each_class", "return"}
        for operation in operations
    )
    return action_count, invocation_count


def _opaque_shell(value: Any) -> bool:
    """Reject raw shell text while allowing typed ``shell_sequence`` IR."""
    if isinstance(value, dict):
        for key, item in value.items():
            if key in {"command", "shell"} and isinstance(item, str):
                return True
            if _opaque_shell(item):
                return True
    elif isinstance(value, (list, tuple)):
        return any(_opaque_shell(item) for item in value)
    return False


def _source_record(root: Path, path: Path, frontend: str,
                   ir: Any) -> dict[str, Any]:
    source = path.read_bytes()
    relative = _relative_source(root, path)
    if frontend == "run":
        ir_value = ir.as_dict()
    elif frontend == "build":
        ir_value = build_frontend.contract_json(ir)
    else:
        ir_value = javac_frontend.contract_json(ir)
    return {
        "path": relative,
        "frontend": frontend,
        "source_sha256": _sha256(source),
        "normalized_ir_sha256": _ir_hash(ir_value, root),
    }


def _auxiliary_record(root: Path, path: Path) -> dict[str, Any]:
    """Own a pinned source helper without pretending to compile it."""
    source = path.read_bytes()
    relative = _relative_source(root, path)
    ir = {"kind": "auxiliary_source_helper", "path": relative}
    return {
        "path": relative,
        "frontend": "auxiliary",
        "source_sha256": _sha256(source),
        "normalized_ir_sha256": _ir_hash(ir, root),
    }


def _unknown_scripts(root: Path, directory: Path) -> tuple[list[str], list[Path], int]:
    unknown: list[str] = []
    auxiliary: list[Path] = []
    unowned = 0
    for candidate in sorted(directory.iterdir(), key=lambda path: path.name):
        if not candidate.is_file() or candidate.suffix not in {".py", ".sh"}:
            continue
        relative = _relative_source(root, candidate)
        if candidate.name in ENTRYPOINTS:
            continue
        if relative in KNOWN_AUXILIARY_SCRIPTS:
            auxiliary.append(candidate)
            continue
        unknown.append(relative)
        unowned += candidate.stat().st_size
    return unknown, auxiliary, unowned


def _compile_source(root: Path, path: Path, frontend: str) -> tuple[Any | None, str | None]:
    try:
        if frontend == "run":
            return run_frontend.compile_path(path), None
        source = path.read_bytes() if frontend == "javac_post" else path.read_text(encoding="utf-8")
        if frontend == "build":
            return build_frontend.compile_contract(source, path), None
        return javac_frontend.compile_contract(source, path), None
    except (OSError, UnicodeError, ValueError) as error:
        return None, str(error)


def build_manifest(root: Path | str) -> dict[str, Any]:
    """Compile every custom entrypoint and return a deterministic manifest."""
    root = Path(root).resolve()
    directories: list[dict[str, Any]] = []
    errors: list[str] = []
    expected = _numeric_custom_directories(root)
    for directory in expected:
        records: list[dict[str, Any]] = []
        capabilities: set[str] = set()
        unsupported: list[str] = []
        opaque = 0
        variants = {
            variant: {"action_count": 0, "invocation_count": 0}
            for variant in VARIANTS
        }
        unknown, auxiliary, unowned = _unknown_scripts(root, directory)
        for relative in unknown:
            message = f"{relative}: unknown custom script; no frontend owns its bytes"
            unsupported.append(message)
            errors.append(message)
        for filename, frontend in ENTRYPOINTS.items():
            path = directory / filename
            if not path.is_file():
                continue
            ir, compile_error = _compile_source(root, path, frontend)
            if compile_error is not None:
                unsupported.append(compile_error)
                errors.append(compile_error)
                continue
            assert ir is not None
            if frontend == "run":
                if not ir.supported:
                    unsupported.extend(issue.reason for issue in ir.issues)
                    errors.extend(f"{path}: {issue.reason}" for issue in ir.issues)
                capabilities.update(ir.capabilities)
                records.append(_source_record(root, path, frontend, ir))
                for variant, context in run_frontend.representative_contexts().items():
                    plan = run_frontend.evaluate_contract(ir, context, variant)
                    if not plan.supported:
                        unsupported.extend(issue.reason for issue in plan.issues)
                        errors.extend(f"{path}: {issue.reason}" for issue in plan.issues)
                    action_count, invocation_count = _run_metrics(plan)
                    variants[variant]["action_count"] += action_count
                    variants[variant]["invocation_count"] += invocation_count
                    if _opaque_shell(plan.as_dict()):
                        opaque += 1
            elif frontend == "build":
                records.append(_source_record(root, path, frontend, ir))
                for action in ir.actions:
                    capabilities.add(f"build.{action.kind}")
                ir_value = build_frontend.contract_json(ir)
                if _opaque_shell(ir_value):
                    opaque += 1
                for variant in VARIANTS:
                    action_count, invocation_count = _build_metrics(ir.actions, variant)
                    variants[variant]["action_count"] += action_count
                    variants[variant]["invocation_count"] += invocation_count
            else:
                records.append(_source_record(root, path, frontend, ir))
                for operation in ir.operations:
                    capabilities.add(f"javac_post.{operation.kind}")
                if _opaque_shell(javac_frontend.contract_json(ir)):
                    opaque += 1
                action_count, invocation_count = _javac_metrics(ir)
                for variant in VARIANTS:
                    variants[variant]["action_count"] += action_count
                    variants[variant]["invocation_count"] += invocation_count
        for path in auxiliary:
            records.append(_auxiliary_record(root, path))
            capabilities.add("auxiliary.source_helper")
        if opaque:
            message = f"{directory}: opaque shell text escaped typed IR"
            unsupported.append(message)
            errors.append(message)
        if unowned:
            message = f"{directory}: {unowned} unowned source bytes"
            unsupported.append(message)
            errors.append(message)
        records.sort(key=lambda record: record["path"])
        directories.append({
            "directory": directory.name,
            "relative_source_paths": [record["path"] for record in records],
            "source_hashes": {record["path"]: record["source_sha256"] for record in records},
            "normalized_ir_hashes": {record["path"]: record["normalized_ir_sha256"] for record in records},
            "sources": records,
            "frontend_kinds": sorted({record["frontend"] for record in records}),
            "capabilities": sorted(capabilities),
            "variants": variants,
            "unsupported": sorted(set(unsupported)),
            "opaque_shell": opaque,
            "unowned_source_bytes": unowned,
        })
    directories.sort(key=lambda record: record["directory"])
    omitted = max(0, 419 - len(directories)) if expected else 0
    if len(directories) != 419:
        errors.append(f"custom-script directory count is {len(directories)}, expected 419")
    summary = {
        "directories": len(directories),
        "sources": sum(len(record["sources"]) for record in directories),
        "unsupported": sum(len(record["unsupported"]) for record in directories),
        "opaque_shell": sum(record["opaque_shell"] for record in directories),
        "unowned_source_bytes": sum(record["unowned_source_bytes"] for record in directories),
        "omitted_directories": omitted,
    }
    return {
        "schema": "aim/custom-contract-manifest/v1",
        "source_root": "_aosp/art/test",
        "directories": directories,
        "summary": summary,
        "errors": sorted(set(errors)),
    }


def verify_manifest(root: Path | str, manifest: dict[str, Any]) -> list[str]:
    """Return deterministic errors when a manifest no longer matches sources."""
    actual = build_manifest(root)
    expected_dirs = [record["directory"] for record in actual["directories"]]
    listed_dirs = [record.get("directory") for record in manifest.get("directories", [])]
    errors = []
    if listed_dirs != sorted(listed_dirs):
        errors.append("manifest directories are not sorted")
    if listed_dirs != expected_dirs:
        missing = sorted(set(expected_dirs) - set(listed_dirs))
        extra = sorted(set(listed_dirs) - set(expected_dirs))
        if missing:
            errors.append(f"manifest omitted custom-script directories: {','.join(missing)}")
        if extra:
            errors.append(f"manifest has unknown custom-script directories: {','.join(extra)}")
    if manifest.get("summary") != actual.get("summary"):
        errors.append("manifest summary does not match current sources")
    if manifest.get("directories") != actual.get("directories"):
        errors.append("manifest source or normalized IR hashes do not match current sources")
    if actual.get("errors"):
        errors.extend(actual["errors"])
    return sorted(set(errors))


def manifest_json(manifest: dict[str, Any]) -> str:
    """Serialize a manifest with stable key and record ordering."""
    return json.dumps(manifest, indent=2, sort_keys=True, ensure_ascii=True) + "\n"


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path,
                        default=Path(__file__).resolve().parent.parent)
    parser.add_argument("--strict", action="store_true",
                        help="require zero unsupported, opaque, unowned, or omitted items")
    args = parser.parse_args(argv)
    manifest = build_manifest(args.root)
    sys.stdout.write(manifest_json(manifest))
    summary = manifest["summary"]
    failed = bool(manifest["errors"] or any(summary[key] for key in (
        "unsupported", "opaque_shell", "unowned_source_bytes", "omitted_directories")))
    # The audit is fail-closed in both modes; --strict is the explicit CI
    # spelling of the invariant and intentionally cannot turn failures into a pass.
    return 1 if failed else 0


if __name__ == "__main__":
    raise SystemExit(main())
