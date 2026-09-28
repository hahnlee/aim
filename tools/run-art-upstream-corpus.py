#!/usr/bin/env python3
"""Run the pinned ART test corpus with a resumable deterministic ledger.

The corpus is discovered from ``_aosp/art/test``.  Directories carrying the
AOSP run-test output contract are executable inputs; sibling directories
without that contract are shared fixtures. There is no test-name allowlist or
exception table here.  The
existing per-test runner remains the sole implementation of ART's interpreter
and JIT execution contract.
"""

from __future__ import annotations

import argparse
from concurrent.futures import FIRST_COMPLETED, Future, ThreadPoolExecutor, wait
import csv
from dataclasses import asdict, dataclass
import hashlib
import io
import json
import os
from pathlib import Path
import subprocess
import sys
from typing import Iterable

try:
    from process_group import run_process_group
except ModuleNotFoundError:  # Imported as ``tools.run_art_upstream_corpus``.
    sys.path.insert(0, str(Path(__file__).resolve().parent))
    from process_group import run_process_group


LEDGER_VERSION = 1
TERMINAL_STATUSES = frozenset({"passed", "failed", "error", "timeout"})


@dataclass(frozen=True)
class TestResult:
    test: str
    input_hash: str
    runner_hash: str
    status: str
    exit_code: int | None
    runner_stdout: str
    runner_stderr: str
    artifacts: str
    error: str
    runtime_identity: str = ""


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def runtime_identity(root: Path) -> str:
    """Hash the immutable host/runtime inputs used by every corpus run."""
    fixed_paths = (
        root / "target/debug/aim-host",
        root / "_build/runtime-graphics-link-probe/libaim_runtime_graphics.dylib",
        root / "_build/native-graph/build.ninja",
        root / "_build/native-graph/build.inputs.sha256",
    )
    boot_dir = root / "_build/android16-boot-image-darwin"
    paths = list(fixed_paths)
    paths.extend(sorted(boot_dir.glob("boot*.art")))
    paths.extend(sorted(boot_dir.glob("boot*.oat")))
    paths.extend(sorted(boot_dir.glob("boot*.vdex")))
    paths.extend((
        root / "_build/android16-core-oj-compat/core-oj-compat.jar",
        root / "_prebuilt/android-16/bootclasspath/core-libart.jar",
        root / "_build/android16-framework-compat/framework-compat.jar",
        root / "_prebuilt/android-16/bootclasspath/framework-location.jar",
        root / "_build/bootclasspath/core-icu4j-api36.jar",
        root / "_build/dex-probe/unsafe-boot-dex/unsafe-boot.jar",
    ))
    paths.extend(sorted(
        (root / "_build/android16-ps16k-r07/extracted").rglob("*.jar")))
    digest = hashlib.sha256(b"aim-corpus-runtime-v1\0")
    for path in sorted(set(paths)):
        relative = path.relative_to(root).as_posix().encode("utf-8")
        digest.update(relative + b"\0")
        if path.is_file():
            digest.update(sha256_file(path).encode("ascii"))
        else:
            digest.update(b"missing")
        digest.update(b"\0")
    return digest.hexdigest()


def input_hash(test: Path) -> str:
    """Hash a test tree including relative names and symlink targets."""
    digest = hashlib.sha256()
    digest.update(b"aim-test-input-v1\0")
    entries = sorted(
        test.rglob("*"), key=lambda path: path.relative_to(test).as_posix())
    for path in entries:
        relative = path.relative_to(test).as_posix().encode("utf-8")
        if path.is_symlink():
            digest.update(b"L\0" + relative + b"\0")
            digest.update(os.readlink(path).encode("utf-8", "surrogateescape"))
            digest.update(b"\0")
        elif path.is_dir():
            digest.update(b"D\0" + relative + b"\0")
        elif path.is_file():
            digest.update(b"F\0" + relative + b"\0")
            with path.open("rb") as stream:
                for chunk in iter(lambda: stream.read(1024 * 1024), b""):
                    digest.update(chunk)
            digest.update(b"\0")
        else:
            raise RuntimeError(f"unsupported corpus input entry: {path}")
    return digest.hexdigest()


def discover_tests(root: Path) -> list[str]:
    archive = root / "_aosp/art/test"
    if not archive.is_dir():
        raise RuntimeError(f"pinned ART corpus is missing: {archive}")
    # AOSP keeps helper classes, dex fixtures, and build support trees beside
    # runnable tests. The run-test harness identifies executable inputs by the
    # expected stdout contract, not by directory names; mirror that structural
    # boundary so fixtures are never reported as failed tests.
    return sorted(
        path.name for path in archive.iterdir()
        if path.is_dir() and (path / "expected-stdout.txt").is_file()
    )


def shard_tests(tests: list[str], shard_index: int, shard_count: int) -> list[str]:
    if shard_count < 1 or not 0 <= shard_index < shard_count:
        raise ValueError("shard-index must be in [0, shard-count), with shard-count >= 1")
    return [test for index, test in enumerate(tests) if index % shard_count == shard_index]


