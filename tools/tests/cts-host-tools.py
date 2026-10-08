#!/usr/bin/env python3
"""Actual extraction checks for the Darwin CTS host-tool build (#1166)."""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import tempfile
import zipfile


def run(*args):
    subprocess.run(list(map(str, args)), check=True)


def fixture(tool, image, destination):
    apex = image.with_suffix('.apex')
    with zipfile.ZipFile(apex, 'w') as archive:
        archive.write(image, 'apex_payload.img')
    run(tool / 'deapexer', 'extract', apex, destination)
    assert (destination / 'nested/content').read_bytes() == b'original fixture bytes\x00\xff'
    assert (destination / 'nested/large').read_bytes() == bytes(range(256)) * 4096
    assert (destination / 'link').is_symlink()
    assert (destination / 'link').readlink() == Path('nested/content')
    assert (destination / 'nested/content').stat().st_mode & 0o777 == 0o640
    corrupt = image.with_name(image.stem + '-corrupt.img')
    corrupt.write_bytes(b'not a filesystem')
    with zipfile.ZipFile(corrupt.with_suffix('.apex'), 'w') as archive:
        archive.write(corrupt, 'apex_payload.img')
    failure = subprocess.run([str(tool / 'deapexer'), 'extract', str(corrupt.with_suffix('.apex')),
                              str(destination.with_name(destination.name + '-bad'))], capture_output=True)
    assert failure.returncode != 0


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--tools', required=True, type=Path)
    parser.add_argument('--apex', required=True, type=Path)
    args = parser.parse_args()
    tool = args.tools.resolve()
    original = args.apex.resolve()
    before = hashlib.sha256(original.read_bytes()).hexdigest()
    with tempfile.TemporaryDirectory(prefix='aim-host-extract-') as directory:
        out = Path(directory)
        source = out / 'input'
        (source / 'nested').mkdir(parents=True)
        content = source / 'nested/content'
        content.write_bytes(b'original fixture bytes\x00\xff')
        content.chmod(0o640)
        (source / 'nested/large').write_bytes(bytes(range(256)) * 4096)
        (source / 'link').symlink_to('nested/content')
        ext4 = out / 'fixture-ext4.img'
        run(tool / 'build/e2fsprogs/misc/mke2fs', '-q', '-t', 'ext4', '-F', '-d', source,
            ext4, '4096')
        fixture(tool, ext4, out / 'ext4')
        for codec in ['lz4', 'lzma', 'zstd', 'deflate']:
            erofs = out / ('fixture-erofs-' + codec + '.img')
            run(tool / 'prefix/bin/mkfs.erofs', '-z' + codec, erofs, source)
            fixture(tool, erofs, out / ('erofs-' + codec))
        run(tool / 'deapexer', 'extract', original, out / 'official')
        assert any(p.is_file() for p in (out / 'official').rglob('*'))
        assert before == hashlib.sha256(original.read_bytes()).hexdigest()
        result = {'ext4': 'pass', 'erofs_codecs': ['lz4', 'lzma', 'zstd', 'deflate'],
                  'corrupt_rejected': 'pass', 'original_apex_sha256': before, 'original_unchanged': True,
                  'validator_sha256': hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
                  'provenance_sha256': hashlib.sha256((tool / 'provenance.json').read_bytes()).hexdigest()}
        (tool / 'validation.json').write_text(json.dumps(result, indent=2) + '\n')
        print(json.dumps(result))


if __name__ == '__main__':
    main()
