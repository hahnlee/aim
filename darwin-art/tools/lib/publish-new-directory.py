#!/usr/bin/env python3
"""Publish a completed directory with Darwin renamex_np(RENAME_EXCL)."""
import ctypes
import os
import sys


def main() -> int:
    if len(sys.argv) != 3:
        print(f"usage: {sys.argv[0]} STAGE DESTINATION", file=sys.stderr)
        return 64
    if sys.platform != "darwin":
        print("exclusive directory publication requires macOS", file=sys.stderr)
        return 69
    libc = ctypes.CDLL(None, use_errno=True)
    renamex_np = libc.renamex_np
    renamex_np.argtypes = [ctypes.c_char_p, ctypes.c_char_p, ctypes.c_uint]
    renamex_np.restype = ctypes.c_int
    result = renamex_np(os.fsencode(sys.argv[1]), os.fsencode(sys.argv[2]), 0x00000004)
    if result != 0:
        error = ctypes.get_errno()
        print(f"exclusive directory publication failed: {os.strerror(error)}", file=sys.stderr)
        return 73
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
