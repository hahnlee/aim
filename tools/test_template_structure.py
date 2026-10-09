#!/usr/bin/env python3
"""Pure inventory fixtures; no Android, volume attachment, or boot."""
import copy
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
                  'flags': '1', 'loadingProgress': '1.0'},
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
    files = audit.REQUIRED | {root + '/base.apk' for root in roots.values()}
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
        compiled = base.removesuffix('base.apk') + 'oat/arm64/base.odex'
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


if __name__ == '__main__':
    unittest.main()
