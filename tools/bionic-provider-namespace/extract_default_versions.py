#!/usr/bin/env python3
"""Extract default exports from a hash-verified original ELF, never infer LIBC.

Print TSV to stdout. This is metadata preparation, not provider registration.
"""
import argparse
import hashlib
import pathlib
import re
import subprocess


def defaults(text):
    result = {}
    for line in text.splitlines():
        fields = line.split()
        if len(fields) != 8 or not re.fullmatch(r"\d+:", fields[0]):
            continue
        _, _, _, kind, binding, visibility, section, name = fields
        if section == "UND" or binding not in ("GLOBAL", "WEAK") or visibility not in ("DEFAULT", "PROTECTED"):
            continue
        if kind not in ("FUNC", "OBJECT", "TLS", "IFUNC", "NOTYPE"):
            continue
        if "@@" in name:
            symbol, version = name.split("@@", 1)
        elif "@" in name:
            continue  # Non-default compatibility version is not eligible.
        else:
            symbol, version = name, ""
        if symbol in result and result[symbol] != version:
            raise ValueError(f"multiple default definitions: {symbol}")
        result[symbol] = version
    if not result:
        raise ValueError("no default dynamic exports parsed")
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--readelf", required=True)
    parser.add_argument("--sha256", required=True)
    parser.add_argument("image", type=pathlib.Path)
    parser.add_argument("--output", type=pathlib.Path)
    args = parser.parse_args()
    if hashlib.sha256(args.image.read_bytes()).hexdigest() != args.sha256:
        raise SystemExit("original image SHA-256 mismatch")
    text = subprocess.check_output([args.readelf, "--dyn-syms", "--wide", str(args.image)], text=True)
    result = "symbol\tdefault_version\n" + "".join(
        f"{symbol}\t{version}\n" for symbol, version in sorted(defaults(text).items()))
    if args.output:
        args.output.write_text(result)
    else:
        print(result, end="")


if __name__ == "__main__":
    main()