def select_contiguous_range(tests: list[str], start_at: str | None = None,
                            stop_after: str | None = None) -> list[str]:
    """Select an inclusive range from the deterministic corpus ordering.

    Applying the range before sharding keeps named boundaries stable across
    invocations.  ``stop_after`` is inclusive despite its CLI wording: it is
    the last test to include in the selected range.
    """
    if not tests:
        if start_at is not None or stop_after is not None:
            missing = start_at if start_at is not None else stop_after
            raise ValueError(f"range boundary is not in the corpus: {missing}")
        return []
    if start_at is None:
        start_index = 0
    else:
        try:
            start_index = tests.index(start_at)
        except ValueError as error:
            raise ValueError(f"start-at test is not in the corpus: {start_at}") from error
    if stop_after is None:
        stop_index = len(tests) - 1
    else:
        try:
            stop_index = tests.index(stop_after)
        except ValueError as error:
            raise ValueError(f"stop-after test is not in the corpus: {stop_after}") from error
    if start_index > stop_index:
        raise ValueError(
            f"start-at test must not follow stop-after test: {start_at} > {stop_after}")
    return tests[start_index:stop_index + 1]


def _atomic_write(path: Path, contents: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_name(f".{path.name}.tmp-{os.getpid()}")
    temporary.write_text(contents, encoding="utf-8")
    os.replace(temporary, path)


def load_records(path: Path) -> dict[str, TestResult]:
    if not path.is_file():
        return {}
    payload = json.loads(path.read_text(encoding="utf-8"))
    if payload.get("version") != LEDGER_VERSION:
        raise RuntimeError(f"unsupported corpus ledger version in {path}")
    records: dict[str, TestResult] = {}
    for raw in payload.get("results", []):
        result = TestResult(**raw)
        records[result.test] = result
    return records


def write_ledgers(ledger_dir: Path, records: dict[str, TestResult]) -> None:
    ordered = [asdict(records[name]) for name in sorted(records)]
    payload = {
        "version": LEDGER_VERSION,
        "runner_hashes": sorted({
            result["runner_hash"] for result in ordered if result["runner_hash"]
        }),
        "results": ordered,
    }
    _atomic_write(
        ledger_dir / "summary.json",
        json.dumps(payload, indent=2, sort_keys=True) + "\n")
    output = io.StringIO(newline="")
    writer = csv.DictWriter(
        output,
        fieldnames=(
            "test", "input_hash", "runner_hash", "status", "exit_code",
            "runner_stdout", "runner_stderr", "artifacts", "error",
            "runtime_identity",
        ),
        delimiter="\t",
        lineterminator="\n",
        extrasaction="ignore",
    )
    writer.writeheader()
    writer.writerows(ordered)
    _atomic_write(ledger_dir / "summary.tsv", output.getvalue())


def _artifact_path(stdout: bytes) -> str:
    for line in stdout.decode("utf-8", "replace").splitlines():
        if line.startswith("artifacts="):
            return line[len("artifacts="):]
    return ""


def run_test(root: Path, runner: Path, test: str, result_dir: Path,
             timeout: float | None, runtime_digest: str) -> TestResult:
    test_input = input_hash(root / "_aosp/art/test" / test)
    runner_digest = sha256_file(runner)
    result_dir.mkdir(parents=True, exist_ok=True)
    command = [
        sys.executable, str(runner), test, "--root", str(root), "--keep",
    ]
    try:
        completed = run_process_group(
            command,
            check=False,
            cwd=root,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            timeout=timeout,
        )
    except subprocess.TimeoutExpired as error:
        stdout = error.stdout or b""
        stderr = error.stderr or b""
        status = "timeout"
        exit_code = None
        message = f"timeout after {timeout:g}s"
    except OSError as error:
        stdout = b""
        stderr = b""
        status = "error"
        exit_code = None
        message = str(error)
    else:
        stdout = completed.stdout
        stderr = completed.stderr
        exit_code = completed.returncode
        status = "passed" if exit_code == 0 else "failed"
        message = "" if status == "passed" else f"runner exit {exit_code}"
    stdout_path = result_dir / "runner.stdout"
    stderr_path = result_dir / "runner.stderr"
    stdout_path.write_bytes(stdout)
    stderr_path.write_bytes(stderr)
    return TestResult(
        test=test,
        input_hash=test_input,
        runner_hash=runner_digest,
        status=status,
        exit_code=exit_code,
        runner_stdout=str(stdout_path),
        runner_stderr=str(stderr_path),
        artifacts=_artifact_path(stdout),
        error=message,
        runtime_identity=runtime_digest,
    )


def _run_parallel(root: Path, runner: Path, tests: list[str], ledger_dir: Path,
                  records: dict[str, TestResult], parallel: int,
                  fail_fast: bool, timeout: float | None,
                  runtime_digest: str) -> None:
    def submit(executor: ThreadPoolExecutor, test: str) -> Future[TestResult]:
        return executor.submit(
            run_test, root, runner, test,
            ledger_dir / "results" / test,
            timeout, runtime_digest)

    pending = iter(tests)
    active: dict[Future[TestResult], str] = {}
    stopped = False
    with ThreadPoolExecutor(max_workers=parallel) as executor:
        for _ in range(parallel):
            try:
                test = next(pending)
            except StopIteration:
                break
            active[submit(executor, test)] = test
        while active:
            done, _ = wait(active, return_when=FIRST_COMPLETED)
            # The runner identity is captured before dispatch.  A concurrent
            # build must never be allowed to stamp later results with an
            # identity different from the one that was actually prepared.
            # Abort before publishing another row; callers can rerun after
            # quiescing the build graph.
            observed_digest = runtime_identity(root)
            if observed_digest != runtime_digest:
                for future in active:
                    future.cancel()
                raise RuntimeError(
                    "runtime identity changed during corpus execution: "
                    f"started={runtime_digest} observed={observed_digest}")
            # Completion order is intentionally normalized before writing the
            # ledger, keeping JSON/TSV content deterministic.
            completed = sorted(
                ((active.pop(future), future) for future in done),
                key=lambda item: item[0])
            for test, future in completed:
                result = future.result()
                records[test] = result
                write_ledgers(ledger_dir, records)
                print(f"{test}\t{result.status}")
                if fail_fast and result.status != "passed":
                    stopped = True
            if stopped:
                continue
            while len(active) < parallel:
                try:
                    test = next(pending)
                except StopIteration:
                    break
                active[submit(executor, test)] = test


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, default=Path(__file__).resolve().parent.parent)
    parser.add_argument(
        "--runner", type=Path,
        default=Path(__file__).resolve().with_name("run-art-upstream-test.py"),
        help="per-test runner invoked as a subprocess")
    parser.add_argument(
        "--ledger", type=Path,
        help="ledger directory (default: <root>/_build/art-upstream-corpus)")
    parser.add_argument("--parallel", type=int, default=1)
    parser.add_argument("--shard-index", type=int, default=0)
    parser.add_argument("--shard-count", type=int, default=1)
    parser.add_argument(
        "--start-at", metavar="TEST",
        help="start at this corpus test (inclusive) in deterministic name order")
    parser.add_argument(
        "--stop-after", metavar="TEST",
        help="stop after this corpus test (inclusive) in deterministic name order")
    parser.add_argument("--limit", type=int, help="run only the first selected inputs")
    parser.add_argument("--timeout", type=float, help="optional per-test subprocess timeout")
    parser.add_argument("--resume", action="store_true",
                        help="reuse terminal results with matching input and runner hashes")
    policy = parser.add_mutually_exclusive_group()
    policy.add_argument("--fail-fast", action="store_true")
    policy.add_argument("--continue", dest="continue_on_failure", action="store_true",
                        help="continue after failures (the default)")
    return parser


