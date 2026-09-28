"""Generates the Rust crates of the AIDL interfaces the replaced native
daemons serve (the aidl-gen node of `cargo aim`), the way Soong's rust_aidl
does, with the crate glue of vendor_hal_aidl.py.

The interfaces are the platform's own, not VINTF: IApexService and IVold
are unstable (compiled at the pinned tag, like the platform's
libapexd/libvold), INetd is a frozen stable version. Each crate's sources
are staged together (a package tree), then compiled with the SDK `aidl`.

Usage: daemon_aidl.py AIDL SRC OUT MANIFESTS ENTRY...
  SRC holds the fetched source trees; the crate of ENTRY goes to
  OUT/<crate>; ENTRY is an AIDL_CRATES line of daemons/sources.lock:
  crate|version|stability|files|includes
  - version: the frozen version, or 0 for an unstable interface;
  - stability: `stable` (frozen, `--structured`, with its .hash) or
    `unstable`;
  - files: comma-separated ROOT::GLOB, ROOT relative to SRC; the matched
    files keep their path below ROOT in the staged tree;
  - includes: comma-separated roots (relative to SRC) of imported types
    that are not compiled into the crate (such as PersistableBundle).

The Rust backend has no raw `FileDescriptor`. In an unstable interface such
a parameter or result is compiled as `ParcelFileDescriptor`, which keeps
every transaction code but not those methods' wire format: a daemon must
refuse them, and must not call such a callback (IVold's AppFuse calls and
IVoldMountCallback.onVolumeChecking).
"""

import glob
import hashlib
import os
import re
import shutil
import subprocess
import sys
import tomllib

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from vendor_hal_aidl import glue, sync  # noqa: E402


def fail(message):
    sys.exit(f"daemon_aidl: {message}")


def frozen_hash(api, version):
    """The frozen API's hash, checked as Soong does (aidlVerifyHashRule):
    sha1 over the sorted `sha1  ./path` lines of its files and the
    previous version, against the last line of `.hash`."""
    want = open(f"{api}/.hash").read().split()[-1]
    files = sorted(
        "./" + os.path.relpath(p, api)
        for p in glob.glob(f"{api}/**/*.aidl", recursive=True)
    )
    text = "".join(
        f"{hashlib.sha1(open(f'{api}/{f[2:]}', 'rb').read()).hexdigest()}  {f}\n"
        for f in files
    )
    text += ("latest-version" if version == 1 else str(version - 1)) + "\n"
    got = hashlib.sha1(text.encode()).hexdigest()
    if got != want:
        fail(f"{api}: API hash {got} != {want}")
    return want


def generate(aidl, src, out, manifests, entry):
    crate, version, stability, files, includes = entry.split("|")
    version = int(version)
    with open(f"{manifests}/{crate}/Cargo.toml", "rb") as f:
        manifest = tomllib.load(f)
    if manifest["package"]["name"] != crate:
        fail(f"daemons/aidl/{crate}/Cargo.toml: package name is not {crate}")
    stage = f"{out}/.src-{crate}"
    shutil.rmtree(stage, ignore_errors=True)
    staged = []
    for spec in files.split(","):
        root, pattern = spec.split("::")
        matches = sorted(glob.glob(f"{src}/{root}/{pattern}", recursive=True))
        if not matches:
            fail(f"{crate}: nothing matches {spec}")
        for path in matches:
            rel = os.path.relpath(path, f"{src}/{root}")
            os.makedirs(os.path.dirname(f"{stage}/{rel}"), exist_ok=True)
            text = open(path).read()
            if stability == "unstable":
                text = re.sub(r"([(,]\s*)FileDescriptor\b", r"\1in ParcelFileDescriptor", text)
                text = re.sub(r"(?<![\w.])FileDescriptor\b", "ParcelFileDescriptor", text)
            open(f"{stage}/{rel}", "w").write(text)
            staged.append(rel)
    args = ["aidl", "--lang=rust"]
    if stability == "stable":
        api = f"{src}/{files.split('::')[0]}"
        api_hash = frozen_hash(api, version)
        args += ["--structured", f"--version={version}", f"--hash={api_hash}"]
    elif stability != "unstable" or version != 0:
        fail(f"{crate}: unknown stability {stability} (version {version})")
    # Relative paths only, so the output depends on the inputs alone.
    for inc in [i for i in includes.split(",") if i]:
        args += ["-I", os.path.relpath(f"{src}/{inc}", stage)]
    new = f"{out}/.staging"
    shutil.rmtree(new, ignore_errors=True)
    subprocess.run(
        args + ["-I", ".", "-o", os.path.relpath(f"{new}/aidl", stage), *sorted(staged)],
        executable=aidl,
        cwd=stage,
        check=True,
    )
    with open(f"{new}/lib.rs", "w") as f:
        f.write(glue(crate, version, sorted(staged), []).replace(
            "vendor_hal_aidl.py", "daemon_aidl.py"))
    sync(new, f"{out}/{crate}")
    shutil.rmtree(new)
    shutil.rmtree(stage)


def main():
    aidl, src, out, manifests, *entries = sys.argv[1:]
    for entry in entries:
        generate(aidl, src, out, manifests, entry)
        print(f"generated {entry.split('|')[0]}")


if __name__ == "__main__":
    main()
