#!/usr/bin/env python3
"""Build the synthetic resource archives used by the discovery session tests."""
from pathlib import Path
import subprocess
import tempfile

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[4]
AAPT = ROOT / "_build/java/build-tools-36.0.0/android-16/aapt2"

with tempfile.TemporaryDirectory(prefix="aim-discovery-resources-") as work:
    work = Path(work)
    for name in ("framework", "providers"):
        compiled = work / f"{name}.zip"
        subprocess.run(
            [AAPT, "compile", "--dir", HERE / name / "res", "-o", compiled],
            check=True,
        )
        command = [
            AAPT, "link", "--manifest", HERE / name / "AndroidManifest.xml",
            "--no-auto-version", "-o", HERE / f"{name}.apk", compiled,
        ]
        command += ["-x"] if name == "framework" else ["-I", HERE / "framework.apk"]
        subprocess.run(command, check=True)
