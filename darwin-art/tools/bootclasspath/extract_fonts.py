"""Extract the original Android font configuration and its referenced files."""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import tempfile
import xml.etree.ElementTree as ET


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("image", type=Path)
    parser.add_argument("output", type=Path)
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[2]
    tool = root / "target/release/super-i18n-apex-extract"
    args.output.mkdir(parents=True, exist_ok=True)
    records = []

    def extract(relative):
        destination = args.output / relative
        destination.parent.mkdir(parents=True, exist_ok=True)
        with tempfile.TemporaryDirectory(prefix="font-extract-", dir=args.output) as temporary:
            staged = Path(temporary) / "file"
            subprocess.run([str(tool), str(args.image), str(staged), "--path", "/" + relative],
                           check=True, stdout=subprocess.DEVNULL)
            sha = hashlib.sha256(staged.read_bytes()).hexdigest()
            if destination.exists():
                if hashlib.sha256(destination.read_bytes()).hexdigest() != sha:
                    raise ValueError(f"original font artifact differs: {relative}")
            else:
                staged.replace(destination)
        records.append(dict(path=relative, sha256=sha))
        return destination

    fonts = set()
    for name in ("fonts.xml", "font_fallback.xml"):
        config = extract(f"system/etc/{name}")
        for font in ET.parse(config).iter("font"):
            name = (font.text or "").strip()
            if not name or Path(name).name != name or name in (".", ".."):
                raise ValueError("invalid font filename")
            fonts.add(name)
    available = set(subprocess.check_output(
        [str(tool), str(args.image), "-", "--path", "/system/fonts"], text=True).splitlines())
    absent = sorted(fonts - available)
    for name in sorted(fonts & available):
        extract(f"system/fonts/{name}")
        print(f"font: {name}", flush=True)
    staged = args.output / "manifest.json.pending"
    # Preserve references absent in the original image; do not invent fonts.
    staged.write_text(json.dumps(dict(version=1, files=records, absent_in_image=absent), indent=2) + "\n")
    staged.replace(args.output / "manifest.json")
    print(f"original font set: {len(fonts)} files")


if __name__ == "__main__":
    main()
