#!/usr/bin/env python3
"""Generate the Darwin-to-Android errno mapping from OsConstants values."""

import pathlib
import re
import sys


def errnos(path: pathlib.Path) -> list[tuple[str, int]]:
    result = []
    for line in path.read_text(encoding="utf-8").splitlines():
        name, value = line.split("\t")
        if re.fullmatch(r"E[A-Z0-9_]+", name) and not name.startswith(
            ("EAI_", "ETH_", "EXIT_")
        ):
            result.append((name, int(value)))
    if len(result) != 80:
        raise ValueError(f"expected 80 Android errno constants, found {len(result)}")
    return result


def main() -> int:
    if len(sys.argv) != 3 or sys.argv[2] not in {"include", "probe", "table"}:
        print("usage: generate_mapping.py <values-or-manifest.tsv> include|probe|table",
              file=sys.stderr)
        return 2
    if sys.argv[2] == "table":
        print("/* Generated from manifests/darwin-to-android.tsv; do not edit. */")
        for line in pathlib.Path(sys.argv[1]).read_text(encoding="utf-8").splitlines():
            name, darwin_value, android_value = line.split("\t")
            if not re.fullmatch(r"E[A-Z0-9_]+", name):
                raise ValueError(f"invalid errno name {name!r}")
            print(f"  {{{darwin_value}, {android_value}}}, /* {name} */")
        return 0
    values = errnos(pathlib.Path(sys.argv[1]))
    if sys.argv[2] == "include":
        print("/* Generated from Android 16 OsConstants; do not edit. */")
        for name, value in values:
            print(f"#ifdef {name}")
            print(
                f"  if (darwin_errno == {name}) {{ *android_errno = {value}; return 1; }}"
            )
            print("#endif")
    else:
        print("#include <errno.h>")
        print("#include <stdio.h>")
        print("int main(void) {")
        for name, value in values:
            print(f"#ifdef {name}")
            print(f'  printf("{name}\\t%d\\t{value}\\n", {name});')
            print("#endif")
        print("  return 0;")
        print("}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
