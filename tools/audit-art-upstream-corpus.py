#!/usr/bin/env python3
"""Classify every test in the checksum-pinned AOSP ART test archive."""

from __future__ import annotations

import argparse
from collections import Counter
from pathlib import Path
import re


SCRIPT_FILES = ("build.py", "run.py", "build.sh", "run.sh", "javac_post.sh")
SECONDARY_SOURCE_DIRS = (
    "src2",
    "src-aotex",
    "src-art",
    "src-bcpex",
    "src-ex",
    "src-ex2",
    "src-java",
    "src-multidex",
    "src-optional",
    "src-redefine",
    "src-secondary",
    "src-util",
    "src_gen",
)
BYTECODE_SUFFIXES = (".smali", ".j", ".jasmin")
NATIVE_SUFFIXES = (".c", ".cc", ".cpp", ".S", ".s")
EXTERNAL_NATIVE_PATTERNS = (
    re.compile(rb"\bnative\s+[A-Za-z_$]"),
    re.compile(rb"\bSystem\s*\.\s*load(?:Library)?\s*\("),
    re.compile(rb"\bRuntime\s*\.\s*(?:getRuntime\s*\(\s*\)\s*\.)?nativeLoad\b"),
    re.compile(rb"[\"']nativeLoad[\"']"),
)


def contains_suffix(root: Path, suffixes: tuple[str, ...]) -> bool:
    return any(path.is_file() and path.suffix in suffixes for path in root.rglob("*"))


def requires_external_native(test: Path) -> bool:
    for source in test.rglob("*.java"):
        contents = source.read_bytes()
        if any(pattern.search(contents) for pattern in EXTERNAL_NATIVE_PATTERNS):
            return True
    return False


def classify(test: Path) -> str:
    # ART run-test directories are revision-stable numeric names
    # (`NNN-description`). The same parent also owns shared source/build
    # fixtures such as common, ti-agent, dexpreopt and verifier inputs. They
    # intentionally have no expected-output contract and are dependencies of
    # real tests, not silently unsupported tests themselves.
    if not test.name[:1].isdigit():
        return "shared-fixture"
    if not (test / "expected-stdout.txt").is_file() or not (
        test / "expected-stderr.txt"
    ).is_file():
        return "missing-expected"
    if test.name == "000-nop":
        return "harness-only"
    if any((test / name).exists() for name in SCRIPT_FILES):
        return "custom-script"
    if contains_suffix(test, BYTECODE_SUFFIXES):
        return "bytecode-source"
    if contains_suffix(test, NATIVE_SUFFIXES):
        return "native"
    if requires_external_native(test):
        return "external-native"
    if any((test / name).exists() for name in SECONDARY_SOURCE_DIRS):
        return "multi-source"
    if not (test / "src/Main.java").is_file():
        return "prebuilt-or-empty"
    return "pure-java"


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--root", type=Path, default=Path(__file__).resolve().parent.parent
    )
    parser.add_argument("--category")
    parser.add_argument("--tsv", action="store_true")
    args = parser.parse_args()
    archive = args.root.resolve() / "_aosp/art/test"
    tests = sorted(path for path in archive.iterdir() if path.is_dir())
    classified = [(test.name, classify(test)) for test in tests]
    if args.category:
        for name, category in classified:
            if category == args.category:
                print(name)
        return 0
    if args.tsv:
        print("test\tcategory")
        for name, category in classified:
            print(f"{name}\t{category}")
        return 0
    counts = Counter(category for _, category in classified)
    print(f"total\t{len(classified)}")
    for category in sorted(counts):
        print(f"{category}\t{counts[category]}")
    if sum(counts.values()) != len(tests):
        raise RuntimeError("classification did not cover the complete archive")
    shared = [test for test, category in classified if category == "shared-fixture"]
    if any((archive / name / "expected-stdout.txt").exists() for name in shared):
        raise RuntimeError("non-numeric AOSP run-test escaped executable classification")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
