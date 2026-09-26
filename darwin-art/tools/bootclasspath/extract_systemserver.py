"""Extract the original system_server classpath JARs from the pinned image.

derive_classpath composes SYSTEMSERVERCLASSPATH from the system partition's
/system/etc/classpaths/systemserverclasspath.pb followed by each APEX's
/etc/classpaths/systemserverclasspath.pb in APEX-name order; standalone
system_server JARs are listed separately. This writes those exact JARs at
their device paths plus a manifest, never expanding a logical partition.
"""
import argparse
import json
from pathlib import Path
import shutil
import subprocess
import tempfile
import zipfile

from extract import digest, run
from metadata import STANDALONE_SYSTEMSERVER_JARS, SYSTEMSERVERCLASSPATH, boot_jars, fields


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

    def extract(device_path, extractor, owner):
        destination = output / device_path.lstrip("/")
        destination.parent.mkdir(parents=True, exist_ok=True)
        with tempfile.TemporaryDirectory(prefix="server-jar-", dir=output) as temporary:
            staged = Path(temporary) / "artifact.jar"
            extractor(staged)
            sha = digest(staged)
            if destination.exists() and digest(destination) != sha:
                raise ValueError(f"existing JAR differs: {destination}")
            if not destination.exists():
                staged.replace(destination)
        records.append(dict(device_path=device_path, sha256=sha,
                            relative_path=str(destination.relative_to(output)), owner=owner))
        print(f"system_server JAR: {device_path}", flush=True)

    classpath = []
    standalone = []
    with tempfile.TemporaryDirectory(prefix="server-module-", dir=output) as temporary:
        stage = Path(temporary)
        fragment = stage / "system.pb"
        run(super_tool, image, fragment, "--path", "/system/etc/classpaths/systemserverclasspath.pb")
        data = fragment.read_bytes()
        for kind, target in ((SYSTEMSERVERCLASSPATH, classpath),
                             (STANDALONE_SYSTEMSERVER_JARS, standalone)):
            for path in boot_jars(data, classpath=kind):
                if not path.startswith("/system/"):
                    raise ValueError(f"system fragment names a module JAR: {path}")
                extract(path, lambda staged, p=path: run(super_tool, image, staged, "--path", p),
                        "system")
                target.append(path)
        modules = []
        names = run(super_tool, image, "-", "--path", "/system/apex").splitlines()
        for name in sorted(names):
            if not name.endswith((".apex", ".capex")):
                continue
            module_dir = Path(tempfile.mkdtemp(prefix="apex-", dir=stage))
            package = module_dir / name
            run(super_tool, image, package, name)
            apex = package
            if name.endswith(".capex"):
                with zipfile.ZipFile(package) as archive:
                    info = archive.getinfo("original_apex")
                    if info.file_size > 256 * 1024 * 1024:
                        raise ValueError("APEX exceeds extraction bound")
                    apex = module_dir / "original.apex"
                    with archive.open(info) as source, apex.open("xb") as target:
                        shutil.copyfileobj(source, target, 1024 * 1024)
            with zipfile.ZipFile(apex) as archive:
                manifest_data = archive.read("apex_manifest.pb")
            module_name = next(value.decode() for key, wire, value in fields(manifest_data)
                               if key == 1 and wire == 2)
            modules.append((module_name, apex))
        # derive_classpath orders APEX fragments by module name.
        for module_name, apex in sorted(modules):
            metadata = apex.parent / "systemserverclasspath.pb"
            try:
                run(apex_tool, apex, metadata, "/etc/classpaths/systemserverclasspath.pb")
            except subprocess.CalledProcessError as error:
                if "ext4 directory entry" not in error.stderr or "was not found" not in error.stderr:
                    raise
                continue
            data = metadata.read_bytes()
            prefix = f"/apex/{module_name}/"
            for kind, target in ((SYSTEMSERVERCLASSPATH, classpath),
                                 (STANDALONE_SYSTEMSERVER_JARS, standalone)):
                for path in boot_jars(data, classpath=kind):
                    if not path.startswith(prefix):
                        raise ValueError(f"APEX {module_name} names a foreign JAR: {path}")
                    internal = path[len(prefix) - 1:]
                    extract(path, lambda staged, i=internal, a=apex: run(apex_tool, a, staged, i),
                            module_name)
                    target.append(path)
    if len(set(classpath + standalone)) != len(classpath) + len(standalone):
        raise ValueError("duplicate system_server JAR")
    manifest = dict(version=1, sdk=36, image_sha256=digest(image), fragments=records,
                    systemserverclasspath=classpath, standalone=standalone)
    staged = output / "manifest.json.pending"
    staged.write_text(json.dumps(manifest, indent=2) + "\n")
    staged.replace(output / "manifest.json")
    print(f"manifest: {output / 'manifest.json'} ({len(classpath)} classpath, "
          f"{len(standalone)} standalone)")


if __name__ == "__main__":
    main()
