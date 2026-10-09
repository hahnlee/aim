"""Detached script generations for long CTS invocations (#1178)."""
import hashlib
import json
import os
from pathlib import Path
import shutil
import tempfile

SCRIPTS = ('tools/cts-tradefed.sh', 'tools/guest-shell.sh', 'tools/lib/cts_host_tools.py')
HARNESS_SCRIPTS = ('cts-tradefed', 'test-utils-script')


def sha(data):
    return hashlib.sha256(data).hexdigest()


def validate(bundle):
    bundle = Path(bundle).resolve(strict=True)
    manifest_path = bundle / 'manifest.json'
    if manifest_path.is_symlink() or manifest_path.stat().st_mode & 0o222:
        raise ValueError('CTS bundle manifest must be detached and readonly')
    manifest = json.loads(manifest_path.read_text())
    expected_scripts = set(SCRIPTS) | {'harness/android-cts/tools/' + name for name in HARNESS_SCRIPTS}
    if set(manifest['files']) != expected_scripts:
        raise ValueError('CTS bundle script inventory differs')
    for subtree, expected in [('tools', set(SCRIPTS)),
                              ('harness/android-cts/tools', expected_scripts - set(SCRIPTS))]:
        actual = {str(path.relative_to(bundle)) for path in (bundle / subtree).rglob('*')
                  if path.is_file() and not path.name.endswith('.jar')}
        if actual != expected:
            raise ValueError('CTS bundle on-disk script inventory differs')
    identity = sha(json.dumps(manifest, sort_keys=True, separators=(',', ':')).encode())
    if bundle.name != identity:
        raise ValueError('CTS script bundle identity differs')
    for name, record in manifest['files'].items():
        path = bundle / name
        if path.is_symlink() or not path.resolve().is_relative_to(bundle):
            raise ValueError('CTS bundle script must be detached: ' + name)
        if sha(path.read_bytes()) != record['sha256']:
            raise ValueError('CTS bundle script differs: ' + name)
        executable = name.endswith('.sh') or name.rsplit('/', 1)[-1] in HARNESS_SCRIPTS
        if record['executable'] != executable:
            raise ValueError('CTS bundle executable declaration differs: ' + name)
        expected_mode = 0o555 if executable else 0o444
        if path.stat().st_mode & 0o777 != expected_mode:
            raise ValueError('CTS bundle script mode differs: ' + name)
    tools = bundle / 'harness/android-cts/tools'
    if {path.name for path in tools.glob('*.jar')} != set(manifest['external']):
        raise ValueError('CTS bundle harness JAR inventory differs')
    for name, record in manifest['external'].items():
        path = Path(record['path'])
        link = tools / name
        if not link.is_symlink() or link.resolve(strict=True) != path:
            raise ValueError('CTS bundle harness JAR link differs: ' + name)
        if sha(path.read_bytes()) != record['sha256']:
            raise ValueError('CTS external harness artifact differs: ' + name)
    testcase_files = manifest.get('testcase_files')
    if testcase_files is not None:
        testcases = bundle / 'harness/android-cts/testcases'
        if testcases.is_symlink() or not testcases.is_dir():
            raise ValueError('CTS bundle testcase directory must be detached')
        if any(path.is_symlink() and path.is_dir() for path in testcases.rglob('*')):
            raise ValueError('CTS bundle testcase subdirectories must be detached')
        actual = {str(path.relative_to(testcases)) for path in testcases.rglob('*') if path.is_file()}
        if actual != set(testcase_files):
            raise ValueError('CTS bundle testcase artifact inventory differs')
        for name, record in testcase_files.items():
            path = Path(record['path'])
            link = testcases / name
            if not link.is_symlink() or link.resolve(strict=True) != path:
                raise ValueError('CTS bundle testcase artifact link differs: ' + name)
            if sha(path.read_bytes()) != record['sha256']:
                raise ValueError('CTS testcase artifact differs: ' + name)
    for name, target in manifest['directories'].items():
        link = bundle / 'harness/android-cts' / name
        if not link.is_symlink() or link.resolve(strict=True) != Path(target):
            raise ValueError('CTS bundle harness directory link differs: ' + name)
    return {'directory': str(bundle), 'identity': identity, 'manifest': manifest}


def snapshot(root, harness, destination):
    root = Path(root).resolve(strict=True)
    harness = Path(harness).resolve(strict=True)
    destination = Path(destination)
    captured = {name: (root / name).read_bytes() for name in SCRIPTS}
    for name in HARNESS_SCRIPTS:
        captured['harness/android-cts/tools/' + name] = (harness / 'tools' / name).read_bytes()
    external = {}
    for path in sorted((harness / 'tools').glob('*.jar')):
        external[path.name] = {'path': str(path.resolve()), 'sha256': sha(path.read_bytes())}
    testcase_files = {str(path.relative_to(harness / 'testcases')):
                      {'path': str(path.resolve()), 'sha256': sha(path.read_bytes())}
                      for path in sorted((harness / 'testcases').rglob('*')) if path.is_file()}
    manifest = {'root': str(root), 'harness': str(harness),
                'directories': {name: str((harness / name).resolve())
                                for name in ['results', 'logs'] if (harness / name).exists()},
                'testcase_files': testcase_files,
                'files': {name: {'sha256': sha(data), 'executable': name.endswith('.sh')
                           or name.rsplit('/', 1)[-1] in HARNESS_SCRIPTS}
                          for name, data in captured.items()}, 'external': external}
    identity = sha(json.dumps(manifest, sort_keys=True, separators=(',', ':')).encode())
    destination.mkdir(parents=True, exist_ok=True)
    bundle = destination / identity
    if bundle.exists():
        return validate(bundle)
    temporary = Path(tempfile.mkdtemp(prefix='.snapshot-', dir=destination))
    try:
        for name, data in captured.items():
            path = temporary / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(data)
            path.chmod(0o555 if manifest['files'][name]['executable'] else 0o444)
        # Preserve the original launcher's relative CTS layout without copying
        # or changing official test artifacts. Their immutable pins remain owners.
        for name in ['results', 'logs']:
            target = harness / name
            if target.exists():
                (temporary / 'harness/android-cts' / name).symlink_to(target, target_is_directory=True)
        for path in sorted((harness / 'tools').glob('*.jar')):
            (temporary / 'harness/android-cts/tools' / path.name).symlink_to(path.resolve())
        # The official launcher uses find without following directory links.
        # Real testcase directories retain its host preparer/resource classpath.
        testcases = temporary / 'harness/android-cts/testcases'
        testcases.mkdir(parents=True)
        for name, record in testcase_files.items():
            link = testcases / name
            link.parent.mkdir(parents=True, exist_ok=True)
            link.symlink_to(record['path'])

        (temporary / 'manifest.json').write_text(json.dumps(manifest, indent=2) + '\n')
        (temporary / 'manifest.json').chmod(0o444)
        try:
            temporary.rename(bundle)
        except FileExistsError:
            shutil.rmtree(temporary)
        return validate(bundle)
    except BaseException:
        if temporary.exists():
            shutil.rmtree(temporary)
        raise
