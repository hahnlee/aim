#!/usr/bin/env python3
"""CLI wrapper for the importable javac_post contract frontend."""

from __future__ import annotations

import importlib.util
from pathlib import Path
import sys


_path = Path(__file__).with_name("art_javac_post_contract.py")
_spec = importlib.util.spec_from_file_location("art_javac_post_contract", _path)
assert _spec is not None and _spec.loader is not None
_module = importlib.util.module_from_spec(_spec)
sys.modules[_spec.name] = _module
_spec.loader.exec_module(_module)

if __name__ == "__main__":
    raise SystemExit(_module.main())
