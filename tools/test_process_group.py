#!/usr/bin/env python3
"""Unit tests for timeout cleanup at the ART runner boundary."""

from __future__ import annotations

import os
from pathlib import Path
import subprocess
import sys
import tempfile
import textwrap
import time
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parent))
import process_group


class ProcessGroupRunnerTest(unittest.TestCase):
    def test_normal_exit_kills_descendant_left_in_group(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            child_pid_file = Path(directory) / "child.pid"
            child_code = textwrap.dedent(f"""
                import os
                from pathlib import Path
                import signal
                import time

                signal.signal(signal.SIGTERM, signal.SIG_IGN)
                Path({str(child_pid_file)!r}).write_text(str(os.getpid()))
                time.sleep(60)
            """)
            parent_code = textwrap.dedent(f"""
                import subprocess
                import sys
                import time

                subprocess.Popen([sys.executable, "-c", {child_code!r}])
                time.sleep(0.2)
            """)
            result = process_group.run_process_group(
                [sys.executable, "-c", parent_code],
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                timeout=3,
            )
            self.assertEqual(result.returncode, 0)
            child_pid = int(child_pid_file.read_text(encoding="utf-8"))
            deadline = time.monotonic() + 3.0
            while time.monotonic() < deadline:
                try:
                    os.kill(child_pid, 0)
                except ProcessLookupError:
                    break
                time.sleep(0.05)
            else:
                self.fail(f"normally-exited process-group child remains: {child_pid}")

    def test_timeout_kills_descendant_and_reaps_direct_child(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            child_pid_file = Path(directory) / "child.pid"
            child_code = textwrap.dedent(f"""
                import os
                from pathlib import Path
                import signal
                import time

                signal.signal(signal.SIGTERM, signal.SIG_IGN)
                Path({str(child_pid_file)!r}).write_text(str(os.getpid()))
                time.sleep(60)
            """)
            parent_code = textwrap.dedent(f"""
                import subprocess
                import sys
                import time

                subprocess.Popen([sys.executable, "-c", {child_code!r}])
                time.sleep(60)
            """)
            with self.assertRaises(subprocess.TimeoutExpired):
                process_group.run_process_group(
                    [sys.executable, "-c", parent_code],
                    stdout=subprocess.PIPE,
                    stderr=subprocess.PIPE,
                    timeout=0.1,
                )

            child_pid = int(child_pid_file.read_text(encoding="utf-8"))
            deadline = time.monotonic() + 3.0
            while time.monotonic() < deadline:
                try:
                    os.kill(child_pid, 0)
                except ProcessLookupError:
                    break
                time.sleep(0.05)
            else:
                self.fail(f"timed-out process-group child remains: {child_pid}")


if __name__ == "__main__":
    unittest.main()
