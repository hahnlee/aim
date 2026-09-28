#!/usr/bin/env python3
import binascii
import pathlib
import struct
import sys
import zlib

PAGE = 16384

def raw_deflate(data):
    compressor = zlib.compressobj(9, zlib.DEFLATED, -15)
    return compressor.compress(data) + compressor.flush()

def build(output, variant, inputs):
    items = [(f"lib/arm64-v8a/{path.name}".encode(), path.read_bytes()) for path in inputs]
    items.append((b"assets/race-byte", b"R"))
    if variant == "traversal":
        items.append((b"lib/arm64-v8a/../evil.so", b"evil"))
    if variant == "duplicate":
        items.append(items[0])
    body = bytearray()
    central = []
    for index, (name, data) in enumerate(items):
        method = 8 if variant == "deflated" and index == 0 else 0
        flags = 1 if variant == "encrypted" and index == 0 else 0
        if variant == "descriptor" and index == 0:
            flags |= 8
        encoded = raw_deflate(data) if method == 8 else data
        crc = binascii.crc32(data) & 0xffffffff
        local_offset = len(body)
        base = local_offset + 30 + len(name)
        padding = (-base) % PAGE if name.startswith(b"lib/arm64-v8a/") else 0
        if variant == "misaligned" and index == 0:
            padding = 0
        if 0 < padding < 4:
            padding += PAGE
        extra = b"" if padding == 0 else struct.pack("<HH", 0xD935, padding - 4) + bytes(padding - 4)
        local_crc = 0 if flags & 8 else crc
        local_comp = 0 if flags & 8 else len(encoded)
        local_uncomp = 0 if flags & 8 else len(data)
        local_method = 8 if variant == "local-mismatch" and index == 0 else method
        body += struct.pack("<IHHHHHIIIHH", 0x04034B50, 20, flags, local_method, 0, 0,
                            local_crc, local_comp, local_uncomp, len(name), len(extra))
        body += name + extra + encoded
        if flags & 8:
            body += struct.pack("<IIII", 0x08074B50, crc, len(encoded), len(data))
        central.append((name, flags, method, crc, len(encoded), len(data), local_offset))
    central_offset = len(body)
    for index, (name, flags, method, crc, comp, uncomp, local_offset) in enumerate(central):
        central_comp = 0xffffffff if variant == "zip64" and index == 0 else comp
        body += struct.pack("<IHHHHHHIIIHHHHHII", 0x02014B50, 0x0314, 20, flags,
                            method, 0, 0, crc, central_comp, uncomp, len(name), 0, 0,
                            0, 0, 0o100444 << 16, local_offset)
        body += name
    central_size = len(body) - central_offset
    body += struct.pack("<IHHHHIIH", 0x06054B50, 0, 0, len(central), len(central),
                        central_size, central_offset, 0)
    if variant == "crc":
        first_data = PAGE
        body[first_data] ^= 1
    output.write_bytes(body)
    output.chmod(0o400)

if __name__ == "__main__":
    if len(sys.argv) != 6:
        raise SystemExit("usage: make_fixture.py OUTPUT VARIANT ROOT CHILD GRANDCHILD")
    build(pathlib.Path(sys.argv[1]), sys.argv[2], [pathlib.Path(x) for x in sys.argv[3:]])
