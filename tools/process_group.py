"""Run a subprocess with deterministic process-group timeout cleanup."""

from __future__ import annotations

import os
import signal
import subprocess
import time
from typing import Any, Sequence


# A timed-out ART invocation gets a short, graceful shutdown window before the
# whole process group is force-killed.  Keeping this at the runner boundary is
# important: subprocess.run() only waits for its direct child and can leave a
# aim-host descendant behind when that child times out.
TERMINATE_GRACE_SECONDS = 1.0


def _signal_group(process: subprocess.Popen[Any], signum: int) -> None:
    """Signal the session/process group, tolerating an already-gone group."""
    if os.name == "posix":
        try:
            os.killpg(process.pid, signum)
            return
        except ProcessLookupError:
            return
    # The ART runner currently targets Darwin, but retain a useful fallback
    # for callers importing this helper on platforms without process groups.
    if process.poll() is None:
        process.send_signal(signum)


def _snapshot_descendant_pids(root_pid: int) -> list[int]:
    """Return descendants while the parent/child relationship still exists."""
    if os.name != "posix":
        return []
    try:
        listing = subprocess.check_output(
            ["ps", "-axo", "pid=,ppid="], text=True, stderr=subprocess.DEVNULL)
    except (OSError, subprocess.SubprocessError):
        return []
    children: dict[int, list[int]] = {}
    for line in listing.splitlines():
        fields = line.split()
        if len(fields) != 2:
            continue
        try:
            pid, parent = (int(value) for value in fields)
        except ValueError:
            continue
        children.setdefault(parent, []).append(pid)
    descendants: list[int] = []
    pending = list(children.get(root_pid, ()))
    while pending:
        pid = pending.pop()
        descendants.append(pid)
        pending.extend(children.get(pid, ()))
    return descendants


def _signal_descendants(pids: list[int], signum: int) -> None:
    """Signal descendants, including helpers that created their own session."""
    for pid in reversed(pids):
        try:
            os.killpg(pid, signum)
        except (ProcessLookupError, PermissionError):
            pass
        try:
            os.kill(pid, signum)
        except (ProcessLookupError, PermissionError):
            pass


def _terminate_process_tree(process: subprocess.Popen[Any]) -> list[int]:
    # Snapshot before terminating the runner: nested helpers can call setsid,
    # and then become PPID 1 immediately when the runner exits.
    descendants = _snapshot_descendant_pids(process.pid)
    _signal_group(process, signal.SIGTERM)
    _signal_descendants(descendants, signal.SIGTERM)
    return descendants


def _reap_orphaned_group(process: subprocess.Popen[Any]) -> None:
    """Terminate descendants that outlived the direct child.

    ART launches helper processes for dex2oat and the host runtime.  A child
    can exit after handing one of those helpers an inherited descriptor, which
    leaves the process group alive even though ``communicate`` has returned.
    Probe the original process-group id and close that group before returning;
    this keeps repeated corpus runs isolated and prevents stale hosts from
    consuming the next test's resources.
    """
    if os.name != "posix":
        return
    try:
        os.killpg(process.pid, 0)
    except ProcessLookupError:
        return
    except PermissionError:
        return
    _signal_group(process, signal.SIGTERM)
    try:
        # A short grace period is enough for normal helper teardown while
        # keeping the runner bounded when a descendant is stuck.
        import time

        deadline = time.monotonic() + TERMINATE_GRACE_SECONDS
        while time.monotonic() < deadline:
            try:
                os.killpg(process.pid, 0)
            except ProcessLookupError:
                return
            time.sleep(0.01)
    finally:
        try:
            os.killpg(process.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass


def run_process_group(
    arguments: Sequence[str],
    *,
    check: bool = False,
    env: dict[str, str] | None = None,
    stdout: Any = None,
    stderr: Any = None,
    timeout: float | None = None,
    cwd: os.PathLike[str] | str | None = None,
) -> subprocess.CompletedProcess[Any]:
    """Run *arguments* in a new session and reap every timed-out descendant.

    A timeout first sends SIGTERM to the complete process group, waits briefly
    for normal cleanup, then sends SIGKILL and waits without a timeout.  The
    final wait is unconditional, so this helper never returns with a direct
    child (or an inherited pipe held by one of its descendants) unreaped.
    """
    process = subprocess.Popen(
        list(arguments),
        env=env,
        stdout=stdout,
        stderr=stderr,
        cwd=cwd,
        start_new_session=True,
    )
    started = time.monotonic()
    # Poll communicate in short slices.  A direct child can exit while a
    # descendant still owns its inherited stdout/stderr descriptors; waiting
    # once for EOF would hide that exit until the full timeout expires.
    while True:
        if timeout is None:
            slice_timeout = 0.1
        else:
            remaining = timeout - (time.monotonic() - started)
            if remaining <= 0:
                original_timeout = subprocess.TimeoutExpired(
                    list(arguments), timeout)
                descendants = _terminate_process_tree(process)
                try:
                    output, error = process.communicate(
                        timeout=TERMINATE_GRACE_SECONDS)
                except subprocess.TimeoutExpired:
                    _signal_group(process, signal.SIGKILL)
                    _signal_descendants(descendants, signal.SIGKILL)
                    output, error = process.communicate()
                _reap_orphaned_group(process)
                raise subprocess.TimeoutExpired(
                    list(arguments), timeout, output=output, stderr=error) \
                    from original_timeout
            slice_timeout = min(0.1, remaining)
        try:
            output, error = process.communicate(timeout=slice_timeout)
            break
        except subprocess.TimeoutExpired as pending:
            if process.poll() is not None:
                # The direct child is gone; remove descendants before the
                # final communicate so inherited pipes cannot keep us stuck.
                _reap_orphaned_group(process)
                output, error = process.communicate()
                break
            if timeout is not None and time.monotonic() - started >= timeout:
                descendants = _terminate_process_tree(process)
                try:
                    output, error = process.communicate(
                        timeout=TERMINATE_GRACE_SECONDS)
                except subprocess.TimeoutExpired:
                    _signal_group(process, signal.SIGKILL)
                    _signal_descendants(descendants, signal.SIGKILL)
                    output, error = process.communicate()
                _reap_orphaned_group(process)
                raise subprocess.TimeoutExpired(
                    list(arguments), timeout, output=output, stderr=error) \
                    from pending

    _reap_orphaned_group(process)

    completed = subprocess.CompletedProcess(
        list(arguments), process.returncode, output, error)
    if check and completed.returncode:
        raise subprocess.CalledProcessError(
            completed.returncode,
            completed.args,
            output=completed.stdout,
            stderr=completed.stderr,
        )
    return completed
