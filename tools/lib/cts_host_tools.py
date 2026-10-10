"""Verified Darwin dependency selection through pinned Tradefed metadata (#1166)."""
import hashlib
import json
import os
from pathlib import Path
import shutil
import zipfile

MODULE = 'CtsStagedInstallHostTestCases'
OFFICIAL_ZIP_SHA256 = '4f35a8a54d1d598b277b40307ca458b3152660365bd664abc435eb834fdcc53f'


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def stage(directory, root, harness):
    directory = directory.resolve(strict=True)
    provenance_path = directory / 'provenance.json'
    proof_path = directory / 'validation.json'
    provenance = json.loads(provenance_path.read_text())
    proof = json.loads(proof_path.read_text())
    if provenance['pins'] != json.loads((root / 'upstream/cts-host-tools.lock.json').read_text()):
        raise ValueError('CTS host-tool source pins differ')
    if provenance['recipe_sha256'] != digest(root / 'tools/cts-host-tools.py'):
        raise ValueError('CTS host-tool recipe differs from its build receipt')
    if proof.get('provenance_sha256') != digest(provenance_path):
        raise ValueError('CTS host-tool extraction proof does not bind this build')
    if (proof.get('ext4') != 'pass' or proof.get('corrupt_rejected') != 'pass'
            or proof.get('original_unchanged') is not True
            or proof.get('erofs_codecs') != ['lz4', 'lzma', 'zstd', 'deflate']):
        raise ValueError('CTS host-tool extraction proof is incomplete')
    if proof.get('validator_sha256') != digest(root / 'tools/tests/cts-host-tools.py'):
        raise ValueError('CTS host-tool validator differs')
    expected = dict(provenance['tools'])
    expected.update(provenance['python_runtime'])
    runtime = {str(p.relative_to(directory)) for p in (directory / 'python').rglob('*')
               if p.is_file() and '__pycache__' not in p.parts}
    if runtime != set(provenance['python_runtime']):
        raise ValueError('CTS host-tool Python runtime inventory differs')
    for name, sha in expected.items():
        path = directory / name
        if not path.resolve().is_relative_to(directory) or digest(path) != sha:
            raise ValueError('CTS host-tool component differs: ' + name)
    for name in ['deapexer', 'debugfs_static', 'fsck.erofs']:
        if not os.access(directory / name, os.X_OK):
            raise ValueError('CTS host tool is not executable: ' + name)
    official = harness / 'testcases' / MODULE
    if digest(official / 'deapexer.zip') != OFFICIAL_ZIP_SHA256:
        raise ValueError('official CTS deapexer ZIP differs from its pin')
    identity = digest(provenance_path)
    destination = root / 'target/aim/cts-host-tools-selection' / identity / MODULE
    destination.mkdir(parents=True, exist_ok=True)
    originals = {}
    for path in sorted(official.iterdir()):
        if path.name == 'deapexer.zip':
            continue
        if not path.is_file():
            raise ValueError('unexpected official module input: ' + str(path))
        target = destination / path.name
        if target.exists() and digest(target) != digest(path):
            raise ValueError('staged official module input differs: ' + path.name)
        if not target.exists():
            shutil.copy2(path, target)
        originals[path.name] = digest(path)
    bundle = destination / 'deapexer.zip'
    temporary = bundle.with_suffix('.partial')
    with zipfile.ZipFile(temporary, 'w', compression=zipfile.ZIP_DEFLATED) as archive:
        for name in sorted(expected):
            info = zipfile.ZipInfo(name, date_time=(2008, 1, 1, 0, 0, 0))
            info.external_attr = ((0o755 if name in provenance['tools'] else 0o644) << 16)
            archive.writestr(info, (directory / name).read_bytes(), compress_type=zipfile.ZIP_DEFLATED)
    if bundle.exists() and digest(bundle) != digest(temporary):
        temporary.unlink()
        raise ValueError('existing selected host-tool bundle differs')
    temporary.replace(bundle)
    option = MODULE + ':{config-descriptor}metadata:module-dir-path:=' + str(destination)
    receipt = {'directory': str(directory), 'provenance_sha256': identity,
               'validation_sha256': digest(proof_path), 'components': expected,
               'official_zip_sha256': OFFICIAL_ZIP_SHA256,
               'selected_zip_sha256': digest(bundle), 'official_inputs': originals,
               'module_arg': option}
    (destination / 'selection.json').write_text(json.dumps(receipt, indent=2) + '\n')
    return receipt


def arguments(receipt, module):
    return ['--module-arg', receipt['module_arg']] if receipt and module == MODULE else []


if __name__ == '__main__':
    import argparse
    parser = argparse.ArgumentParser()
    parser.add_argument('directory', type=Path)
    parser.add_argument('root', type=Path)
    parser.add_argument('harness', type=Path)
    args = parser.parse_args()
    print(json.dumps(stage(args.directory, args.root, args.harness), sort_keys=True))
