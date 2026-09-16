"""Extract original system/APEX boot JARs, never expand a logical partition."""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import tempfile
import zipfile

from metadata import boot_jars, fields


def digest(path):
    with path.open("rb") as source:
        return hashlib.file_digest(source, "sha256").hexdigest()


def run(*args):
    return subprocess.run([str(a) for a in args], check=True, capture_output=True, text=True).stdout


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("image", type=Path)
    parser.add_argument("output", type=Path)
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[2]
    image = args.image.resolve()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    super_tool = root / "target/release/super-i18n-apex-extract"
    apex_tool = root / "target/release/apex-ext2-extract"
    records = []
    seen = set()

    def extract_jars(paths, extractor, owner):
        for device_path in paths:
            # The platform fragment may reference an APEX (i18n). Its own
            # fragment or a subsequent explicit owner extraction supplies it.
            prefix = "/system/" if owner == "system" else f"/apex/{owner}/"
            if not device_path.startswith(prefix):
                continue
            if device_path in seen:
                raise ValueError(f"duplicate bootclasspath owner: {device_path}")
            seen.add(device_path)
            destination = output / device_path.lstrip("/")
            destination.parent.mkdir(parents=True, exist_ok=True)
            internal = device_path if owner == "system" else device_path[len(f"/apex/{owner}"):]
            with tempfile.TemporaryDirectory(prefix="boot-jar-", dir=output) as temporary:
                staged = Path(temporary) / "artifact.jar"
                extractor(staged, internal)
                sha = digest(staged)
                if destination.exists() and digest(destination) != sha:
                    raise ValueError(f"existing JAR differs: {destination}")
                if not destination.exists():
                    staged.replace(destination)
            records.append(dict(device_path=device_path, sha256=sha,
                                relative_path=str(destination.relative_to(output)), owner=owner))
            print(f"boot JAR: {device_path}", flush=True)

    with tempfile.TemporaryDirectory(prefix="boot-module-", dir=output) as temporary:
        stage = Path(temporary)
        platform = stage / "platform.pb"
        run(super_tool, image, platform, "--path", "/system/etc/classpaths/bootclasspath.pb")
        platform_paths = boot_jars(platform.read_bytes())
        names = run(super_tool, image, "-", "--path", "/system/apex").splitlines()
        for name in sorted(names):
            if not name.endswith((".apex", ".capex")):
                continue
            with tempfile.TemporaryDirectory(prefix="apex-", dir=stage) as module_temp:
                module = Path(module_temp)
                package = module / name
                run(super_tool, image, package, name)
                apex = package
                if name.endswith(".capex"):
                    with zipfile.ZipFile(package) as archive:
                        info = archive.getinfo("original_apex")
                        if info.file_size > 256 * 1024 * 1024:
                            raise ValueError("APEX exceeds extraction bound")
                        apex = module / "original.apex"
                        with archive.open(info) as source, apex.open("xb") as target:
                            import shutil
                            shutil.copyfileobj(source, target, 1024 * 1024)
                metadata = module / "bootclasspath.pb"
                with zipfile.ZipFile(apex) as archive:
                    manifest_data = archive.read("apex_manifest.pb")
                module_name = next(value.decode() for key, wire, value in fields(manifest_data)
                                   if key == 1 and wire == 2)
                try:
                    run(apex_tool, apex, metadata, "/etc/classpaths/bootclasspath.pb")
                except subprocess.CalledProcessError as error:
                    if "ext4 directory entry" not in error.stderr or "was not found" not in error.stderr:
                        raise
                paths = boot_jars(metadata.read_bytes()) if metadata.exists() else []
                for path in platform_paths:
                    if path.startswith(f"/apex/{module_name}/") and path not in paths:
                        paths.append(path)
                owners = {path.split("/")[2] for path in paths if path.startswith("/apex/")}
                if len(owners) > 1:
                    raise ValueError("mixed module owners")
                for owner in owners:
                    if owner != module_name:
                        raise ValueError("APEX manifest and classpath owner disagree")
                    extract_jars(paths, lambda target, path: run(apex_tool, apex, target, path), owner)
        extract_jars(platform_paths, lambda target, path: run(super_tool, image, target, "--path", path), "system")
        if set(platform_paths) - seen:
            raise ValueError("platform bootclasspath has unresolved entries")
    # Android 16 derive_classpath: ART fragment, platform fragment, then
    # other APEX fragments in glob order; retain each fragment's entry order.
    by_path = {record["device_path"]: record for record in records}
    art_paths = [r["device_path"] for r in records if r["owner"] == "com.android.art"]
    remaining = [r for r in records if r["device_path"] not in art_paths + platform_paths]
    remaining.sort(key=lambda record: record["owner"])
    ordered = art_paths + platform_paths + [r["device_path"] for r in remaining]
    if len(ordered) != len(set(ordered)) or set(ordered) != set(by_path):
        raise ValueError("invalid merged bootclasspath")
    manifest = dict(version=1, sdk=36, image_sha256=digest(image), fragments=records,
                    bootclasspath=ordered)
    destination = output / "manifest.json"
    staged = output / "manifest.json.pending"
    staged.write_text(json.dumps(manifest, indent=2) + "\n")
    staged.replace(destination)
    print(f"manifest: {destination} ({len(records)} JARs)")


if __name__ == "__main__":
    try:
        main()
    except subprocess.CalledProcessError as error:
        raise SystemExit(error.stderr)
