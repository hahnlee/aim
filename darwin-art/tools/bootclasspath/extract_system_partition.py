"""Extract the pinned image's system partition packages and platform configuration.

PackageManagerService scans /system/app and /system/priv-app for system
packages and reads /system/etc/permissions, /system/etc/sysconfig and the
SELinux mac_permissions policy. This copies those trees from the locked image
without expanding the logical partition. From the product partition it copies
the static overlays that target the framework ("android"): every process
loads them over framework-res.apk as immutable framework overlays. The
partitions' aconfig flag protos decide which featureFlag-gated manifest
elements PackageManagerService parses. The aconfig storage files of every
partition and image APEX are laid out as aconfigd publishes them at boot for
the new flag storage readers: /metadata/aconfig/maps/CONTAINER.{package,flag}.map
and /metadata/aconfig/boot/CONTAINER.{val,info} (no local overrides: the
values are the image's). From vendor.img (next to IMAGE) it
copies the feature declarations that describe what the host provides: the
Vulkan level/version/compute XMLs match the MoltenVK provider (Vulkan 1.3 with
every level 1 feature). Dexpreopt output (`oat/` directories)
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
import struct
import subprocess
import tempfile

TREES = ("/system/app", "/system/priv-app", "/system/etc/permissions", "/system/etc/sysconfig")
# The aconfig flag protos (DeviceProtos) that decide manifest featureFlag
# elements. This root keeps system_ext at /system/system_ext; the product
# partition's proto is empty (it declares no flags).
FLAG_PROTOS = {
    "/system/etc/aconfig_flags.pb": ([], "/system/etc/aconfig_flags.pb"),
    "/system/system_ext/etc/aconfig_flags.pb": (["--partition", "system_ext"],
                                                 "/etc/aconfig_flags.pb"),
}
# Partition containers whose etc/aconfig holds storage files.
STORAGE_PARTITIONS = (
    ("system", False, [], "/system/etc/aconfig"),
    ("system_ext", False, ["--partition", "system_ext"], "/etc/aconfig"),
    ("product", False, ["--partition", "product"], "/etc/aconfig"),
    ("vendor", True, ["--partition", "vendor"], "/etc/aconfig"),
)
# aconfigd's name for each storage file of a container.
STORAGE_FILES = (
    ("package.map", "maps/{}.package.map"),
    ("flag.map", "maps/{}.flag.map"),
    ("flag.val", "boot/{}.val"),
    ("flag.info", "boot/{}.info"),
)
FILES = ("/system/etc/selinux/plat_mac_permissions.xml",)
# vendor.img feature XMLs whose capabilities the host graphics provider has.
VENDOR_FILES = tuple(f"/vendor/etc/permissions/android.hardware.vulkan.{name}.xml"
                     for name in ("compute", "level", "version"))
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


def storage_container(package_map):
    # PackageTable header: u32 version, then the container as a u32-length string.
    data = package_map.read_bytes()
    length = struct.unpack_from("<I", data, 4)[0]
    return data[8:8 + length].decode()


def aconfig_storage(image, vendor_image, output, run, root):
    tool = root / "target/release/super-i18n-apex-extract"
    apex_tool = root / "target/release/apex-ext2-extract"
    metadata = output / "metadata/aconfig"
    (metadata / "maps").mkdir(parents=True, exist_ok=True)
    (metadata / "boot").mkdir(parents=True, exist_ok=True)
    published = {}
    with tempfile.TemporaryDirectory() as scratch:
        scratch = Path(scratch)

        def publish(container, extract):
            if container in published:
                raise ValueError(f"duplicate aconfig container: {container}")
            published[container] = [template.format(container) for _, template in STORAGE_FILES]
            for name, template in STORAGE_FILES:
                extract(name, metadata / template.format(container))

        for container, vendor, selector, directory in STORAGE_PARTITIONS:
            source = vendor_image if vendor else image
            publish(container, lambda name, destination: subprocess.run(
                [str(tool), str(source), str(destination), *selector,
                 "--path", f"{directory}/{name}"], check=True, capture_output=True))
        for name in sorted(run(image, "--path", "/system/apex").split()):
            if not name.endswith((".apex", ".capex")):
                continue
            work = scratch / name
            work.mkdir()
            archive = work / "payload.apex"
            packaged = work / name
            subprocess.run([str(tool), str(image), str(packaged), "--path", f"/system/apex/{name}"],
                           check=True, capture_output=True)
            if name.endswith(".capex"):
                archive.write_bytes(subprocess.run(["unzip", "-p", str(packaged), "original_apex"],
                                                   check=True, capture_output=True).stdout)
            else:
                packaged.replace(archive)
            package_map = work / "package.map"
            probe = subprocess.run([str(apex_tool), str(archive), str(package_map),
                                    "/etc/package.map"], capture_output=True, text=True)
            if probe.returncode != 0:
                # APEXes that declare no flags carry no storage files.
                if ('"package.map" was not found' in probe.stderr
                        or '"etc" was not found' in probe.stderr):
                    continue
                raise SystemExit(f"{name}: {probe.stderr.strip()}")
            publish(storage_container(package_map), lambda file, destination: subprocess.run(
                [str(apex_tool), str(archive), str(destination), f"/etc/{file}"],
                check=True, capture_output=True))
    return [f"{digest(metadata / path)} /metadata/aconfig/{path}"
            for container in sorted(published) for path in published[container]]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("image", type=Path)
    parser.add_argument("output", type=Path)
    parser.add_argument("--lock", type=Path)
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[2]
    tool = root / "target/release/super-i18n-apex-extract"
    image = args.image.resolve()
    vendor_image = image.parent / "vendor.img"
    output = args.output.resolve()

    def partition(path):
        # (image, --partition arguments, path inside that partition)
        if path in FLAG_PROTOS:
            return (image, *FLAG_PROTOS[path])
        if path.startswith("/product/"):
            return image, ["--partition", "product"], path[len("/product"):]
        if path.startswith("/vendor/"):
            return vendor_image, ["--partition", "vendor"], path[len("/vendor"):]
        return image, [], path

    def run(source, *extra):
        return subprocess.run([str(tool), str(source), "-", *extra], check=True,
                              capture_output=True, text=True).stdout

    def file_type(path):
        source, selector, inner = partition(path)
        return int(run(source, *selector, "--stat", inner).split()[0].split("=")[1],
                   8) & 0o170000

    files = []

    def walk(path):
        if file_type(path) != 0o040000:
            files.append(path)
            return
        for name in sorted(run(image, "--path", path).split()):
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
    files.extend(VENDOR_FILES)
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
        source, selector, inner = partition(path)
        subprocess.run([str(tool), str(source), str(destination), *selector, "--path", inner],
                       check=True, capture_output=True)
        inventory.append(f"{digest(destination)} {path}")
    inventory.extend(aconfig_storage(image, vendor_image, output, run, root))
    if args.lock:
        expected = [line for line in args.lock.read_text().splitlines()
                    if line and not line.startswith("#")]
        if expected != inventory:
            raise SystemExit("system partition inventory differs from the lock")
    else:
        print("\n".join(inventory))


if __name__ == "__main__":
    main()
