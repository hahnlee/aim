"""Extract the pinned image's system partition packages and platform configuration.

PackageManagerService scans /system/app and /system/priv-app for system
packages and reads /system/etc/permissions, /system/etc/sysconfig and the
SELinux mac_permissions policy. This copies those trees from the locked image
without expanding the logical partition. From the product partition it copies
the static overlays that target the framework ("android"): every process
loads them over framework-res.apk as immutable framework overlays. The
partitions' aconfig flag protos decide which featureFlag-gated manifest
elements PackageManagerService parses. Dexpreopt output (`oat/` directories)
is left out: it was compiled against the image's own boot image, which this
runtime does not use, so ART would reject it.

Usage: extract_system_partition.py IMAGE OUTPUT_DIR [--lock LOCK]
With --lock the extracted inventory must equal the lock; without it the
inventory is printed in lock format.
"""
import argparse
import hashlib
import re
from pathlib import Path, PurePosixPath
import subprocess

TREES = ("/system/app", "/system/priv-app", "/system/etc/permissions", "/system/etc/sysconfig")
# The aconfig flag protos (DeviceProtos) that decide manifest featureFlag
# elements. This root keeps system_ext at /system/system_ext; the product
# partition's proto is empty (it declares no flags).
FLAG_PROTOS = {
    "/system/etc/aconfig_flags.pb": ([], "/system/etc/aconfig_flags.pb"),
    "/system/system_ext/etc/aconfig_flags.pb": (["--partition", "system_ext"],
                                                 "/etc/aconfig_flags.pb"),
}
FILES = ("/system/etc/selinux/plat_mac_permissions.xml",)
# /product/overlay APKs whose <overlay> targets "android" (all isStatic).
FRAMEWORK_OVERLAYS = tuple(f"/product/overlay/{name}.apk" for name in (
    "GoogleConfigOverlay",
    "GoogleWebViewOverlay",
    "LargeScreenConfigOverlay",
    "PixelConfigOverlayCommon",
    "RanchuCommonOverlay",
    "framework-res__emulator__auto_generated_characteristics_rro",
    "framework-res__sdk_gphone16k_arm64__auto_generated_rro_product",
))


def digest(path):
    with path.open("rb") as source:
        return hashlib.file_digest(source, "sha256").hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("image", type=Path)
    parser.add_argument("output", type=Path)
    parser.add_argument("--lock", type=Path)
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[2]
    tool = root / "target/release/super-i18n-apex-extract"
    image = args.image.resolve()
    output = args.output.resolve()

    def partition(path):
        # (--partition arguments, path inside that partition)
        if path in FLAG_PROTOS:
            return FLAG_PROTOS[path]
        if path.startswith("/product/"):
            return ["--partition", "product"], path[len("/product"):]
        return [], path

    def run(*extra):
        return subprocess.run([str(tool), str(image), "-", *extra], check=True,
                              capture_output=True, text=True).stdout

    def file_type(path):
        selector, inner = partition(path)
        return int(run(*selector, "--stat", inner).split()[0].split("=")[1], 8) & 0o170000

    files = []

    def walk(path):
        if file_type(path) != 0o040000:
            files.append(path)
            return
        for name in sorted(run("--path", path).split()):
            if name in (".", ".."):
                continue
            if name == "oat":
                continue  # dexpreopt against the image's own boot image
            walk(f"{path}/{name}")

    for tree in TREES:
        walk(tree)
    files.extend(FILES)
    files.extend(FRAMEWORK_OVERLAYS)
    files.extend(FLAG_PROTOS)
    inventory = []

    def library_jars():
        # Shared library JARs the permission files declare (<library file=...>).
        jars = set()
        for permissions in sorted((output / "system/etc/permissions").glob("*.xml")):
            jars.update(re.findall(r'file="(/system/framework/[^"]+\.jar)"',
                                   permissions.read_text()))
        return sorted(jar for jar in jars if jar not in files)

    pending = list(files)
    while pending:
        path = pending.pop(0)
        if path == FILES[-1]:
            extra = library_jars()
            files.extend(extra)
            pending.extend(extra)
        pure = PurePosixPath(path)
        if not pure.is_absolute() or ".." in pure.parts:
            raise ValueError(f"unsafe image path: {path}")
        destination = output / path.lstrip("/")
        destination.parent.mkdir(parents=True, exist_ok=True)
        if file_type(path) == 0o120000:
            # System app JNI libraries link into /system/lib64; keep the link.
            subprocess.run([str(tool), str(image), str(destination), "--symlink", path],
                           check=True, capture_output=True)
            inventory.append(f"link:{destination.readlink()} {path}")
            continue
        selector, inner = partition(path)
        subprocess.run([str(tool), str(image), str(destination), *selector, "--path", inner],
                       check=True, capture_output=True)
        inventory.append(f"{digest(destination)} {path}")
    if args.lock:
        expected = [line for line in args.lock.read_text().splitlines()
                    if line and not line.startswith("#")]
        if expected != inventory:
            raise SystemExit("system partition inventory differs from the lock")
    else:
        print("\n".join(inventory))


if __name__ == "__main__":
    main()
