#!/usr/bin/env python3
"""Derive the closed namespace from the existing provider manifests."""

from __future__ import annotations

import csv
import pathlib
import re
import sys
from default_versions import load_defaults


ROOT = pathlib.Path(__file__).resolve().parents[2]
HERE = pathlib.Path(__file__).resolve().parent


def rows(relative: str) -> list[dict[str, str]]:
    with (ROOT / relative).open(newline="") as stream:
        return list(csv.DictReader(stream, delimiter="\t"))


def names(relative: str, predicate=lambda row: True) -> list[str]:
    return [row["symbol"] for row in rows(relative) if predicate(row)]


def main() -> int:
    output = pathlib.Path(sys.argv[1]) if len(sys.argv) == 2 else HERE / "generated"
    output.mkdir(parents=True, exist_ok=True)

    universe_rows = rows(
        "tools/bionic-libc-leaf-facade/imports/ndk-r28c-api35-arm64-libc.tsv"
    )
    universe = {row["symbol"]: row for row in universe_rows}
    claims: list[tuple[str, str]] = []

    claims += [(row["symbol"], "leaf") for row in universe_rows if row["category"] == "A"]
    claims += [(name, "allocator") for name in names(
        "tools/bionic-libc-allocator-facade/imports/ndk-r28c-api35-arm64-allocators.tsv"
    )]
    claims.append(("__errno", "errno"))
    claims += [(name, "filesystem") for name in names(
        "tools/bionic-fs-facade/manifests/imports.tsv"
    ) if name not in {"close", "read"}]
    claims += [(name, "central-fd-broker") for name in ("close", "read")]
    claims += [(name, "time") for name in names(
        "tools/bionic-time-facade/manifests/imports.tsv"
    )]
    claims += [(row["symbol"], "pthread") for row in rows(
        "tools/android-bionic-pthread-provider/libcxx-pthread-imports.tsv"
    ) if row["status"] == "supported"]
    claims += [(name, "process-state") for name in names(
        "tools/bionic-process-state-facade/manifests/imports.tsv"
    )]
    claims.append(("dl_iterate_phdr", "phdr"))
    claims += [(row["symbol"], "stdio") for row in rows(
        "tools/bionic-stdio-facade/manifests/imports.tsv"
    ) if not row["policy"].startswith("rejected-")]
    claims += [(name, "wide-stdio") for name in names(
        "tools/bionic-wide-stdio-facade/manifests/imports.tsv"
    )]
    claims += [(name, "scanf") for name in names(
        "tools/bionic-scanf-facade/manifests/imports.tsv"
    )]
    claims += [(name, "swprintf") for name in names(
        "tools/bionic-swprintf-facade/manifests/imports.tsv"
    )]
    claims += [(name, "ioctl") for name in names(
        "tools/bionic-ioctl-facade/manifests/imports.tsv"
    )]
    claims += [(name, "strftime") for name in names(
        "tools/bionic-strftime-facade/manifests/imports.tsv"
    )]
    claims += [(name, "sendfile") for name in names(
        "tools/bionic-sendfile-facade/manifests/imports.tsv"
    )]
    claims += [(name, "locale") for name in names(
        "tools/bionic-locale-facade/manifests/imports.tsv"
    )]
    claims += [(name, "numeric") for name in names(
        "tools/bionic-numeric-facade/manifests/imports.tsv"
    )]
    claims += [(name, "float-conversion") for name in names(
        "tools/bionic-float-conversion-facade/manifests/imports.tsv"
    )]
    claims += [(row["symbol"], "format") for row in rows(
        "tools/bionic-format-facade/manifests/imports.tsv") if row["status"] == "supported"]
    claims += [(name, "formatted-stdio") for name in names(
        "tools/bionic-formatted-stdio-facade/manifests/imports.tsv")]
    claims += [(name, "strerror") for name in names(
        "tools/bionic-strerror-facade/manifests/imports.tsv")]
    claims += [(name, "wide-integer") for name in names(
        "tools/bionic-wide-integer-facade/manifests/imports.tsv")]
    claims += [(name, "wide-float") for name in names(
        "tools/bionic-wide-float-facade/manifests/imports.tsv")]
    claims += [(name, "binary128-conversion") for name in names(
        "tools/bionic-binary128-conversion-facade/manifests/imports.tsv")]
    claims += [(name, "abort") for name in names(
        "tools/bionic-abort-facade/manifests/imports.tsv")]
    claims += [(name, "syslog") for name in names(
        "tools/bionic-syslog-facade/manifests/imports.tsv")]
    claims += [(name, "syscall") for name in names(
        "tools/bionic-syscall-facade/manifests/imports.tsv")]
    with (ROOT / "tools/bionic-dso-lifecycle-facade/manifests/imports.tsv").open() as stream:
        claims += [(line.split("\t", 1)[0], "dso-lifecycle")
                   for line in stream if line.strip()]

    extensions = rows("tools/bionic-provider-namespace/extensions.tsv")
    extension_claims = {
        (row["symbol"], row["owner"])
        for row in extensions
        if row["soname"] in {"libc.so", "libdl.so"}
    }
    claimed: dict[str, str] = {}
    duplicates: list[str] = []
    for symbol, owner in claims:
        if symbol not in universe:
            if (symbol, owner) not in extension_claims:
                raise SystemExit(
                    f"provider claims unreviewed namespace extension: {symbol} ({owner})"
                )
            continue
        if symbol in claimed:
            duplicates.append(f"{symbol}: {claimed[symbol]}, {owner}")
        claimed[symbol] = owner
    if duplicates:
        raise SystemExit("duplicate owners:\n" + "\n".join(duplicates))

    liblog_source = (ROOT / "tools/android-liblog-exec-provider/liblog_provider.cc").read_text()
    liblog_block = liblog_source.split("const Entry kEntries[] = {", 1)[1].split("};", 1)[0]
    liblog = re.findall(r"^\s*LIBLOG_ENTRY\(([^)]+)\),", liblog_block, re.MULTILINE)
    liblog += re.findall(
        r'^\s*\{"(__android_log_[^"]+)"\s*,', liblog_block, re.MULTILINE
    )
    if len(liblog) != 19 or len(set(liblog)) != 19:
        raise SystemExit("liblog provider surface is not exactly 19 unique symbols")
    defaults = load_defaults(HERE)
    liblog_versions = defaults["liblog.so"]
    if any(symbol not in liblog_versions for symbol in liblog):
        raise SystemExit("liblog provider export absent from original ELF metadata")

    owner_enum = {
        "leaf": "DARWIN_ART_BIONIC_PROVIDER_LEAF",
        "allocator": "DARWIN_ART_BIONIC_PROVIDER_ALLOCATOR",
        "errno": "DARWIN_ART_BIONIC_PROVIDER_ERRNO",
        "filesystem": "DARWIN_ART_BIONIC_PROVIDER_FILESYSTEM",
        "time": "DARWIN_ART_BIONIC_PROVIDER_TIME",
        "pthread": "DARWIN_ART_BIONIC_PROVIDER_PTHREAD",
        "process-state": "DARWIN_ART_BIONIC_PROVIDER_PROCESS_STATE",
        "phdr": "DARWIN_ART_BIONIC_PROVIDER_PHDR",
        "stdio": "DARWIN_ART_BIONIC_PROVIDER_STDIO",
        "wide-stdio": "DARWIN_ART_BIONIC_PROVIDER_WIDE_STDIO",
        "scanf": "DARWIN_ART_BIONIC_PROVIDER_SCANF",
        "swprintf": "DARWIN_ART_BIONIC_PROVIDER_SWPRINTF",
        "ioctl": "DARWIN_ART_BIONIC_PROVIDER_IOCTL",
        "strftime": "DARWIN_ART_BIONIC_PROVIDER_STRFTIME",
        "sendfile": "DARWIN_ART_BIONIC_PROVIDER_SENDFILE",
        "locale": "DARWIN_ART_BIONIC_PROVIDER_LOCALE",
        "numeric": "DARWIN_ART_BIONIC_PROVIDER_NUMERIC",
        "float-conversion": "DARWIN_ART_BIONIC_PROVIDER_FLOAT_CONVERSION",
        "format": "DARWIN_ART_BIONIC_PROVIDER_FORMAT",
        "strerror": "DARWIN_ART_BIONIC_PROVIDER_STRERROR",
        "wide-integer": "DARWIN_ART_BIONIC_PROVIDER_WIDE_INTEGER",
        "wide-float": "DARWIN_ART_BIONIC_PROVIDER_WIDE_FLOAT",
        "binary128-conversion": "DARWIN_ART_BIONIC_PROVIDER_BINARY128_CONVERSION",
        "abort": "DARWIN_ART_BIONIC_PROVIDER_ABORT",
        "liblog": "DARWIN_ART_BIONIC_PROVIDER_LIBLOG",
        "dso-lifecycle": "DARWIN_ART_BIONIC_PROVIDER_DSO_LIFECYCLE",
        "syslog": "DARWIN_ART_BIONIC_PROVIDER_SYSLOG",
        "formatted-stdio": "DARWIN_ART_BIONIC_PROVIDER_FORMATTED_STDIO",
        "syscall": "DARWIN_ART_BIONIC_PROVIDER_SYSCALL",
        "central-fd-broker": "DARWIN_ART_BIONIC_PROVIDER_CENTRAL_FD_BROKER",
        "socket": "DARWIN_ART_BIONIC_PROVIDER_SOCKET",
        "dns": "DARWIN_ART_BIONIC_PROVIDER_DNS",
        "math": "DARWIN_ART_BIONIC_PROVIDER_MATH",
        "vm": "DARWIN_ART_BIONIC_PROVIDER_VM",
        "binder-ndk": "DARWIN_ART_BIONIC_PROVIDER_BINDER_NDK",
        "aaudio": "DARWIN_ART_BIONIC_PROVIDER_AAUDIO",
    }
    extension_keys: set[tuple[str, str, str]] = set()
    for row in extensions:
        key = (row["soname"], row["symbol"], row["version"])
        if (
            row["owner"] not in owner_enum
            or row["symbol"] in universe
            or key in extension_keys
        ):
            raise SystemExit(f"invalid provider namespace extension: {row}")
        extension_keys.add(key)
    ownership = sorted(
        [("libdl.so" if owner == "phdr" else "libc.so",
          symbol, "LIBC", owner) for symbol, owner in claimed.items()]
        + [(row["soname"], row["symbol"], row["version"], row["owner"])
           for row in extensions]
        + [("liblog.so", symbol, liblog_versions[symbol], "liblog")
           for symbol in liblog]
    )
    unsupported = sorted(
        (symbol, row["category"], row["rationale"])
        for symbol, row in universe.items() if symbol not in claimed
    )
    if len(universe) != 160 or len(claimed) != 160 or len(unsupported) != 0:
        raise SystemExit(
            f"coverage drift: universe={len(universe)} owned={len(claimed)} "
            f"unsupported={len(unsupported)}"
        )

    with (output / "ownership.tsv").open("w", newline="") as stream:
        writer = csv.writer(stream, delimiter="\t", lineterminator="\n")
        writer.writerow(("soname", "symbol", "version", "owner"))
        writer.writerows(ownership)
    with (output / "unsupported-libc.tsv").open("w", newline="") as stream:
        writer = csv.writer(stream, delimiter="\t", lineterminator="\n")
        writer.writerow(("symbol", "class", "reason"))
        writer.writerows(unsupported)
    with (output / "liblog_versions.inc").open("w") as stream:
        for symbol in sorted(liblog):
            stream.write(f'    {{"{symbol}", "{liblog_versions[symbol]}"}},\n')
    with (output / "ownership.inc").open("w") as stream:
        for soname, symbol, version, owner in ownership:
            # A known original definition selects only its real default version.
            # Other providers retain explicit unversioned routes until their
            # own original-image metadata is imported.
            default = (defaults[soname].get(symbol) == version if soname in defaults
                       else version == "")
            stream.write(
                f'    {{"{soname}", "{symbol}", "{version}", {owner_enum[owner]}, {str(default).lower()}}},\n'
            )
    with (output / "unsupported_symbols.inc").open("w") as stream:
        for symbol, _category, _reason in unsupported:
            stream.write(f'    "{symbol}",\n')
    with (output / "unsupported.inc").open("w") as stream:
        for symbol, category, reason in unsupported:
            stream.write(f'    {{"{symbol}", \'{category}\', "{reason}"}},\n')
    (output / "unsupported_count.inc").write_text(f"{len(unsupported)}\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
