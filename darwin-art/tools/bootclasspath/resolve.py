"""Resolve one ordered image inventory with explicit Darwin artifact replacements."""
import hashlib
import json
import argparse
from pathlib import Path, PurePosixPath


def resolve(root, *, locations=False):
    inventory = root / "_build/android16-bootclasspath-original"
    manifest = json.loads((inventory / "manifest.json").read_text())
    replacements = json.loads((root / "tools/bootclasspath/replacements.json").read_text())
    records = {record["device_path"]: record for record in manifest["fragments"]}
    if len(records) != len(manifest["fragments"]):
        raise ValueError("duplicate original component")
    order = manifest["bootclasspath"]
    if len(order) != len(set(order)) or set(order) != set(records):
        raise ValueError("incomplete ordered bootclasspath")
    if set(replacements) - set(records):
        raise ValueError("replacement has no original component")
    paths = []
    for device in order:
        logical = PurePosixPath(device)
        if (not logical.is_absolute() or ".." in logical.parts
                or any(character in device for character in (":", "\n", "\r", "\0"))):
            raise ValueError(f"invalid Android JAR location: {device!r}")
        record = records[device]
        original = (inventory / record["relative_path"]).resolve()
        if not original.is_relative_to(inventory.resolve()):
            raise ValueError("inventory path escapes root")
        with original.open("rb") as source:
            if hashlib.file_digest(source, "sha256").hexdigest() != record["sha256"]:
                raise ValueError(f"original JAR hash mismatch: {device}")
        path = (root / replacements[device]).resolve() if device in replacements else original
        if not path.is_relative_to(root.resolve()):
            raise ValueError("runtime JAR escapes root")
        if not path.is_file() or ":" in str(path) or "\n" in str(path):
            raise ValueError(f"invalid runtime JAR: {path}")
        # Replacements change backing bytes, not the Android/APEX identity
        # consumed by ART's DexCache and NativeLoader caller_location.
        paths.append(device if locations else str(path))
    return paths


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--locations", action="store_true")
    arguments = parser.parse_args()
    print(":".join(resolve(Path(__file__).resolve().parents[2], locations=arguments.locations)))
