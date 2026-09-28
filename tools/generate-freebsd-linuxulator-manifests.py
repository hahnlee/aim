#!/usr/bin/env python3
"""Generate deterministic ABI manifests from a pinned FreeBSD Linuxulator slice."""

from __future__ import annotations

import argparse
import ast
import re
from pathlib import Path


OBJECT_DEFINE = re.compile(r"^\s*#\s*define\s+(LINUX_[A-Z0-9_]+)\s+(.+?)\s*$")
FUNCTION_DEFINE = re.compile(r"^\s*#\s*define\s+LINUX_[A-Z0-9_]+\(")
INTEGER_SUFFIX = re.compile(r"(?i)\b(0x[0-9a-f]+|0[0-7]+|\d+)(?:u|l)+\b")
OCTAL_INTEGER = re.compile(r"(?<![A-Za-z0-9_])0([0-7]+)(?![A-Za-z0-9_])")


def logical_lines(path: Path):
    physical = path.read_text(encoding="utf-8").splitlines()
    index = 0
    while index < len(physical):
        start = index + 1
        value = physical[index]
        while value.rstrip().endswith("\\") and index + 1 < len(physical):
            value = value.rstrip()[:-1] + " " + physical[index + 1].strip()
            index += 1
        yield start, value
        index += 1


class IntegerEvaluator(ast.NodeVisitor):
    OPERATORS = {
        ast.Add: lambda left, right: left + right,
        ast.Sub: lambda left, right: left - right,
        ast.BitOr: lambda left, right: left | right,
        ast.BitAnd: lambda left, right: left & right,
        ast.BitXor: lambda left, right: left ^ right,
        ast.LShift: lambda left, right: left << right,
        ast.RShift: lambda left, right: left >> right,
        ast.Mult: lambda left, right: left * right,
    }
    UNARY = {
        ast.UAdd: lambda value: value,
        ast.USub: lambda value: -value,
        ast.Invert: lambda value: ~value,
    }

    def __init__(self, names: dict[str, int]):
        self.names = names

    def visit_Expression(self, node: ast.Expression) -> int:
        return self.visit(node.body)

    def visit_Constant(self, node: ast.Constant) -> int:
        if type(node.value) is not int:
            raise ValueError("not an integer")
        return node.value

    def visit_Name(self, node: ast.Name) -> int:
        if node.id not in self.names:
            raise ValueError(f"unresolved name: {node.id}")
        return self.names[node.id]

    def visit_BinOp(self, node: ast.BinOp) -> int:
        operation = self.OPERATORS.get(type(node.op))
        if operation is None:
            raise ValueError("unsupported binary operation")
        return operation(self.visit(node.left), self.visit(node.right))

    def visit_UnaryOp(self, node: ast.UnaryOp) -> int:
        operation = self.UNARY.get(type(node.op))
        if operation is None:
            raise ValueError("unsupported unary operation")
        return operation(self.visit(node.operand))

    def generic_visit(self, node):
        raise ValueError(f"unsupported expression node: {type(node).__name__}")


def clean_expression(expression: str) -> str:
    expression = re.sub(r"/\*.*?\*/", "", expression)
    expression = expression.split("//", 1)[0].strip()
    return expression


def evaluate(expression: str, names: dict[str, int]) -> int:
    expression = clean_expression(expression)
    expression = INTEGER_SUFFIX.sub(lambda match: match.group(1), expression)
    expression = OCTAL_INTEGER.sub(lambda match: "0o" + match.group(1), expression)
    tree = ast.parse(expression, mode="eval")
    return IntegerEvaluator(names).visit(tree)


def constant_domain(name: str, relative: str) -> str | None:
    if relative.endswith("linux_errno.h") and name.startswith("LINUX_E"):
        return "errno"
    if relative.endswith("compat/linux/linux.h") and name.startswith("LINUX_SIG"):
        return "signal"
    if (relative.endswith("linux_file.h") or relative.endswith("arm64/linux/linux.h")) and name.startswith("LINUX_O_"):
        return "open"
    if relative.endswith("linux_mmap.h") and name.startswith(("LINUX_MAP_", "LINUX_PROT_", "LINUX_MADV_")):
        return "mmap"
    socket_prefixes = (
        "LINUX_MSG_", "LINUX_SOCK_", "LINUX_SOL_", "LINUX_SO_",
        "LINUX_SCM_", "LINUX_AF_", "LINUX_PF_", "LINUX_IP_",
        "LINUX_IPV6_", "LINUX_TCP_", "LINUX_UDP_", "LINUX_SHUT_",
    )
    if relative.endswith("linux_socket.h") and name.startswith(socket_prefixes):
        return "socket"
    return None


