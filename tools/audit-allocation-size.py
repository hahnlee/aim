#!/usr/bin/env python3
"""Execute the actual runtime TLAB size macro, with native high-window classes."""
import pathlib
import re
import subprocess

ROOT = pathlib.Path(__file__).resolve().parents[1]
BUILD = ROOT / "_build/allocation-size-audit"
ARM64 = ROOT / "_build/runtime-arm64/patched-source/runtime/arch/arm64"
GENERATED = ROOT / "_build/runtime-arm64/generated"


def macro(path, name):
    matches = re.findall(r"(?ms)^\.macro " + name + r"\b.*?^\.endm", path.read_text())
    if len(matches) != 1:
        raise RuntimeError(f"Expected exactly one {name} in {path}")
    return matches[0]


BUILD.mkdir(parents=True, exist_ok=True)
source = '#include "asm_defines.h"\n'
source += macro(ARM64 / "asm_support_arm64.S", "UNPOISON_HEAP_REF") + "\n"
source += macro(ARM64 / "quick_entrypoints_arm64.S", "COMPUTE_ARRAY_SIZE_UNKNOWN") + "\n"
source += """
.text
.p2align 2
.globl _darwin_test_array_size
_darwin_test_array_size:
    mov w1, w1
    COMPUTE_ARRAY_SIZE_UNKNOWN x0, w0, x1, w1, x4, w4, x5, w5, x6, w6
    and x0, x5, #OBJECT_ALIGNMENT_MASK_TOGGLED64
    ret
"""
for poisoning in (False, True):
    tag = "poisoned" if poisoning else "plain"
    flags = ["-DUSE_HEAP_POISONING"] if poisoning else []
    obj = BUILD / (tag + ".o")
    exe = BUILD / tag
    subprocess.run(["clang", "-arch", "arm64", "-x", "assembler-with-cpp",
                    "-I" + str(GENERATED), *flags, "-c", "-", "-o", str(obj)],
                   input=source, text=True, check=True)
    subprocess.run(["clang++", "-arch", "arm64", "-std=c++20", "-O2",
                    "-I" + str(GENERATED), *flags,
                    str(ROOT / "tools/allocation-size-smoke.cc"), str(obj),
                    "-o", str(exe)], check=True)
    subprocess.run([str(exe)], check=True)
