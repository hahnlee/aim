#!/usr/bin/env python3
"""Pure inventory fixtures; no Android, volume attachment, or boot."""
import copy
import ctypes
import errno
import json
import os
import struct
import sys
import tempfile
import hashlib
import importlib.util
from pathlib import Path
import stat
import unittest

spec = importlib.util.spec_from_file_location('template_structure', Path(__file__).with_name('template-structure.py'))
audit = importlib.util.module_from_spec(spec)
spec.loader.exec_module(audit)


def node(tag, attrs=None, children=None):
    return {'tag': tag, 'attrs': attrs or {}, 'children': children or []}


def fixture(uid=10001, salt='a', key='1', keyset='1'):
    names = ['com.android.chrome', 'com.google.android.webview', 'com.google.android.trichromelibrary_123']
    roots = {name: 'data/app/~~' + salt + name + '/' + name + '-' + salt for name in names}
    packages = [node('shared-user', {'name': 'fixture.shared', 'userId': str(uid)})]
    packages += [node('package', {'name': name, 'sharedUserId': str(uid), 'codePath': '/' + roots[name],
                  'ut': '1234', 'ft': '1234', 'domainSetId': '11111111-1111-4111-8111-111111111111',
                  'flags': '1', 'loadingProgress': '1.0', 'primaryCpuAbi': 'arm64-v8a'},
                 [node('proper-signing-keyset', {'identifier': keyset}),
                  node('sigs', children=[node('cert', {'index': key, 'key': 'actual-fixture-cert'})])]) for name in names]
    # Each original domain identity belongs to exactly one package.
    for i, package in enumerate(packages[1:], 1):
        package['attrs']['domainSetId'] = f'{i:08d}-1111-4111-8111-111111111111'
    packages.append(node('keyset-settings', children=[node('keys', children=[node('public-key', {'identifier': key, 'value': 'fixture-key'})]),
                    node('keysets', children=[node('keyset', {'identifier': keyset}, [node('key-id', {'identifier': key})])])]))
    xml = {path: node('state', {'version': '1'}) for path in audit.REQUIRED if not path.endswith('packages.list')}
    xml['data/system/packages.xml'] = node('packages', children=packages)
    xml['data/system/packages.xml.reservecopy'] = copy.deepcopy(xml['data/system/packages.xml'])
    xml['data/system/users/0/package-restrictions.xml'] = node('package-restrictions', children=[
        node('pkg', {'name': names[0], 'stopped': 'true', 'first-install-time': '1234'}),
        node('preferred-activities', children=[node('item', {'name': 'one'}), node('item', {'name': 'two'})])])
    xml['data/system/users/0/package-restrictions.xml.reservecopy'] = copy.deepcopy(xml['data/system/users/0/package-restrictions.xml'])
    apk_names = ['Chrome.apk', 'WebViewGoogle.apk', 'TrichromeLibrary.apk']
    files = audit.REQUIRED | {roots[name] + '/' + apk for name, apk in zip(names, apk_names)}
    inventory = {}
    for path in files:
        data = path.removesuffix('.reservecopy').encode() if path in audit.REQUIRED else path.split('/')[-2].split('-')[0].encode()
        inventory[path] = {'kind': stat.S_IFREG, 'host_mode': '0o600', 'host_uid': 501, 'host_gid': 20,
            'guest': {'uid': uid if path.startswith('data/app/') else 1000,
                      'gid': uid + 40000 if path.startswith('data/app/') else 1000, 'mode': '0o600'},
            'xattrs': {'dev.aim.guest-inode': 'fixture', 'dev.aim.xattr.security.selinux': 'label'},
            'mtime_ns': 100, 'size': len(data), 'sha256': hashlib.sha256(data).hexdigest()}
    for path in list(files):
        parent = str(Path(path).parent)
        while parent != '.':
            inventory.setdefault(parent, {'kind': stat.S_IFDIR, 'host_mode': '0o700', 'host_uid': 501, 'host_gid': 20,
                'guest': {'uid': 1000, 'gid': 1000, 'mode': '0o700'}, 'xattrs': {}, 'mtime_ns': 100, 'size': None, 'sha256': None})
            parent = str(Path(parent).parent)
    text = '\n'.join(f'{name} {uid} 0 /data/user/0/{name} default none' for name in names)
    return {'inputs': {}, 'inventory': inventory, 'xml': xml, 'texts': {'data/system/packages.list': text}}


