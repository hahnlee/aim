#!/usr/bin/env python3
"""Evaluate the parts of ART's Android.bp files needed to build libart.

This is not Soong. It reads Blueprint files, resolves `defaults` chains and
flattens the property groups that apply to one configuration: an Android
device, arm64, with the arm64 code generator enabled. `select()` expressions
take their `default` branch (ART's only selects are simulator switches).

Usage:
  bp_query.py ROOT MODULE PROPERTY [--bp FILE ...]

ROOT is the directory that contains `art/` (paths in the output are relative
to it). PROPERTY is one of the list properties (srcs, cflags, ...). The output
is one value per line; for path properties the values are prefixed with the
module's directory.
"""

import os
import re
import sys

TOKEN = re.compile(
    r'\s+|//[^\n]*|/\*.*?\*/|"(?:[^"\\]|\\.)*"|[A-Za-z_][A-Za-z0-9_.]*|-?\d+|[{}\[\]():,=+@]',
    re.S,
)

# Property groups that apply to an Android arm64 device build.
APPLICABLE_GROUPS = {
    ("arch", "arm64"),
    ("target", "android"),
    ("target", "android_arm64"),
    ("target", "linux"),
    ("target", "bionic"),
    ("target", "not_windows"),
    ("target", "android64"),
    ("codegen", "arm64"),
    ("codegen", "arm"),  # art/build/codegen.go: arm64 codegen implies arm.
    ("multilib", "lib64"),
    ("shared", None),  # We build shared libraries.
}

PATH_PROPERTIES = {"srcs", "export_include_dirs", "local_include_dirs", "exclude_srcs"}


class Parser:
    def __init__(self, text, path):
        self.tokens = [t for t in TOKEN.findall(text)
                       if t.strip() and not t.startswith("//") and not t.startswith("/*")]
        self.pos = 0
        self.path = path
        self.variables = {}

    def peek(self):
        return self.tokens[self.pos] if self.pos < len(self.tokens) else None

    def take(self, expected=None):
        token = self.peek()
        if expected is not None and token != expected:
            raise SyntaxError(f"{self.path}: expected {expected!r}, got {token!r} at {self.pos}")
        self.pos += 1
        return token

    def value(self):
        left = self.atom()
        while self.peek() == "+":
            self.take("+")
            right = self.atom()
            if isinstance(left, list) and isinstance(right, list):
                left = left + right
            elif isinstance(left, str) and isinstance(right, str):
                left = left + right
            elif isinstance(left, dict) and isinstance(right, dict):
                left = merge(dict(left), right)
            else:
                raise SyntaxError(f"{self.path}: cannot add {left!r} and {right!r}")
        return left

    def atom(self):
        token = self.peek()
        if token.startswith('"'):
            self.take()
            return bytes(token[1:-1], "utf-8").decode("unicode_escape")
        if token == "[":
            self.take("[")
            items = []
            while self.peek() != "]":
                items.append(self.value())
                if self.peek() == ",":
                    self.take(",")
            self.take("]")
            return items
        if token == "{":
            return self.map()
        if token == "select":
            return self.select()
        if token in ("true", "false"):
            self.take()
            return token == "true"
        if re.fullmatch(r"-?\d+", token):
            self.take()
            return int(token)
        self.take()
        # Unknown names only occur as bound values in non-default select
        # cases (`any @ name: [name]`), which are never chosen.
        return self.variables.get(token, [])

    def select(self):
        # select(condition, { case: value, ..., default: value })
        self.take("select")
        self.take("(")
        depth = 0
        while not (self.peek() == "," and depth == 0):
            token = self.take()
            depth += token == "("
            depth -= token == ")"
        self.take(",")
        self.take("{")
        chosen = None
        while self.peek() != "}":
            key = self.take()
            depth = 1 if key == "(" else 0
            while depth or self.peek() != ":":  # tuple or `any @ name` case
                token = self.take()
                depth += token == "("
                depth -= token == ")"
            self.take(":")
            value = self.value()
            if key == "default":
                chosen = value
            if self.peek() == ",":
                self.take(",")
        self.take("}")
        self.take(")")
        return chosen if chosen is not None else []

    def map(self):
        self.take("{")
        result = {}
        while self.peek() != "}":
            key = self.take()
            if key.startswith('"'):
                key = key[1:-1]
            self.take(":")
            result[key] = self.value()
            if self.peek() == ",":
                self.take(",")
        self.take("}")
        return result

    def file(self):
        modules = []
        while self.peek() is not None:
            name = self.take()
            if self.peek() == "=":
                self.take("=")
                self.variables[name] = self.value()
            elif self.peek() == "+":  # VAR += value
                self.take("+")
                self.take("=")
                self.variables[name] = self.variables[name] + self.value()
            else:
                body = self.map()
                modules.append((name, body))
        return modules


def merge(base, extra):
    for key, value in extra.items():
        if key in base and isinstance(base[key], list) and isinstance(value, list):
            base[key] = base[key] + value
        elif key in base and isinstance(base[key], dict) and isinstance(value, dict):
            base[key] = merge(dict(base[key]), value)
        else:
            base[key] = value
    return base


def load(root, bp_files):
    modules = {}
    for bp in bp_files:
        path = os.path.join(root, bp)
        with open(path) as handle:
            parsed = Parser(handle.read(), path).file()
        for kind, body in parsed:
            name = body.get("name")
            if name is not None:
                modules[name] = (kind, body, os.path.dirname(bp))
    return modules


def flatten(body):
    """Collapses the applicable arch/target/codegen groups into one map."""
    flat = {}
    for key, value in body.items():
        if key in ("arch", "target", "codegen", "multilib"):
            for group, props in value.items():
                if (key, group) in APPLICABLE_GROUPS and isinstance(props, dict):
                    merge(flat, flatten(props))
        elif key == "shared" and isinstance(value, dict):
            merge(flat, flatten(value))
        elif key in ("static", "host", "lto", "sanitize"):
            continue
        else:
            merge(flat, {key: value})
    return flat


def resolve(modules, name, seen=None):
    seen = seen or set()
    if name in seen:
        return {}
    seen.add(name)
    kind, body, directory = modules[name]
    flat = flatten(body)
    result = {}
    for default in flat.get("defaults", []):
        if default in modules:
            merge(result, resolve(modules, default, seen))
        else:
            print(f"warning: unknown defaults {default}", file=sys.stderr)
    own = {}
    for key, value in flat.items():
        if key in PATH_PROPERTIES and isinstance(value, list):
            value = [v if v.startswith(":") else os.path.normpath(os.path.join(directory, v))
                     for v in value]
        own[key] = value
    merge(result, own)
    return result


def main():
    args = sys.argv[1:]
    root, module, prop = args[0], args[1], args[2]
    bp_files = args[args.index("--bp") + 1:] if "--bp" in args else []
    modules = load(root, bp_files)
    values = resolve(modules, module).get(prop, [])
    if isinstance(values, list):
        excluded = set(resolve(modules, module).get("exclude_srcs", [])) if prop == "srcs" else set()
        seen = set()
        for value in values:
            if value in seen or value in excluded:
                continue
            seen.add(value)
            print(value)
    else:
        print(values)


if __name__ == "__main__":
    main()
