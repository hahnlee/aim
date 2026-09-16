"""Integration test against a supplied original APEX; never activates it."""
import ast
import pathlib
import re
import stat
import subprocess
import sys
import tempfile

tool, apex = map(lambda p: str(pathlib.Path(p).resolve()), sys.argv[1:])


def run(*args):
    return subprocess.run([tool, apex, *map(str, args)], check=True,
                          capture_output=True, text=True).stdout


with tempfile.TemporaryDirectory(prefix="apex-tree-test.") as temporary:
    root = pathlib.Path(temporary) / "payload"
    inventory = run("-", "/", "--inventory").splitlines()
    run(root, "/", "--tree")
    expected = set()
    for index, line in enumerate(inventory):
        match = re.fullmatch(r'mode=([0-7]+) bytes=(\d+) path=(".*") link=(.*)', line)
        assert match, line
        mode, size = int(match[1], 8), int(match[2])
        name = ast.literal_eval(match[3]).lstrip("/")
        expected.add(name)
        path = root / name
        info = path.lstat()  # Never follow an Android absolute symlink.
        assert stat.S_IFMT(info.st_mode) == stat.S_IFMT(mode), name
        if stat.S_ISLNK(mode):
            assert str(path.readlink()) == ast.literal_eval(match[4][5:-1]), name
        else:
            assert stat.S_IMODE(info.st_mode) == mode & 0o777, name
            if stat.S_ISREG(mode):
                reference = pathlib.Path(temporary) / f"reference-{index}"
                run(reference, "/" + name)
                assert info.st_size == size, name
                assert path.read_bytes() == reference.read_bytes(), name
                reference.unlink()
    actual = {""} | {str(p.relative_to(root)) for p in root.rglob("*")}
    assert actual == expected, (actual - expected, expected - actual)
    # Refuse an existing destination, including a symlink to a directory.
    alias = pathlib.Path(temporary) / "alias"
    alias.symlink_to(root, target_is_directory=True)
    for destination in (root, alias):
        result = subprocess.run([tool, apex, str(destination), "/", "--tree"],
                                capture_output=True)
        assert result.returncode != 0, destination
    print(f"PASS: {len(expected)} payload entries, modes, link targets, bytes; no overwrite")
