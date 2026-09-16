"""Materialize the selected boot JARs at their Android filesystem locations.

ART, libcore and NIO must observe the same bytes. This is image construction,
not a Java API-specific host path redirect. The caller owns a private staging
directory and publishes it only after this function succeeds.
"""
import argparse
import os
from pathlib import Path, PurePosixPath
import shutil

from package_runtime import digest
from resolve import resolve


def stage_system_root(source, destination):
    source = source.resolve()
    if destination.is_symlink():
        raise ValueError("staging root is a symlink")
    destination = destination.resolve()
    if source == destination or source.is_relative_to(destination):
        raise ValueError("staging root overlaps source")
    backings = resolve(source)
    locations = resolve(source, locations=True)
    plan = []
    for backing, location in zip(backings, locations, strict=True):
        logical = PurePosixPath(location)
        if (len(logical.parts) < 3 or str(logical) != location
                or logical.parts[1] not in ("apex", "system")
                or logical.suffix != ".jar"):
            raise ValueError(f"invalid system JAR location: {location}")
        relative = Path(*logical.parts[1:])
        target = destination / relative
        if (not target.resolve().is_relative_to(destination)
                or target.exists() or target.is_symlink()):
            raise ValueError(f"conflicting system JAR location: {location}")
        original = Path(backing)
        plan.append((original, target, relative, digest(original)))
    # Validate every input and target before writing any selected JAR.
    for original, target, relative, expected in plan:
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(original, target)
        if digest(target) != expected:
            raise ValueError(f"system JAR changed during staging: {relative}")
        target.chmod(0o444)
        os.utime(target, (1199145600, 1199145600))
    return [str(relative) for _, _, relative, _ in plan]


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("destination", type=Path)
    args = parser.parse_args()
    print("\n".join(stage_system_root(Path(__file__).resolve().parents[2], args.destination)))
