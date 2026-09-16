"""Test-only equivalent of AOSP rundiff/prepare_root, portable to macOS Bash3.

Copies upstream fixtures into one owned temporary directory; never stages
synthetic APEXes into a user profile or changes the upstream golden files.
"""
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import xml.etree.ElementTree as ET

project = Path(__file__).resolve().parents[2]
source = project / "_aosp/android16-linkerconfig/testdata"
converter = project / "_build/linkerconfig-golden/fixture-converter"
generator = project / "_build/linkerconfig-host/linkerconfig"
bootstrap = {"com.android.art", "com.android.runtime", "com.android.i18n",
             "com.android.tzdata", "com.android.vndk.vR"}

with tempfile.TemporaryDirectory(prefix="linkerconfig-golden.") as temporary:
    stage = Path(temporary)
    for name, early, blocked, flags in [
        ("stage1", True, set(), []),
        ("stage2", False, set(), []),
        ("vendor_with_vndk", False, set(), ["-v", "R"]),
        ("gen-only-a-single-apex", False, set(), ["-v", "R", "--apex", "com.vendor.service2"]),
        ("guest", False, {"com.android.art", "com.android.vndk.vR"}, ["-v", "R", "-p", "R"]),
    ]:
        root = stage / (name + "-root")
        shutil.copytree(source / "root", root)
        for pattern, mode in [("linker.config.json", "linker"), ("apex_manifest.json", "apex")]:
            for json in sorted(root.rglob(pattern)):
                subprocess.run([converter, mode, json, json.with_suffix(".pb")], check=True)
                json.unlink()
        (root / "apex").mkdir()
        inventory = ET.Element("apex-info-list")
        block_index = 1
        for partition in ["system", "product", "system_ext", "vendor", "odm"]:
            for module in sorted((root / partition / "apex").glob("*/")):
                if not module.is_dir():
                    continue
                module_path = "/" + str(module.relative_to(root)) + "/"
                if module.name in blocked:
                    module_path = f"/dev/block/vda{block_index}"
                    block_index += 1
                if early and module.name not in bootstrap:
                    continue
                shutil.copytree(module, root / "apex" / module.name)
                ET.SubElement(inventory, "apex-info", moduleName=module.name,
                              modulePath=module_path, partition=partition.upper(),
                              isFactory="true", isActive="true")
        ET.ElementTree(inventory).write(root / "apex/apex-info-list.xml", encoding="utf-8", xml_declaration=True)
        output = stage / name
        output.mkdir()
        subprocess.run([generator, "-z", "-r", root, "-t", output, *flags], check=True)
        expected = source / "golden_output" / name
        expected_files = {p.relative_to(expected) for p in expected.rglob("*") if p.is_file()}
        actual_files = {p.relative_to(output) for p in output.rglob("*") if p.is_file()}
        if expected_files != actual_files:
            raise RuntimeError(f"{name}: output file set differs: {expected_files ^ actual_files}")
        result = subprocess.run(["diff", "-ruN", expected, output])
        if result.returncode:
            sys.exit(result.returncode)
        print(f"AOSP linkerconfig golden {name}: PASS", flush=True)