def generate_constants(source_root: Path) -> str:
    inputs = [
        "sys/compat/linux/linux_errno.h",
        "sys/compat/linux/linux.h",
        "sys/compat/linux/linux_file.h",
        "sys/arm64/linux/linux.h",  # arm64 overrides common open values
        "sys/compat/linux/linux_mmap.h",
        "sys/compat/linux/linux_socket.h",
    ]
    values: dict[str, int] = {}
    rows: dict[str, tuple[str, str, int, str, int]] = {}
    pending: list[tuple[str, str, int, str, str]] = []
    for relative in inputs:
        for line_number, line in logical_lines(source_root / relative):
            if FUNCTION_DEFINE.match(line):
                continue
            match = OBJECT_DEFINE.match(line)
            if not match:
                continue
            name, expression = match.groups()
            domain = constant_domain(name, relative)
            if domain is not None:
                pending.append((domain, name, line_number, clean_expression(expression), relative))

    # Resolve aliases and simple arithmetic in repeated passes. Later arm64
    # definitions intentionally replace common definitions.
    unresolved = pending
    for _ in range(len(pending) + 1):
        next_unresolved = []
        progress = False
        for domain, name, line_number, expression, relative in unresolved:
            try:
                value = evaluate(expression, values)
            except (SyntaxError, ValueError):
                next_unresolved.append((domain, name, line_number, expression, relative))
                continue
            values[name] = value
            rows[name] = (domain, expression, value, relative, line_number)
            progress = True
        unresolved = next_unresolved
        if not progress:
            break

    # The source path lookup above must remain unambiguous for every emitted
    # object-like constant; unresolved host-dependent macros are omitted.
    output = ["domain\tname\texpression\tvalue\tsource\tline"]
    for name, (domain, expression, value, relative, line_number) in sorted(
        rows.items(), key=lambda item: (item[1][0], item[0])
    ):
        output.append(f"{domain}\t{name}\t{expression}\t{value}\t{relative}\t{line_number}")
    return "\n".join(output) + "\n"


def syscall_records(master: str):
    starts = list(re.finditer(r"(?m)^(\d+)\s+(\S+)\s+(\S+)\s+(.*)$", master))
    for index, match in enumerate(starts):
        end = starts[index + 1].start() if index + 1 < len(starts) else len(master)
        yield match, master[match.start():end]


def generate_syscalls(source_root: Path) -> tuple[str, set[str]]:
    master_path = source_root / "sys/arm64/linux/syscalls.master"
    master = master_path.read_text(encoding="utf-8")
    rows = []
    handlers = set()
    for match, record in syscall_records(master):
        number, audit, status, tail = match.groups()
        function = re.search(r"\bint\s+([A-Za-z0-9_]+)\s*\(", record)
        if function:
            name = function.group(1)
        else:
            name = tail.strip().split()[0].rstrip(";")
        handlers.add(name)
        line_number = master.count("\n", 0, match.start()) + 1
        rows.append((int(number), audit, status, name, line_number))

    generated = {}
    header = (source_root / "sys/arm64/linux/linux_syscall.h").read_text(encoding="utf-8")
    for name, number in re.findall(r"^#define\s+LINUX_SYS_([A-Za-z0-9_]+)\s+(\d+)\s*$", header, re.MULTILINE):
        generated[(name, int(number))] = True
    expected = {
        (name, number) for number, _, status, name, _ in rows
        if status != "UNIMPL"
    }
    missing = sorted(expected - set(generated))
    if missing:
        raise SystemExit(f"generated linux_syscall.h disagrees with syscalls.master: {missing[:5]}")
    max_syscall = max(number for number, _, _, _, _ in rows) + 1
    allowed_extra = {("MAXSYSCALL", max_syscall)}
    extra = sorted(set(generated) - expected - allowed_extra)
    if extra or not allowed_extra.issubset(generated):
        raise SystemExit(
            "generated linux_syscall.h has unexpected entries or MAXSYSCALL: "
            f"extra={extra[:5]} expected_max={max_syscall}"
        )

    output = ["number\taudit\tstatus\tname\tsource\tline"]
    for number, audit, status, name, line_number in rows:
        output.append(
            f"{number}\t{audit}\t{status}\t{name}\tsys/arm64/linux/syscalls.master\t{line_number}"
        )
    return "\n".join(output) + "\n", handlers


def generate_errno_translations(source_root: Path) -> str:
    relative = "sys/compat/linux/linux_errno.inc"
    lines = (source_root / relative).read_text(encoding="utf-8").splitlines()
    direction = None
    output = ["direction\tsource_name\ttarget_name\tsource\tline"]
    for line_number, line in enumerate(lines, 1):
        if "linux_errtbl[" in line:
            direction = "freebsd-to-linux"
        elif "linux_to_bsd_errtbl[" in line:
            direction = "linux-to-freebsd"
        match = re.search(r"\[([A-Z0-9_]+)\]\s*=\s*-?([A-Z0-9_]+)", line)
        if direction is None or not match:
            continue
        source_name, target_name = match.groups()
        output.append(f"{direction}\t{source_name}\t{target_name}\t{relative}\t{line_number}")
    return "\n".join(output) + "\n"


