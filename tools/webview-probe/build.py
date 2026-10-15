#!/usr/bin/env python3
"""Build a normal WebView consumer APK using existing local SDK inputs."""
import argparse
import hashlib
import os
from pathlib import Path
import re
import shutil
import subprocess
import zipfile

ROOT = Path(__file__).resolve().parents[2]
SOURCE = Path(__file__).resolve().parent
OUTPUT = ROOT / "target/aim/webview-probe"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--android-jar", required=True, type=Path,
                        help="existing Android SDK 36 platforms/android-36/android.jar")
    args = parser.parse_args()
    pins = {}
    for line in (ROOT / "upstream/java-toolchain.lock").read_text().splitlines():
        match = re.fullmatch(r"([A-Z_]+)=([^\s#]+)", line)
        if match:
            pins[match[1]] = match[2]
    version = pins["JDK_VERSION"]
    jdk = ROOT / f"_build/java/temurin-{version}/jdk-{version}/Contents/Home"
    tools = ROOT / f"_build/java/build-tools-{pins['BUILD_TOOLS_VERSION']}/{pins['BUILD_TOOLS_DIR']}"
    sdk = args.android_jar.expanduser().resolve()
    for path in (sdk, tools / "aapt2", tools / "d8", tools / "apksigner",
                 jdk / "bin/javac", jdk / "bin/keytool"):
        if not path.is_file():
            parser.error(f"missing local input: {path}")
    OUTPUT.mkdir(parents=True, exist_ok=True)
    build = OUTPUT / "build"
    if build.exists():
        shutil.rmtree(build)
    (build / "classes").mkdir(parents=True)
    (build / "dex").mkdir()
    environment = dict(os.environ, JAVA_HOME=str(jdk))

    def run(*command):
        subprocess.run([str(value) for value in command], env=environment, check=True)

    run(tools / "aapt2", "link", "-I", sdk, "--manifest", SOURCE / "AndroidManifest.xml",
        "-o", build / "unsigned.apk")
    run(jdk / "bin/javac", "--release", "17", "-classpath", sdk, "-d", build / "classes",
        *sorted((SOURCE / "src").rglob("*.java")))
    run(tools / "d8", "--min-api", "26", "--lib", sdk, "--output", build / "dex",
        *sorted((build / "classes").rglob("*.class")))
    with zipfile.ZipFile(build / "unsigned.apk", "a", compression=zipfile.ZIP_DEFLATED) as apk:
        apk.write(build / "dex/classes.dex", "classes.dex")
    keystore = OUTPUT / "probe.keystore"
    if not keystore.exists():
        run(jdk / "bin/keytool", "-genkeypair", "-keystore", keystore,
            "-storepass", "webviewprobe", "-keypass", "webviewprobe", "-alias", "probe",
            "-keyalg", "RSA", "-keysize", "2048", "-validity", "30",
            "-dname", "CN=AIM Disposable WebView Probe")
    apk = OUTPUT / "webview-probe.apk"
    run(tools / "apksigner", "sign", "--ks", keystore, "--ks-pass", "pass:webviewprobe",
        "--key-pass", "pass:webviewprobe", "--out", apk, build / "unsigned.apk")
    run(tools / "apksigner", "verify", "--verbose", apk)
    receipt = hashlib.sha256(apk.read_bytes()).hexdigest() + "  webview-probe.apk\n"
    (OUTPUT / "SHA256SUMS").write_text(receipt)
    print(receipt, end="")


if __name__ == "__main__":
    main()