class TemplateStructure(unittest.TestCase):
    def test_only_allowed_ids_paths_keys_and_positive_times_normalize(self):
        a, b = fixture(), fixture(10033, 'b', '7', '9')
        for path in ('data/system/packages.xml', 'data/system/packages.xml.reservecopy'):
            for package in b['xml'][path]['children'][1:4]:
                package['attrs']['ut'] = '5678'
                package['attrs']['ft'] = '5678'
                package['attrs']['domainSetId'] = package['attrs']['domainSetId'].replace('1111-4111', '2222-4222')
        self.assertTrue(audit.compare(a, b, {})['structure_pass'])

    def test_forbidden_flag_loading_and_unset_time_fail(self):
        for field, value in [('flags', '2'), ('loadingProgress', '0.0'), ('ut', '0')]:
            with self.subTest(field=field):
                a, b = fixture(), fixture()
                b['xml']['data/system/packages.xml']['children'][1]['attrs'][field] = value
                self.assertFalse(audit.compare(a, b, {})['structure_pass'])

    def test_preferred_record_order_is_preserved(self):
        a, b = fixture(), fixture()
        b['xml']['data/system/users/0/package-restrictions.xml']['children'][1]['children'].reverse()
        self.assertFalse(audit.compare(a, b, {})['structure_pass'])

    def test_host_gid_and_recorded_selinux_label_fail(self):
        for field, value in [('host_gid', 21), ('xattrs', {'dev.aim.xattr.security.selinux': 'different'})]:
            a, b = fixture(), fixture()
            b['inventory']['data/system/packages.list'][field] = value
            self.assertFalse(audit.compare(a, b, {})['structure_pass'])

    def test_unknown_app_and_apk_bytes_fail(self):
        a, b = fixture(), fixture()
        b['xml']['data/system/packages.xml']['children'][1]['attrs']['name'] = 'unexpected.package'
        with self.assertRaises(ValueError):
            audit.compare(a, b, {})
        b = fixture()
        path = next(path for path in b['inventory'] if path.endswith('.apk'))
        b['inventory'][path]['sha256'] = 'different'
        self.assertFalse(audit.compare(a, b, {})['structure_pass'])

    def test_compiled_difference_never_implies_validity(self):
        a, b = fixture(), fixture()
        base = next(path for path in a['inventory'] if path.endswith('.apk'))
        compiled = str(Path(base).parent / 'oat/arm64' / (Path(base).stem + '.odex'))
        for capture, value in [(a, 'first'), (b, 'second')]:
            capture['inventory'][compiled] = copy.deepcopy(capture['inventory'][base])
            capture['inventory'][compiled]['sha256'] = value
        result = audit.compare(a, b, {})
        self.assertTrue(result['structure_pass'])
        self.assertEqual(result['compiled_outputs'][0]['validity'], 'NOT_RUN')
        self.assertEqual(result['runtime_parity'], 'NOT_RUN')

    def test_equal_verifier_identity_and_unknown_fields_still_fail(self):
        a, b = fixture(), fixture()
        for capture in (a, b):
            capture['xml']['data/system/packages.xml']['children'].append(node('verifier', {'device': 'same-device'}))
        self.assertFalse(audit.compare(a, b, {})['structure_pass'])
        a, b = fixture(), fixture()
        b['xml']['data/system/packages.xml']['children'][1]['attrs']['future-state'] = 'unexpected'
        self.assertFalse(audit.compare(a, b, {})['structure_pass'])

    def test_named_stub_library_dirs_and_volume_internal_are_exact(self):
        a, b = fixture(), fixture()
        for capture, value in ((a, 'uuid-one'), (b, 'uuid-two')):
            root = next(path for path in capture['inventory'] if path.endswith('/Chrome.apk')).rsplit('/', 1)[0]
            for suffix in ('/lib', '/lib/arm64'):
                capture['inventory'][root + suffix] = copy.deepcopy(capture['inventory'][root])
            capture['inventory']['.fseventsd/fseventsd-uuid'] = copy.deepcopy(capture['inventory']['data/system/packages.list'])
            capture['inventory']['.fseventsd/fseventsd-uuid']['sha256'] = value
        self.assertTrue(audit.compare(a, b, {})['structure_pass'])
        for suffix in ('/unexpected.apk', '/lib/arm64/unknown.so', '/oat/arm64/unknown.odex'):
            altered = copy.deepcopy(b)
            altered['inventory'][root + suffix] = copy.deepcopy(b['inventory']['data/system/packages.list'])
            self.assertFalse(audit.compare(a, altered, {})['structure_pass'])
        altered = copy.deepcopy(b)
        altered['inventory']['data/.fseventsd/unknown'] = copy.deepcopy(b['inventory']['data/system/packages.list'])
        self.assertFalse(audit.compare(a, altered, {})['structure_pass'])
        altered = copy.deepcopy(b)
        altered['inventory'][root + '/lib/other-isa'] = copy.deepcopy(b['inventory'][root])
        self.assertFalse(audit.compare(a, altered, {})['structure_pass'])

    def test_pinned_permission_maps_and_package_rows_reorder(self):
        permission_file = 'data/misc_de/0/apexdata/com.android.permission/access.abx'
        rows = [node('app-id', {'id': '1001'}, [node('permission', {'name': 'p.a', 'flags': '1'}), node('permission', {'name': 'p.b', 'flags': '2'})]),
                node('app-id', {'id': '1002'}, [node('permission', {'name': 'p.c', 'flags': '4'})])]
        tree = node('access', children=[node('app-id-permissions', children=rows),
            node('app-id-app-ops', children=[node('app-id', {'id': '1001'}, [node('app-op', {'name': 'op.a', 'mode': '0'}), node('app-op', {'name': 'op.b', 'mode': '1'})])]),
            node('package-app-ops', children=[node('package', {'name': 'pkg'}, [node('app-op', {'name': 'op.a', 'mode': '0'}), node('app-op', {'name': 'op.b', 'mode': '1'})])]),
            node('package-versions', children=[node('package', {'name': 'a', 'version': '1'}), node('package', {'name': 'b', 'version': '2'})])])
        changed = copy.deepcopy(tree)
        for container in changed['children']:
            container['children'].reverse()
            for row in container['children']:
                row['children'].reverse()
        self.assertEqual(audit.canonical(tree, permission_file), audit.canonical(changed, permission_file))
        changed['children'][0]['children'][0]['children'][0]['attrs']['flags'] = '999'
        self.assertNotEqual(audit.canonical(tree, permission_file), audit.canonical(changed, permission_file))
        restrictions = node('package-restrictions', children=[node('pkg', {'name': 'a', 'stopped': 'true'}), node('pkg', {'name': 'b', 'stopped': 'false'})])
        changed = copy.deepcopy(restrictions)
        changed['children'].reverse()
        filename = 'data/system/users/0/package-restrictions.xml'
        self.assertEqual(audit.canonical(restrictions, filename), audit.canonical(changed, filename))

    def test_duplicate_map_keys_fail_instead_of_sorting_last_wins(self):
        filename = 'data/misc_de/0/apexdata/com.android.permission/access.abx'
        for container, entry, key in [('app-id-permissions', 'app-id', 'id'), ('app-id-app-ops', 'app-id', 'id'),
                                     ('package-app-ops', 'package', 'name'), ('package-versions', 'package', 'name')]:
            tree = node('access', children=[node(container, children=[node(entry, {key: 'same'}), node(entry, {key: 'same'})])])
            with self.assertRaisesRegex(ValueError, 'duplicate map key'):
                audit.canonical(tree, filename)
        for container, entry, child in [('app-id-permissions', 'app-id', 'permission'), ('app-id-app-ops', 'app-id', 'app-op'), ('package-app-ops', 'package', 'app-op')]:
            tree = node('access', children=[node(container, children=[node(entry, {'id': '1001', 'name': 'pkg'},
                [node(child, {'name': 'same', 'flags': '1', 'mode': '0'}), node(child, {'name': 'same', 'flags': '2', 'mode': '1'})])])])
            with self.assertRaisesRegex(ValueError, 'duplicate map key'):
                audit.canonical(tree, filename)
        tree = node('package-restrictions', children=[node('pkg', {'name': 'same'}), node('pkg', {'name': 'same'})])
        with self.assertRaisesRegex(ValueError, 'duplicate map key'):
            audit.canonical(tree, 'data/system/users/0/package-restrictions.xml')

    def test_unknown_permission_child_order_namespace_and_roles_hash_stay_strict(self):
        filename = 'data/misc_de/0/apexdata/com.android.permission/access.abx'
        tree = node('access', children=[node('app-id-permissions', children=[node('unknown', {'name': 'a'}), node('unknown', {'name': 'b'})])])
        changed = copy.deepcopy(tree)
        changed['children'][0]['children'].reverse()
        self.assertNotEqual(audit.canonical(tree, filename), audit.canonical(changed, filename))
        tree['children'][0]['tag'] = '{unknown}app-id-permissions'
        changed = copy.deepcopy(tree)
        changed['children'][0]['children'].reverse()
        self.assertNotEqual(audit.canonical(tree, filename), audit.canonical(changed, filename))
        a, b = fixture(), fixture()
        b['xml']['data/misc_de/0/apexdata/com.android.permission/roles.xml']['attrs']['packagesHash'] = 'different'
        self.assertFalse(audit.compare(a, b, {})['structure_pass'])

    def test_missing_mtime_rejects_incomplete_receipt(self):
        a, b = fixture(), fixture()
        del b['inventory']['data/system/packages.list']['mtime_ns']
        self.assertFalse(audit.compare(a, b, {})['structure_pass'])

    def test_duplicate_permission_owner_requires_exact_explicit_pair(self):
        a, b = fixture(), fixture()
        for capture, owner in [(a, 'owner.one'), (b, 'owner.two')]:
            capture['xml']['data/system/packages.xml']['children'].append(node('permissions', children=[
                node('item', {'name': 'fixture.permission', 'package': owner, 'protection': '1'})]))
        self.assertFalse(audit.compare(a, b, {})['structure_pass'])
        self.assertTrue(audit.compare(a, b, {'fixture.permission': {'owner.one', 'owner.two'}})['structure_pass'])
        with self.assertRaises(ValueError):
            audit.compare(a, b, {'fixture.permission': {'owner.one', 'owner.other'}})


