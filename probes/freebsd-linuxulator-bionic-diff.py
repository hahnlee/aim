#!/usr/bin/env python3
"""Differentially check source-derived Linuxulator constants against Bionic."""

from __future__ import annotations

import argparse
import csv
from pathlib import Path


def read_bionic(path: Path) -> dict[str, int]:
    values = {}
    with path.open(encoding="utf-8", newline="") as stream:
        for name, value in csv.reader(stream, delimiter="\t"):
            values[name] = int(value)
    return values


def normalize(domain: str, linuxulator_name: str) -> str:
    name = linuxulator_name.removeprefix("LINUX_")
    aliases = {
        "EOPNOTSUPPORT": "EOPNOTSUPP",
        "ESOCKNOTSUPPORT": "ESOCKTNOSUPPORT",
        "MAP_ANON": "MAP_ANONYMOUS",
    }
    return aliases.get(name, name)


def read_linuxulator(path: Path) -> list[tuple[str, str, int]]:
    rows = []
    with path.open(encoding="utf-8", newline="") as stream:
        for row in csv.DictReader(stream, delimiter="\t"):
            rows.append((row["domain"], normalize(row["domain"], row["name"]), int(row["value"])))
    return rows


def read_expected(path: Path) -> dict[str, tuple[int, int, str]]:
    expected = {}
    with path.open(encoding="utf-8", newline="") as stream:
        for row in csv.DictReader(stream, delimiter="\t"):
            expected[row["name"]] = (
                int(row["linuxulator_value"]),
                int(row["bionic_value"]),
                row["reason"],
            )
    return expected


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--linuxulator", type=Path, required=True)
    parser.add_argument("--bionic", type=Path, required=True)
    parser.add_argument("--expected", type=Path, required=True)
    parser.add_argument("--expected-comparisons", type=int)
    arguments = parser.parse_args()

    bionic = read_bionic(arguments.bionic)
    expected = read_expected(arguments.expected)
    observed_expected = set()
    compared = 0
    failures = []
    for domain, name, linuxulator_value in read_linuxulator(arguments.linuxulator):
        if name not in bionic:
            continue
        compared += 1
        pair = (linuxulator_value, bionic[name])
        if pair[0] == pair[1]:
            if name in expected:
                failures.append(f"stale expected difference now agrees: {name}={pair[0]}")
            continue
        if name not in expected:
            failures.append(
                f"unexpected {domain} difference: {name} Linuxulator={pair[0]} Bionic={pair[1]}"
            )
            continue
        expected_pair = expected[name][:2]
        if pair != expected_pair:
            failures.append(
                f"changed expected difference: {name} got={pair} expected={expected_pair}"
            )
            continue
        observed_expected.add(name)

    missing = sorted(set(expected) - observed_expected)
    if missing:
        failures.append("expected differences were not observed: " + ", ".join(missing))
    if arguments.expected_comparisons is not None and compared != arguments.expected_comparisons:
        failures.append(
            f"comparison coverage drift: got={compared} "
            f"expected={arguments.expected_comparisons}"
        )
    if failures:
        raise SystemExit("\n".join(failures))
    print(
        f"freebsd-linuxulator-bionic-diff: PASS compared={compared} "
        f"documented_differences={len(observed_expected)}"
    )


if __name__ == "__main__":
    main()
