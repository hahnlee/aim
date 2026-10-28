#!/usr/bin/env python3
"""Inspect or reclaim leftover link objects in one inactive local Cargo profile."""
import argparse
import fcntl
import json
import os
from pathlib import Path
import stat

ROOT = Path(__file__).resolve().parents[1]
DIRECTORY = os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW | os.O_CLOEXEC


def reclaim(profile, apply=False, root=ROOT):
    profile = Path(profile)
    target = root / "target"
    if not profile.is_absolute():
        profile = root / profile
    parts = profile.relative_to(target).parts
    if not parts or any(part in ("..", ".") for part in parts):
        raise ValueError("name one Cargo profile below this checkout's target directory")
    # Keep every lookup anchored to an open directory; never resolve symlinks.
    directory = os.open(target, DIRECTORY)
    try:
        for part in parts:
            child = os.open(part, DIRECTORY, dir_fd=directory)
            os.close(directory)
            directory = child
        lock = os.open(".cargo-lock", os.O_RDWR | os.O_NOFOLLOW | os.O_CLOEXEC,
                       dir_fd=directory)
        try:
            if not stat.S_ISREG(os.fstat(lock).st_mode):
                raise ValueError("Cargo profile lock is not a regular file")
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
            deps = os.open("deps", DIRECTORY, dir_fd=directory)
            try:
                count = size = 0
                for name in sorted(os.listdir(deps)):
                    if not name.endswith(".rcgu.o"):
                        continue
                    info = os.stat(name, dir_fd=deps, follow_symlinks=False)
                    if not stat.S_ISREG(info.st_mode):
                        continue
                    if apply:
                        os.unlink(name, dir_fd=deps)
                    count += 1
                    size += info.st_size
                return {"profile": str(profile), "applied": apply,
                        "objects": count, "logical_bytes": size}
            finally:
                os.close(deps)
        finally:
            os.close(lock)
    finally:
        os.close(directory)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("profile", type=Path, help="e.g. target/debug or target/host/release")
    parser.add_argument("--apply", action="store_true", help="delete objects; default only reports")
    args = parser.parse_args()
    try:
        print(json.dumps(reclaim(args.profile, args.apply), sort_keys=True))
    except (OSError, ValueError) as error:
        parser.exit(1, f"reclaim-cargo-objects: {error}\n")


if __name__ == "__main__":
    main()
