#!/usr/bin/env python3
import ast
import re
import sys
from pathlib import Path


def fail(message: str) -> None:
    raise SystemExit(message)


if len(sys.argv) != 3:
    fail("usage: generate_messages.py BIONIC_ERRDEFS_H ANDROID_CPP_MACROS")

definitions: dict[str, str] = {}
for line in Path(sys.argv[2]).read_text().splitlines():
    match = re.fullmatch(r"#define ([A-Z][A-Z0-9_]*) (.+)", line)
    if match:
        definitions[match.group(1)] = match.group(2).strip()


def value(token: str, seen: set[str] | None = None) -> int:
    if token == "0":
        return 0
    seen = set() if seen is None else seen
    if token in seen:
        fail(f"recursive macro: {token}")
    seen.add(token)
    expression = definitions.get(token, token).strip()
    while expression.startswith("(") and expression.endswith(")"):
        expression = expression[1:-1].strip()
    expression = re.sub(r"([0-9a-fA-FxX]+)[uUlL]+$", r"\1", expression)
    if re.fullmatch(r"-?(?:0[xX][0-9a-fA-F]+|[0-9]+)", expression):
        return int(expression, 0)
    if re.fullmatch(r"[A-Z][A-Z0-9_]*", expression):
        return value(expression, seen)
    fail(f"unsupported macro expression for {token}: {expression}")


entries: list[tuple[int, str, str]] = []
pattern = re.compile(r'^__BIONIC_ERRDEF\(([^,]+),\s*("(?:[^"\\]|\\.)*")\)$')
for line in Path(sys.argv[1]).read_text().splitlines():
    match = pattern.fullmatch(line.strip())
    if not match:
        continue
    name = match.group(1).strip()
    message = ast.literal_eval(match.group(2))
    entries.append((value(name), name, message))

if not entries or entries[0] != (0, "0", "Success"):
    fail("Bionic errno table did not begin with Success")
entries.sort()
if len({number for number, _, _ in entries}) != len(entries):
    fail("Bionic errno table contains duplicate numeric entries")

print("/* Generated from pinned Bionic private/bionic_errdefs.h; do not edit. */")
for number, name, message in entries:
    escaped = message.replace("\\", "\\\\").replace('"', '\\"')
    print(f'  {{{number}, "{escaped}"}}, /* {name} */')
