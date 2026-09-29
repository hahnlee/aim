#!/usr/bin/env python3
"""Fetches files of the pinned official CTS release (upstream/cts.lock) into
_build/cts, without downloading the whole archive: the release zip is about
18 GB, so its central directory and the pinned entries are read with HTTP
range requests, inflated and checked against their pinned sha256.

The CTS is a test input (docs/system-services.md, "Conformance"): it is
never shipped or committed.

Usage: cts-module.py [--list PATTERN]
  --list PATTERN  print the entries of the archive whose name contains
                  PATTERN (to pin new ones), instead of fetching
"""

import hashlib
import os
import struct
import subprocess
import sys
import zlib

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))


def lock():
    values, entries = {}, []
    for line in open(os.path.join(ROOT, "upstream/cts.lock")):
        line = line.split("#", 1)[0].strip()
        if not line:
            continue
        if "=" in line and not line.startswith('"'):
            key, value = line.split("=", 1)
            values[key] = value
        elif line.startswith('"'):
            entries.append(line.strip('"').split("|"))
    return values, entries


def fetch(url, start, length):
    out = subprocess.run(
        ["curl", "-sfL", "--retry", "5", "-r", f"{start}-{start + length - 1}", url],
        capture_output=True,
        check=True,
    ).stdout
    if len(out) != length:
        sys.exit(f"cts-module: short read of {url} at {start}")
    return out


def central_directory(url, size):
    tail = fetch(url, size - 65536, 65536)
    at = tail.rfind(b"PK\x06\x07")
    if at < 0:
        sys.exit("cts-module: no zip64 end of central directory locator")
    (eocd,) = struct.unpack("<Q", tail[at + 8 : at + 16])
    record = tail[eocd - (size - 65536) :][:56]
    cd_size, cd_offset = struct.unpack("<QQ", record[40:56])
    data = fetch(url, cd_offset, cd_size)
    entries, i = {}, 0
    while data[i : i + 4] == b"PK\x01\x02":
        method, = struct.unpack("<H", data[i + 10 : i + 12])
        csize, usize = struct.unpack("<II", data[i + 20 : i + 28])
        nl, el, cl = struct.unpack("<HHH", data[i + 28 : i + 34])
        (offset,) = struct.unpack("<I", data[i + 42 : i + 46])
        name = data[i + 46 : i + 46 + nl].decode()
        extra, j = data[i + 46 + nl : i + 46 + nl + el], 0
        while j + 4 <= len(extra):
            hid, hs = struct.unpack("<HH", extra[j : j + 4])
            if hid == 1:
                # zip64: the 64-bit values of the fields that overflowed.
                wide = iter(struct.unpack(f"<{hs // 8}Q", extra[j + 4 : j + 4 + hs]))
                usize = next(wide) if usize == 0xFFFFFFFF else usize
                csize = next(wide) if csize == 0xFFFFFFFF else csize
                offset = next(wide) if offset == 0xFFFFFFFF else offset
            j += 4 + hs
        entries[name] = (offset, csize, usize, method)
        i += 46 + nl + el + cl
    return entries


def main():
    values, pinned = lock()
    url, size = values["URL"], int(values["SIZE"])
    entries = central_directory(url, size)
    if len(sys.argv) == 3 and sys.argv[1] == "--list":
        for name, (_, csize, usize, _) in sorted(entries.items()):
            if sys.argv[2] in name:
                print(f"{usize:>12} {name}")
        return
    dest_root = os.path.join(ROOT, "_build", "cts")
    for name, want in pinned:
        dest = os.path.join(dest_root, os.path.basename(name))
        if os.path.exists(dest) and hashlib.sha256(open(dest, "rb").read()).hexdigest() == want:
            continue
        offset, csize, usize, method = entries[name]
        header = fetch(url, offset, 30)
        nl, el = struct.unpack("<HH", header[26:30])
        body = fetch(url, offset + 30 + nl + el, csize)
        data = zlib.decompressobj(-15).decompress(body) if method == 8 else body
        got = hashlib.sha256(data).hexdigest()
        if len(data) != usize or got != want:
            sys.exit(f"cts-module: {name}: sha256 {got}, pinned {want}")
        os.makedirs(dest_root, exist_ok=True)
        with open(dest + ".partial", "wb") as f:
            f.write(data)
        os.replace(dest + ".partial", dest)
        print(f"fetched {dest}")


if __name__ == "__main__":
    main()