class DarwinCapture(unittest.TestCase):
    def setUp(self):
        if sys.platform != 'darwin':
            raise RuntimeError('actual Darwin capture fixture NOT RUN on this platform')
        self.libc = ctypes.CDLL(None, use_errno=True)
        self.libc.setxattr.argtypes = [ctypes.c_char_p, ctypes.c_char_p, ctypes.c_void_p,
                                     ctypes.c_size_t, ctypes.c_uint32, ctypes.c_int]
        self.libc.setxattr.restype = ctypes.c_int

    def put(self, path, name, value):
        buffer = ctypes.create_string_buffer(value)
        if self.libc.setxattr(os.fsencode(path), os.fsencode(name), buffer, len(value), 0, 1) != 0:
            code = ctypes.get_errno()
            raise OSError(code, os.strerror(code), str(path))

    def test_actual_darwin_capture_preserves_all_bytes_and_metadata(self):
        with tempfile.TemporaryDirectory(prefix='aim-template-xattr-') as directory:
            base = Path(directory)
            root = base / 'volume'
            (root / 'data').mkdir(parents=True)
            file = root / 'data/payload'
            file.write_bytes(b'actual-native-capture')
            file.chmod(0o604)
            os.utime(file, ns=(1000000001, 1000000037))
            raw = b'DAGI\x01\x07\x00\x00' + struct.pack('<III', 10007, 20007, 0o640)
            attributes = {'dev.aim.guest-inode': raw,
                          'dev.aim.xattr.security.selinux': b'u:object_r:system_data_file:s0\0',
                          'dev.aim.unknown-capture': b'\x00\xff\x80unknown\x00',
                          'dev.aim.empty-capture': b''}
            for name, value in attributes.items():
                self.put(file, name, value)
            provenance = base / 'provenance.json'
            provenance.write_text(json.dumps({'capture': 'owned-disposable-fixture'}))
            before = file.stat()
            result = audit.capture(root, provenance)['inventory']['data/payload']
            for name, value in attributes.items():
                self.assertEqual(result['xattrs'][name], value.hex())
            self.assertEqual(result['guest'], {'uid': 10007, 'gid': 20007, 'mode': '0o640'})
            self.assertEqual(result['host_mode'], '0o604')
            self.assertEqual((result['host_uid'], result['host_gid']), (before.st_uid, before.st_gid))
            self.assertEqual(result['mtime_ns'], before.st_mtime_ns)
            self.assertEqual(result['sha256'], hashlib.sha256(file.read_bytes()).hexdigest())

    def test_actual_nofollow_and_missing_path_errors(self):
        with tempfile.TemporaryDirectory(prefix='aim-template-xattr-') as directory:
            base = Path(directory)
            target = base / 'target'
            target.write_bytes(b'target')
            self.put(target, 'dev.aim.target-only', b'not-the-link')
            link = base / 'link'
            link.symlink_to(target)
            self.assertNotIn('dev.aim.target-only', audit.xattrs(link))
            dangling = base / 'dangling'
            dangling.symlink_to(base / 'absent-target')
            self.assertIsInstance(audit.xattrs(dangling), dict)
            with self.assertRaises(OSError) as failure:
                audit.xattrs(base / 'missing')
            self.assertEqual(failure.exception.errno, errno.ENOENT)

    def test_actual_corrupt_dagi_is_not_defaulted(self):
        with tempfile.TemporaryDirectory(prefix='aim-template-xattr-') as directory:
            base = Path(directory)
            root = base / 'volume'
            root.mkdir()
            file = root / 'corrupt'
            file.write_bytes(b'payload')
            provenance = base / 'provenance.json'
            provenance.write_text('{}')
            for malformed in (b'bad', b''):
                self.put(file, 'dev.aim.guest-inode', malformed)
                with self.assertRaisesRegex(ValueError, 'invalid guest inode metadata'):
                    audit.capture(root, provenance)


if __name__ == '__main__':
    unittest.main()
