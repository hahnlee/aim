"""Verify a materialized font artifact against its extraction manifest.

This verifies local artifact consistency, not an independent upstream signature.
References absent from the original image remain absent; never invent fonts.
"""
import hashlib
import json
from pathlib import Path
import sys
import xml.etree.ElementTree as ET


def verify(root):
    for relative in ("", "system", "system/etc", "system/fonts"):
        directory = root / relative
        if directory.is_symlink() or not directory.is_dir():
            raise ValueError(f"font directory is not a real directory: {directory}")
    manifest_path = root / "manifest.json"
    if manifest_path.is_symlink() or not manifest_path.is_file():
        raise ValueError("font manifest is not a regular file")
    manifest = json.loads(manifest_path.read_text())
    if manifest.get("version") != 1:
        raise ValueError("unsupported font manifest")
    names = set()
    for record in manifest["files"]:
        relative = record["path"]
        parts = Path(relative).parts
        if relative in names or not (
            relative in ("system/etc/fonts.xml", "system/etc/font_fallback.xml")
            or (len(parts) == 3 and parts[:2] == ("system", "fonts")
                and parts[2] not in (".", ".."))
        ):
            raise ValueError(f"invalid or duplicate font artifact path: {relative}")
        names.add(relative)
        path = root / relative
        if path.is_symlink() or not path.is_file():
            raise ValueError(f"font artifact is not a regular file: {relative}")
        digest = hashlib.sha256()
        with path.open("rb") as source:
            for chunk in iter(lambda: source.read(65536), b""):
                digest.update(chunk)
        if digest.hexdigest() != record["sha256"]:
            raise ValueError(f"font artifact hash mismatch: {relative}")
    required = {"system/etc/fonts.xml", "system/etc/font_fallback.xml", "system/fonts/Roboto-Regular.ttf"}
    if not required <= names:
        raise ValueError("font artifact lacks required configuration/default font")
    actual = {"system/fonts/" + entry.name for entry in (root / "system/fonts").iterdir()}
    listed = {name for name in names if name.startswith("system/fonts/")}
    if actual != listed:
        raise ValueError("font directory differs from manifest inventory")
    references = set()
    for config in ("fonts.xml", "font_fallback.xml"):
        for font in ET.parse(root / "system/etc" / config).iter("font"):
            name = (font.text or "").strip()
            if not name or Path(name).name != name or name in (".", ".."):
                raise ValueError("invalid font reference")
            references.add(name)
    present = {Path(name).name for name in listed}
    absent = set(manifest["absent_in_image"])
    if references != present | absent or present & absent:
        raise ValueError("font references differ from original absence inventory")
    print(f"font artifact verified: {len(listed)} files; {len(absent)} original absences")


if __name__ == "__main__":
    verify(Path(sys.argv[1]))
