#!/usr/bin/env python3
"""Summarizes a binder trace (`guest-init --binder-trace FILE`,
docs/system-services.md): per process, which services, interfaces and
methods it called, how often, and the driver's p50/p99 latency; then,
per service, where the synchronous calls' time went: the wake of a
target thread (or the wait for a free one), the target's work, and the
sender's wake with the reply.

Method names come from the image itself: every AIDL Java stub
(`<Interface>$Stub`) in the image's jars declares a `TRANSACTION_<method>`
constant per code, and its interface a `DESCRIPTOR`; dexdump reads them.
Service names come from `service list` (`name: [descriptor]`). Interfaces
without a Java stub (native AIDL such as SurfaceFlinger's) show their codes.

Usage: binder-trace-report.py --image ROOT --services SERVICE_LIST
           [--pid PID=NAME]... [--dexdump PATH] TRACE
  --pid   the processes to report (default: every sender), e.g.
          --pid 48328=Settings
"""

import argparse
import collections
import glob
import os
import re
import subprocess
import sys
import tempfile
import zipfile

SPECIAL = {
    0x5F504E47: "PING",
    0x5F4E5446: "INTERFACE",
    0x5F444D50: "DUMP",
    0x5F434D44: "SHELL_COMMAND",
    0x5F535052: "SYSPROPS",
    0x5F455854: "EXTENSION",
    0x5F444247: "DEBUG_PID",
    0x5F52504C: "SET_RPC_CLIENT",
    0x5F434850: "TWEET",
    0x5F4C494B: "LIKE",
}
# android.os.IBinder.FIRST_CALL_TRANSACTION .. LAST_CALL_TRANSACTION
LAST_CALL = 0x00FFFFFF


def default_dexdump():
    sdk = os.environ.get("ANDROID_SDK_ROOT", os.path.expanduser("~/Library/Android/sdk"))
    found = sorted(glob.glob(f"{sdk}/build-tools/*/dexdump"))
    if not found:
        sys.exit("binder-trace-report: no dexdump in the SDK's build-tools; pass --dexdump")
    return found[-1]


def stub_methods(image, dexdump):
    """descriptor -> {code: method}, from every jar of the image."""
    jars = glob.glob(f"{image}/system/framework/*.jar")
    jars += glob.glob(f"{image}/system_ext/framework/*.jar")
    jars += glob.glob(f"{image}/apex/*/javalib/*.jar")
    descriptors = {}  # interface class -> descriptor
    codes = collections.defaultdict(dict)  # interface class -> {code: name}
    field = re.compile(r"name\s+: '(\w+)'")
    value = re.compile(r"value\s+: (.*)")
    with tempfile.TemporaryDirectory() as tmp:
        for jar in jars:
            try:
                archive = zipfile.ZipFile(jar)
            except zipfile.BadZipFile:
                continue
            for entry in archive.namelist():
                if not re.fullmatch(r"classes\d*\.dex", entry):
                    continue
                dex = os.path.join(tmp, "x.dex")
                with open(dex, "wb") as f:
                    f.write(archive.read(entry))
                out = subprocess.run(
                    [dexdump, dex], capture_output=True, text=True, errors="replace"
                ).stdout
                cls, name = None, None
                for line in out.splitlines():
                    if "Class descriptor" in line:
                        cls = line.split("'")[1]
                        continue
                    m = field.search(line)
                    if m:
                        name = m.group(1)
                        continue
                    m = value.search(line)
                    if not (m and cls and name):
                        continue
                    if cls.endswith("$Stub;") and name.startswith("TRANSACTION_"):
                        iface = cls[: -len("$Stub;")] + ";"
                        codes[iface][int(m.group(1))] = name[len("TRANSACTION_"):]
                    elif name == "DESCRIPTOR" and m.group(1).startswith('"'):
                        iface = cls[: -len("$Stub;")] + ";" if cls.endswith("$Stub;") else cls
                        descriptors[iface] = m.group(1).strip('"')
    return {descriptors[c]: codes[c] for c in codes if c in descriptors}


def service_names(path):
    names = collections.defaultdict(list)
    for line in open(path, errors="replace"):
        m = re.match(r"\d+\s+(\S+): \[(.*)\]", line.strip())
        if m and m.group(2):
            names[m.group(2)].append(m.group(1))
    return names