def generate_signal_translations(source_root: Path) -> str:
    relative = "sys/compat/linux/linux.c"
    lines = (source_root / relative).read_text(encoding="utf-8").splitlines()
    direction = None
    output = ["direction\tsource_name\ttarget_name\tsource\tline"]
    for line_number, line in enumerate(lines, 1):
        if "bsd_to_linux_sigtbl[" in line:
            direction = "freebsd-to-linux"
            continue
        if "linux_to_bsd_sigtbl[" in line:
            direction = "linux-to-freebsd"
            continue
        if direction and line.strip() == "};":
            direction = None
            continue
        if direction is None:
            continue
        match = re.match(r"\s*([A-Z][A-Z0-9_]*)\s*,?\s*/\*\s*([A-Z][A-Z0-9_]*)", line)
        if not match:
            continue
        target_name, source_name = match.groups()
        output.append(f"{direction}\t{source_name}\t{target_name}\t{relative}\t{line_number}")
    return "\n".join(output) + "\n"


def ownership_domain(relative: str, symbol: str) -> str:
    basename = Path(relative).name
    if basename == "linux.c":
        if "sig" in symbol:
            return "signal"
        if any(token in symbol for token in ("domain", "sockaddr", "socket")):
            return "socket"
        return "common"
    return {
        "linux_emul.c": "path-prefix",
        "linux_errno.c": "errno",
        "linux_event.c": "event",
        "linux_file.c": "open-file-path",
        "linux_futex.c": "futex",
        "linux_mmap.c": "mmap",
        "linux_signal.c": "signal",
        "linux_socket.c": "socket",
        "linux_util.c": "path-device",
        "linux_sysvec.c": "arm64-personality",
    }[basename]


def generate_ownership(source_root: Path, syscall_handlers: set[str]) -> str:
    inputs = [
        "sys/arm64/linux/linux_sysvec.c",
        "sys/compat/linux/linux.c",
        "sys/compat/linux/linux_emul.c",
        "sys/compat/linux/linux_errno.c",
        "sys/compat/linux/linux_event.c",
        "sys/compat/linux/linux_file.c",
        "sys/compat/linux/linux_futex.c",
        "sys/compat/linux/linux_mmap.c",
        "sys/compat/linux/linux_signal.c",
        "sys/compat/linux/linux_socket.c",
        "sys/compat/linux/linux_util.c",
    ]
    rows = []
    function = re.compile(r"^((?:linux|bsd_to_linux|linux_to_bsd)_[A-Za-z0-9_]+)\s*\(")
    for relative in inputs:
        for line_number, line in enumerate((source_root / relative).read_text(encoding="utf-8").splitlines(), 1):
            match = function.match(line)
            if not match:
                continue
            symbol = match.group(1)
            # Token-pasting helpers in macro definitions end with an
            # underscore and are not concrete ownership symbols.
            if symbol.endswith("_"):
                continue
            if symbol in syscall_handlers:
                role = "syscall-handler"
            elif "_to_" in symbol or symbol.endswith(("flags", "domain", "signal", "sigset")):
                role = "converter"
            else:
                role = "helper"
            rows.append((ownership_domain(relative, symbol), symbol, role, relative, line_number))
    output = ["domain\tsymbol\trole\treuse\tsource\tline"]
    for domain, symbol, role, relative, line_number in sorted(rows):
        output.append(
            f"{domain}\t{symbol}\t{role}\tsemantics-only-freebsd-kernel-coupled\t{relative}\t{line_number}"
        )
    return "\n".join(output) + "\n"


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--source-root", type=Path, required=True)
    parser.add_argument("--output-dir", type=Path, required=True)
    arguments = parser.parse_args()
    arguments.output_dir.mkdir(parents=True, exist_ok=True)

    syscalls, handlers = generate_syscalls(arguments.source_root)
    outputs = {
        "arm64-syscalls.tsv": syscalls,
        "constants.tsv": generate_constants(arguments.source_root),
        "errno-translations.tsv": generate_errno_translations(arguments.source_root),
        "signal-translations.tsv": generate_signal_translations(arguments.source_root),
        "translation-ownership.tsv": generate_ownership(arguments.source_root, handlers),
    }
    for name, content in outputs.items():
        (arguments.output_dir / name).write_text(content, encoding="utf-8")


if __name__ == "__main__":
    main()
