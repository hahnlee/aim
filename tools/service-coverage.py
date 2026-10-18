#!/usr/bin/env python3
"""The app host's coverage of the original system services' binder surface
(ADR 0013, ADR 0014: the AIDL axis).

For each service the original system registers (`service list` of a full
boot of the derived image), it reports whether the app host registers it
and how many of its interface's transactions the app host's dispatch
serves, and who provides it there: the app host, the ADR 0013 native
service host (`image/native-services`), or the original in the guest.
An interface's transactions are its stub's `TRANSACTION_*` fields
in the original image's jars (read with the SDK's dexdump); the app host
serves those its `module::CONSTANT =>` arms in
crates/aim-services/src name, by the codes `aidl-gen` generated
from the pinned AIDL (target/aim/gen/service-aidl/lib.rs). A transaction
answered outside such an arm is not counted, so the figures are a lower
bound.

Usage:
  tools/service-coverage.py ORIGINAL_SERVICES APP_HOST_SERVICES [--json OUT]
    *_SERVICES: the output of `service list` in a full boot and in the app host
"""

import json
import os
import pathlib
import re
import subprocess
import sys
import tempfile
import zipfile

ROOT = pathlib.Path(__file__).resolve().parent.parent
GENERATED = ROOT / "target/aim/gen/service-aidl/lib.rs"
SERVICES = ROOT / "crates/aim-services/src"
NATIVE = ROOT / "image/native-services"
IMAGE = ROOT / "_build/android16-image/root"
DEXDUMP = pathlib.Path(os.environ.get("ANDROID_SDK_ROOT", pathlib.Path.home() / "Library/Android/sdk")) / "build-tools/36.0.0/dexdump"


def stubs():
    """Descriptor -> {method: code} of every AIDL stub in the image's jars."""
    jars = list((IMAGE / "system/framework").glob("*.jar"))
    jars += list((IMAGE / "apex").glob("*/javalib/*.jar"))
    out = {}
    with tempfile.TemporaryDirectory() as tmp:
        for jar in jars:
            with zipfile.ZipFile(jar) as z:
                for member in z.namelist():
                    if not re.fullmatch(r"classes\d*\.dex", member):
                        continue
                    dex = pathlib.Path(tmp) / "classes.dex"
                    dex.write_bytes(z.read(member))
                    dump = subprocess.run([DEXDUMP, dex], capture_output=True).stdout.decode("utf-8", "replace")
                    parse_stubs(dump, out)
    return out


def parse_stubs(dump, out):
    descriptor, codes, field = None, None, None
    for line in dump.splitlines():
        m = re.match(r"  Class descriptor  : 'L(.*)\$Stub;'", line)
        if m:
            descriptor, codes = m.group(1).replace("/", "."), {}
            out[descriptor] = codes
            continue
        if line.startswith("  Class descriptor"):
            descriptor = None
            continue
        if descriptor is None:
            continue
        m = re.match(r"      name          : '(\w+)'", line)
        if m:
            field = m.group(1)
            continue
        m = re.match(r'      value         : (.*)', line)
        if m and field:
            if field.startswith("TRANSACTION_") and m.group(1).isdigit():
                codes[field[len("TRANSACTION_"):]] = int(m.group(1))
            elif field == "DESCRIPTOR" and m.group(1).startswith('"'):
                out[m.group(1).strip('"')] = out.pop(descriptor)
                descriptor = m.group(1).strip('"')
            field = None


def interfaces():
    """Generated module name -> (descriptor, {transaction name: code})."""
    out, module, descriptor, codes = {}, None, None, {}
    for line in GENERATED.read_text().splitlines():
        m = re.match(r"pub mod (\w+) \{", line)
        if m:
            if module and descriptor:
                out[module] = (descriptor, codes)
            module, descriptor, codes = m.group(1), None, {}
            continue
        m = re.match(r'    pub const DESCRIPTOR: &str = "([\w.$]+)";', line)
        if m:
            descriptor = m.group(1)
            continue
        m = re.match(r"    pub const ([A-Z][A-Z0-9_]*): u32 = (\d+);", line)
        if m and descriptor:
            codes[m.group(1)] = int(m.group(2))
    if module and descriptor:
        out[module] = (descriptor, codes)
    return out


def served(modules):
    """Descriptor -> transaction codes some app host dispatch arm serves."""
    out = {}
    for path in SERVICES.rglob("*.rs"):
        text = path.read_text()
        aliases = {}
        for m in re.finditer(r"aim_service_aidl::(\w+)(?: as (\w+))?", text):
            aliases[m.group(2) or m.group(1)] = m.group(1)
        for m in re.finditer(r"use aim_service_aidl::\{([^}]*)\}", text):
            for item in m.group(1).split(","):
                parts = item.split(" as ")
                name = parts[0].strip()
                if name:
                    aliases[(parts[1] if len(parts) > 1 else name).strip()] = name
        for line in text.splitlines():
            if "=>" not in line:
                continue
            arm = line.split("=>")[0]
            for m in re.finditer(r"(\w+)::([A-Z][A-Z0-9_]*)", arm):
                module = aliases.get(m.group(1), m.group(1))
                if module in modules and m.group(2) in modules[module][1]:
                    out.setdefault(modules[module][0], set()).add(modules[module][1][m.group(2)])
    return out


def service_list(path):
    """Name -> descriptor from `service list` output."""
    out = {}
    for line in pathlib.Path(path).read_text().splitlines():
        m = re.match(r"\d+\s+(\S+): \[([^\]]*)\]", line)
        if m:
            out[m.group(1)] = m.group(2)
    return out


def main():
    args = sys.argv[1:]
    out_json = None
    if "--json" in args:
        i = args.index("--json")
        out_json = args[i + 1]
        del args[i : i + 2]
    if len(args) != 2:
        sys.exit(__doc__)
    modules = interfaces()
    by_descriptor = stubs()
    implemented = served(modules)
    original, app_host = service_list(args[0]), service_list(args[1])
    native = {line.split()[0] for line in NATIVE.read_text().splitlines() if line.strip() and not line.startswith("#")}
    rows = []
    for name, descriptor in sorted(original.items()):
        codes = by_descriptor.get(descriptor)
        done = implemented.get(descriptor, set())
        if name not in app_host:
            provider = None
        elif name in native:
            provider = "service host"
        elif done:
            provider = "app host"
        else:
            provider = "guest"
        rows.append({
            "service": name,
            "descriptor": descriptor,
            "registered": name in app_host,
            "provider": provider,
            "methods": len(codes) if codes is not None else None,
            "served": len(done) if codes is not None else None,
            "missing": sorted(m for m, c in codes.items() if c not in done) if codes is not None else None,
        })
    registered = sum(r["registered"] for r in rows)
    known = [r for r in rows if r["methods"] is not None]
    methods = sum(r["methods"] for r in known)
    served_n = sum(r["served"] for r in known if r["provider"] in ("app host", "service host"))
    print(f"services: {registered} of {len(rows)} the original registers are in the app host")
    print(f"methods:  {served_n} of {methods} transactions of the {len(known)} interfaces with a stub served")
    for r in rows:
        state = r["provider"] or "absent"
        count = f"{r['served']}/{r['methods']}" if r["methods"] is not None else "no stub"
        print(f"  {r['service']:<40} {state:<12} {count:<14} {r['descriptor']}")
    if out_json:
        pathlib.Path(out_json).write_text(json.dumps(rows, indent=1))


if __name__ == "__main__":
    main()
