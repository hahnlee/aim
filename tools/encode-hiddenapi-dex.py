#!/usr/bin/env python3
"""Encode AOSP hidden-api CSV flags into a standard DEX file in place."""

from __future__ import annotations

import argparse
import hashlib
from pathlib import Path
import struct
import zlib


FLAG_VALUES = {
    "sdk": 0,
    "unsupported": 1,
    "blocked": 2,
    "max-target-o": 3,
    "max-target-p": 4,
    "max-target-q": 5,
    "max-target-r": 6,
    "max-target-s": 7,
    # HiddenApiAccessFlags::kFuture follows the last released target SDK
    # bucket and is encoded as the next four-bit value (8).  Keep the
    # encoder aligned with ART's hiddenapi_flags.h so future-target corpus
    # inputs are represented faithfully without a test-specific exception.
    "max-target-future": 8,
    "core-platform-api": 1 << 4,
    "test-api": 1 << 5,
}


def u32(data: bytes | bytearray, offset: int) -> int:
    return struct.unpack_from("<I", data, offset)[0]


def read_uleb(data: bytes | bytearray, offset: int) -> tuple[int, int]:
    value = 0
    shift = 0
    while True:
        byte = data[offset]
        offset += 1
        value |= (byte & 0x7f) << shift
        if byte < 0x80:
            return value, offset
        shift += 7
        if shift >= 35:
            raise ValueError("invalid DEX uleb128")


def write_uleb(value: int) -> bytes:
    result = bytearray()
    while True:
        byte = value & 0x7f
        value >>= 7
        result.append(byte | (0x80 if value else 0))
        if not value:
            return bytes(result)


def align4(data: bytearray) -> None:
    data.extend(b"\0" * (-len(data) % 4))


def encode(path: Path, flags: dict[str, int]) -> None:
    data = bytearray(path.read_bytes())
    header_size = u32(data, 36)
    if data[:4] != b"dex\n" or header_size not in (112, 120):
        raise ValueError(f"unsupported non-standard DEX: {path}")
    # DEX 041 appends container_size/header_offset after the legacy Header.
    # Its legacy data_size/data_off fields stay zero; offsets address the whole
    # container. Earlier DEX versions still require those two fields updated.

    string_count, string_off = u32(data, 56), u32(data, 60)
    type_count, type_off = u32(data, 64), u32(data, 68)
    proto_count, proto_off = u32(data, 72), u32(data, 76)
    field_count, field_off = u32(data, 80), u32(data, 84)
    method_count, method_off = u32(data, 88), u32(data, 92)
    class_count, class_off = u32(data, 96), u32(data, 100)

    strings: list[str] = []
    for index in range(string_count):
        cursor = u32(data, string_off + index * 4)
        _, cursor = read_uleb(data, cursor)
        end = data.index(0, cursor)
        strings.append(bytes(data[cursor:end]).decode("utf-8"))
    types = [strings[u32(data, type_off + index * 4)] for index in range(type_count)]

    protos: list[str] = []
    for index in range(proto_count):
        _, return_type, parameters_off = struct.unpack_from(
            "<III", data, proto_off + index * 12)
        parameters: list[str] = []
        if parameters_off:
            size = u32(data, parameters_off)
            parameters = [
                types[struct.unpack_from("<H", data, parameters_off + 4 + i * 2)[0]]
                for i in range(size)
            ]
        protos.append(f"({''.join(parameters)}){types[return_type]}")

    fields: list[str] = []
    for index in range(field_count):
        class_idx, type_idx, name_idx = struct.unpack_from(
            "<HHI", data, field_off + index * 8)
        fields.append(f"{types[class_idx]}->{strings[name_idx]}:{types[type_idx]}")
    methods: list[str] = []
    for index in range(method_count):
        class_idx, proto_idx, name_idx = struct.unpack_from(
            "<HHI", data, method_off + index * 8)
        methods.append(f"{types[class_idx]}->{strings[name_idx]}{protos[proto_idx]}")

    hidden = bytearray(4 * (class_count + 1))
    for class_index in range(class_count):
        class_data_off = u32(data, class_off + class_index * 32 + 24)
        if not class_data_off:
            continue
        cursor = class_data_off
        counts = []
        for _ in range(4):
            count, cursor = read_uleb(data, cursor)
            counts.append(count)
        encoded_flags = bytearray()
        any_restricted = False
        for count, table, is_method in (
            (counts[0], fields, False), (counts[1], fields, False),
            (counts[2], methods, True), (counts[3], methods, True),
        ):
            member_index = 0
            for _ in range(count):
                index_delta, cursor = read_uleb(data, cursor)
                member_index += index_delta
                _, cursor = read_uleb(data, cursor)  # access flags
                if is_method:
                    _, cursor = read_uleb(data, cursor)  # code offset
                value = flags.get(table[member_index], 0)
                encoded_flags.extend(write_uleb(value))
                any_restricted |= value != 0
        if any_restricted:
            struct.pack_into("<I", hidden, 4 + class_index * 4, len(hidden))
            hidden.extend(encoded_flags)
    if len(hidden) == 4 * (class_count + 1):
        return
    struct.pack_into("<I", hidden, 0, len(hidden))

    map_off = u32(data, 52)
    map_count = u32(data, map_off)
    old_map_size = 4 + map_count * 12
    if map_off + old_map_size != len(data):
        raise ValueError("DEX map list is not the final section")
    items = [
        struct.unpack_from("<HHII", data, map_off + 4 + i * 12)
        for i in range(map_count)
    ]
    items = [item for item in items if item[0] not in (0x1000, 0xF000)]
    data[map_off:] = b"\0" * old_map_size
    align4(data)
    hidden_off = len(data)
    data.extend(hidden)
    align4(data)
    new_map_off = len(data)
    items.extend(((0xF000, 0, 1, hidden_off), (0x1000, 0, 1, new_map_off)))
    data.extend(struct.pack("<I", len(items)))
    for item in items:
        data.extend(struct.pack("<HHII", *item))

    struct.pack_into("<I", data, 52, new_map_off)
    struct.pack_into("<I", data, 32, len(data))
    if header_size == 120:
        if u32(data, 116) != 0:
            raise ValueError("multi-DEX 041 containers are not supported")
        struct.pack_into("<I", data, 112, len(data))
    else:
        struct.pack_into("<I", data, 104, len(data) - u32(data, 108))
    data[12:32] = hashlib.sha1(data[32:]).digest()
    struct.pack_into("<I", data, 8, zlib.adler32(data[12:]) & 0xFFFFFFFF)
    path.write_bytes(data)


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--api-flags", type=Path, required=True)
    parser.add_argument("dex", type=Path, nargs="+")
    args = parser.parse_args()
    flags: dict[str, int] = {}
    for raw_line in args.api_flags.read_text().splitlines():
        line = raw_line.strip()
        if not line or line.startswith("#"):
            continue
        signature, *names = line.split(",")
        value = 0
        for name in names:
            value |= FLAG_VALUES[name]
        flags[signature] = value
    for dex in args.dex:
        encode(dex, flags)


if __name__ == "__main__":
    main()
