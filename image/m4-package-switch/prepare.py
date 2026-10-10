#!/usr/bin/env python3
"""Prepare/finalize an experimental C artifact in an isolated Git worktree."""
import argparse
import hashlib
import pathlib
import re
import subprocess


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def rows(path):
    result = []
    for line in path.read_text().splitlines():
        text = line.strip()
        if text and not text.startswith("#"):
            fields = text.split(None, 4)
            if len(fields) != 5:
                raise ValueError(f"invalid redirect row: {text}")
            result.append(fields)
    return result


def service_pairs(path):
    pairs = []
    for line in path.read_text().splitlines():
        text = line.strip()
        if not text or text.startswith("#"):
            continue
        fields = text.split()
        if len(fields) != 2 or any(name == fields[0] for name, _ in pairs):
            raise ValueError(f"invalid or duplicate native service: {text}")
        pairs.append(tuple(fields))
    return pairs


def stamp(path):
    lines = path.read_text().splitlines()
    if not lines or lines[0] != "aim-stamp 1":
        raise ValueError(f"invalid build stamp: {path}")
    result = {"deps": {}, "inputs": {}}
    for line in lines[1:]:
        tag, text = line.split(" ", 1)
        if tag == "key":
            result["key"] = text
        elif tag == "dep":
            name, key = text.split(" ", 1)
            result["deps"][name] = key
        elif tag == "input":
            checksum, size, mtime, name = text.split(" ", 3)
            result["inputs"][name] = checksum
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("mode", choices=["prepare", "finalize"])
    parser.add_argument("--isolated-repo", type=pathlib.Path, required=True)
    parser.add_argument("--out", type=pathlib.Path, required=True)
    parser.add_argument("--original", type=pathlib.Path)
    parser.add_argument("--redirect-cli", type=pathlib.Path)
    args = parser.parse_args()
    repo = args.isolated_repo.resolve(strict=True)
    source_repo = pathlib.Path(__file__).resolve().parents[2]
    if repo == source_repo or not (repo / ".git").exists():
        parser.error("--isolated-repo must be a separate Git worktree")
    out = args.out.resolve()
    relative_out = out.relative_to(repo).as_posix()
    if not relative_out.startswith("target/"):
        parser.error("--out must be a fresh target/ directory inside the isolated worktree")
    if args.mode == "prepare":
        if args.original is None or args.redirect_cli is None:
            parser.error("prepare requires --original and --redirect-cli")
        original = args.original.resolve(strict=True)
        if original != args.original.absolute() or args.original.is_symlink():
            parser.error("--original must name its real canonical tree, not a symlink")
        if out.is_relative_to(original) or original.is_relative_to(out):
            parser.error("experimental output overlaps original inputs")
        out.mkdir(parents=True, exist_ok=False)
        combined = {}
        for path in [repo / "image/system-server-redirects",
                     repo / "image/m4-package-switch/system-server-redirects.inactive",
                     repo / "crates/aim-services/tests/fixtures/native-user-manager-redirects"]:
            for entry in rows(path):
                combined[tuple(entry[:2])] = entry
        um = [entry for entry in combined.values() if entry[0].startswith("com.android.server.pm.UserManagerService")]
        if sum(int(entry[3]) for entry in um) != 14:
            raise ValueError("pinned original UM inventory must account for 14 calls")
        redirects = out / "system-server-redirects"
        redirects.write_text("# Experimental C only; default build inputs remain unchanged.\n" +
                             "\n".join(" ".join(entry) for entry in combined.values()) + "\n")
        services = (repo / "image/native-services").read_text()
        if any(line.split()[0] == "package" for line in services.splitlines() if line.strip() and not line.startswith("#")):
            raise ValueError("isolated baseline native-services already activates package")
        (out / "native-services").write_text(services + "\npackage com.android.server.pm.PackageManagerService\n")
        command = [str(args.redirect_cli.resolve(strict=True)), "--input", str(original / "system/framework/services.jar"),
                   "--redirects", str(redirects), "--targets", str(repo / "target/aim/device-services/aim-services.jar"),
                   "--native-services", str(repo / "image/native-services"), "--out", str(out / "services.jar")]
        subprocess.run(command, check=True)
        (out / "services.sha256").write_text(digest(out / "services.jar") + "\n")
        print(f"Prepared {out}; rebuild system-server/oat using this redirect table in the isolated worktree before finalize.")
    else:
        services = repo / "target/aim/system-server/services.jar"
        if digest(services) != digest(out / "services.jar"):
            raise ValueError("oat's system-server input differs from prepared experimental C jar")
        if (repo / "image/system-server-redirects").read_bytes() != (out / "system-server-redirects").read_bytes():
            raise ValueError("isolated build did not use the experimental C redirect table")
        system_stamp = stamp(repo / "target/aim-cache/system-server.stamp")
        oat_stamp = stamp(repo / "target/aim-cache/oat.stamp")
        device_stamp = stamp(repo / "target/aim-cache/device-services.stamp")
        if system_stamp["inputs"].get("image/system-server-redirects") != digest(out / "system-server-redirects"):
            raise ValueError("system-server stamp does not bind this experimental table")
        for name, built in [("system-server", system_stamp), ("device-services", device_stamp)]:
            if oat_stamp["deps"].get(name) != built["key"]:
                raise ValueError(f"experimental oat/vdex output is not built against current {name}")
        if not (repo / "target/aim/oat/overlay.toml").is_file():
            raise ValueError("experimental oat/vdex manifest absent")
        experimental_list = repo / "image/m4-package-switch/native-services.experimental"
        if service_pairs(out / "native-services") != service_pairs(experimental_list):
            raise ValueError("prepared native registration pairs differ from checked-in experimental list")
        manifest = (repo / "image/overlay.toml").read_text()
        blocks = re.split(r"(?=\[\[(?:add|replace|remove|include)\]\])", manifest)
        found = set()
        for index, block in enumerate(blocks):
            if 'path = "/system/framework/services.jar"' in block:
                block = re.sub(r'^source = .*$', f'source = "{relative_out}/services.jar"', block, flags=re.M)
                block = re.sub(r'^reason = .*$', 'reason = "Experimental M4 C only: native PMS entry and original UM concrete calls redirected; original image and default checkout preserved (ADR 0013)"', block, flags=re.M)
                found.add("services")
            if 'path = "/system/etc/aim/native-services"' in block:
                block = re.sub(r'^source = .*$', f'source = "{relative_out}/native-services"', block, flags=re.M)
                found.add("native")
            blocks[index] = block
        if found != {"services", "native"}:
            raise ValueError("baseline overlay does not expose both experimental replacement slots")
        (out / "overlay.toml").write_text("".join(blocks))
        build_blocks = []
        for block in blocks:
            if 'path = "/system/framework/services.jar"' in block:
                block = re.sub(r'^source = .*$', 'source = "target/aim/system-server/services.jar"', block, flags=re.M)
            if 'path = "/system/etc/aim/native-services"' in block:
                block = re.sub(r'^source = .*$', 'source = "image/m4-package-switch/native-services.experimental"', block, flags=re.M)
            build_blocks.append(block)
        (out / "build-overlay.toml").write_text("".join(build_blocks))
        print(f"Ready for the isolated derived-image build owner: {out / 'build-overlay.toml'}")


if __name__ == "__main__":
    main()