def main(argv: Iterable[str] | None = None) -> int:
    args = build_parser().parse_args(argv)
    if args.parallel < 1:
        raise SystemExit("parallel must be >= 1")
    if args.limit is not None and args.limit < 0:
        raise SystemExit("limit must be >= 0")
    if args.timeout is not None and args.timeout <= 0:
        raise SystemExit("timeout must be > 0")
    root = args.root.resolve()
    runner = args.runner.resolve()
    if not runner.is_file():
        raise SystemExit(f"runner is missing: {runner}")
    ledger_dir = (
        args.ledger if args.ledger is not None
        else root / "_build/art-upstream-corpus")
    ledger_dir = ledger_dir.resolve()
    all_tests = discover_tests(root)
    selected = select_contiguous_range(all_tests, args.start_at, args.stop_after)
    selected = shard_tests(selected, args.shard_index, args.shard_count)
    if args.limit is not None:
        selected = selected[:args.limit]
    records = load_records(ledger_dir / "summary.json")
    runner_digest = sha256_file(runner)
    runtime_digest = runtime_identity(root)
    work: list[str] = []
    for test in selected:
        current_hash = input_hash(root / "_aosp/art/test" / test)
        previous = records.get(test)
        if (args.resume and previous is not None
                and previous.input_hash == current_hash
                and previous.runner_hash == runner_digest
                and previous.runtime_identity == runtime_digest
                # A failed run is deliberately retried.  ``--resume`` is a
                # continuation mechanism for proven results, not a way to
                # freeze transient allocator/runtime failures in the ledger.
                and previous.status == "passed"):
            print(f"{test}\tresumed\t{previous.status}")
            continue
        work.append(test)
    write_ledgers(ledger_dir, records)
    _run_parallel(
        root, runner, work, ledger_dir, records, args.parallel,
        args.fail_fast, args.timeout, runtime_digest)
    write_ledgers(ledger_dir, records)
    selected_results = [records[test] for test in selected if test in records]
    return 0 if all(result.status == "passed" for result in selected_results) \
        and len(selected_results) == len(selected) else 1


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (RuntimeError, ValueError, json.JSONDecodeError) as error:
        print(f"ART corpus runner failed: {error}", file=sys.stderr)
        raise SystemExit(2)
