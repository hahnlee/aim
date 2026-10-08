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

The Rust backend has no raw `FileDescriptor`. Unstable raw FD declarations
use an import-only Rust type surrogate whose codec preserves original raw
FD wire objects and native libbinder_ndk ownership. Explicit
`ParcelFileDescriptor` declarations retain their original distinct codec.
"""

import glob
import hashlib
import json
import os
import re
import shutil
import subprocess
import sys
import tomllib

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from vendor_hal_aidl import glue, sync  # noqa: E402

RAW_FD_PACKAGE = "dev.aim.binder"
RAW_FD_TYPE = "AimRawFileDescriptor"
RAW_FD_DECLARATION = (f"package {RAW_FD_PACKAGE};\n"
                      f'parcelable {RAW_FD_TYPE} rust_type "crate::RawFileDescriptor";\n')


def stage_raw_fds(text):
    """Distinguish original raw FD type tokens from explicit PFD declarations."""
    if not re.search(r"(?<![\w.])FileDescriptor\b", text):
        return text, False
    text = re.sub(r"([(,]\s*(?:@\w+(?:\([^)]*\))?\s*)*)FileDescriptor\b",
                  rf"\1in {RAW_FD_TYPE}", text)
    text = re.sub(r"(?<![\w.])FileDescriptor\b", RAW_FD_TYPE, text)
    text = re.sub(r"(\bpackage\s+[\w.]+\s*;)",
                  rf"\1\nimport {RAW_FD_PACKAGE}.{RAW_FD_TYPE};", text, count=1)
    return text, True


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
    raw_fd_files = []
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
                text, raw = stage_raw_fds(text)
                if raw:
                    raw_fd_files.append(rel)
            open(f"{stage}/{rel}", "w").write(text)
            staged.append(rel)
    if raw_fd_files:
        declaration = os.path.join(stage, *RAW_FD_PACKAGE.split("."), f"{RAW_FD_TYPE}.aidl")
        os.makedirs(os.path.dirname(declaration), exist_ok=True)
        with open(declaration, "w") as file:
            file.write(RAW_FD_DECLARATION)
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
        if raw_fd_files:
            f.write('\nmod raw_file_descriptor;\npub use raw_file_descriptor::RawFileDescriptor;\n')
    if raw_fd_files:
        shutil.copyfile(os.path.join(os.path.dirname(__file__), "raw_file_descriptor.rs"),
                        f"{new}/raw_file_descriptor.rs")
        with open(f"{new}/raw_file_descriptors.json", "w") as file:
            json.dump({"version": 1, "wire_type": "FileDescriptor", "rust_type": "RawFileDescriptor",
                       "sources": sorted(raw_fd_files)}, file, sort_keys=True, indent=2)
            file.write("\n")
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