def percentile(values, p):
    values = sorted(values)
    return values[min(len(values) - 1, int(len(values) * p / 100))]


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--image", required=True)
    ap.add_argument("--services", required=True)
    ap.add_argument("--pid", action="append", default=[])
    ap.add_argument("--dexdump", default=None)
    ap.add_argument("trace")
    args = ap.parse_args()
    methods = stub_methods(args.image, args.dexdump or default_dexdump())
    names = service_names(args.services)
    wanted = dict(p.split("=", 1) for p in args.pid)

    calls = collections.defaultdict(list)  # (pid, descriptor, code) -> latencies
    oneway = collections.Counter()
    splits = collections.defaultdict(list)  # descriptor -> [(deliver, work, back, idle)]
    for line in open(args.trace, errors="replace"):
        f = line.rstrip("\n").split("\t")
        if len(f) != 13 or (wanted and f[2] not in wanted):
            continue
        key = (f[2], f[5], int(f[6]))
        if f[7] == "oneway":
            oneway[key] += 1
        calls[key].append(int(f[8]) if f[8] != "-" else None)
        if "-" not in f[8:11]:
            replied, delivered, returned = int(f[8]), int(f[9]), int(f[10])
            splits[f[5]].append((delivered, replied - delivered, returned - replied,
                                 int(f[11]) > 0))

    def method(descriptor, code):
        if code in SPECIAL:
            return SPECIAL[code]
        name = methods.get(descriptor, {}).get(code)
        return name if name else f"#{code}"

    by_pid = collections.defaultdict(list)
    for key, lat in calls.items():
        by_pid[key[0]].append((key, lat))
    for pid in sorted(by_pid, key=lambda p: wanted.get(p, p)):
        rows = by_pid[pid]
        total = sum(len(lat) for _, lat in rows)
        synced = [l for _, lat in rows for l in lat if l is not None]
        print(f"## {wanted.get(pid, pid)} (pid {pid}): {total} transactions, "
              f"{len(synced)} synchronous with a reply"
              + (f", p50 {percentile(synced, 50) / 1000:.0f} us, p99 "
                 f"{percentile(synced, 99) / 1000:.0f} us" if synced else ""))
        per_iface = collections.defaultdict(list)
        for (_, descriptor, code), lat in rows:
            per_iface[descriptor].append((code, lat))
        print()
        print("| service | interface | method | calls | p50 us | p99 us |")
        print("| --- | --- | --- | --- | --- | --- |")
        order = sorted(per_iface, key=lambda d: -sum(len(l) for _, l in per_iface[d]))
        for descriptor in order:
            service = ", ".join(names.get(descriptor, [])) or "-"
            for code, lat in sorted(per_iface[descriptor], key=lambda r: -len(r[1])):
                done = [l for l in lat if l is not None]
                n = oneway[(pid, descriptor, code)]
                kind = "" if not n else " (oneway)" if n == len(lat) else f" ({n} oneway)"
                p50 = f"{percentile(done, 50) / 1000:.0f}" if done else "-"
                p99 = f"{percentile(done, 99) / 1000:.0f}" if done else "-"
                print(f"| {service} | {descriptor or '-'} | {method(descriptor, code)}{kind} "
                      f"| {len(lat)} | {p50} | {p99} |")
        print()

    print("## Where the synchronous calls' time went (us)")
    print()
    print("| service | interface | calls | total p50 | p99 | wake/wait p50 | p99 "
          "| no idle thread | work p50 | p99 | return p50 | p99 |")
    print("| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |")
    rows = [(d, v) for d, v in splits.items()]
    everything = [x for _, v in rows for x in v]
    rows.append(("(all)", everything))
    for descriptor, v in sorted(rows, key=lambda r: -len(r[1])):
        if not v:
            continue
        us = lambda xs, p: f"{percentile(xs, p) / 1000:.0f}"
        total = [a + b + c for a, b, c, _ in v]
        deliver = [a for a, _, _, _ in v]
        work = [b for _, b, _, _ in v]
        back = [c for _, _, c, _ in v]
        busy = sum(1 for *_, idle in v if not idle)
        service = ", ".join(names.get(descriptor, [])) or "-"
        print(f"| {service} | {descriptor or '-'} | {len(v)} | {us(total, 50)} | {us(total, 99)} "
              f"| {us(deliver, 50)} | {us(deliver, 99)} | {100 * busy // len(v)} % "
              f"| {us(work, 50)} | {us(work, 99)} | {us(back, 50)} | {us(back, 99)} |")


if __name__ == "__main__":
    main()
