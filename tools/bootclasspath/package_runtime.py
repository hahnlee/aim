"""Package the ordered bootclasspath and its verification inputs, not a dev tree."""
import argparse
import hashlib
import json
from pathlib import Path
import shutil

from resolve import resolve


def digest(path):
    with path.open("rb") as source:
        return hashlib.file_digest(source, "sha256").hexdigest()


def package_runtime(source, destination):
    source = source.resolve()
    destination = destination.resolve()
    if source == destination or source.is_relative_to(destination):
        raise ValueError("package destination overlaps source root")
    selected = resolve(source)
    inventory = Path("_build/android16-bootclasspath-original")
    manifest = json.loads((source / inventory / "manifest.json").read_text())
    inputs = {
        inventory / "manifest.json",
        Path("tools/bootclasspath/resolve.py"),
        Path("tools/bootclasspath/replacements.json"),
    }
    for record in manifest["fragments"]:
        inputs.add(inventory / record["relative_path"])
    inputs.update(Path(path).relative_to(source) for path in selected)
    # Validate the entire copy plan before publishing any resource. Existing
    # Manager inputs are reusable only when they contain exactly these bytes.
    copies = []
    for relative in sorted(inputs):
        if relative.is_absolute() or ".." in relative.parts:
            raise ValueError("package input escapes root")
        original = source / relative
        target = destination / relative
        if not original.resolve().is_relative_to(source):
            raise ValueError("package input escapes root")
        if not target.resolve().is_relative_to(destination) or target.is_symlink():
            raise ValueError("package target escapes root or is a symlink")
        if target.exists():
            if not target.is_file() or digest(original) != digest(target):
                raise ValueError(f"conflicting package input: {relative}")
        else:
            copies.append((original, target))
    for original, target in copies:
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(original, target)
    # Resolve at the relocated path: a successful source check cannot establish
    # that the resulting signed bundle will have all its required inputs.
    result = resolve(destination)
    expected = [str(destination / Path(path).relative_to(source)) for path in selected]
    if result != expected:
        raise ValueError("packaged bootclasspath order or relocation differs")
    return result


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("destination", type=Path)
    args = parser.parse_args()
    paths = package_runtime(Path(__file__).resolve().parents[2], args.destination)
    print(f"packaged bootclasspath verified: {len(paths)} ordered components")
