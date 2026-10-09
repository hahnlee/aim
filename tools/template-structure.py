#!/usr/bin/env python3
"""Audit captured template structure (docs/first-boot.md item 3).

Capture reads an already mounted, caller-owned volume; it never mounts or boots.
Compare accepts captures or the legacy {inputs,inventory,xml} report as FILE#SIDE.
Compiled ART outputs are inventoried, not declared byte-identical or valid. Runtime
parity, official CTS, BootStats and app checks are separate gates.
"""
import argparse
import base64
import collections
import copy
import hashlib
import json
import re
import stat
import struct
import sys
import uuid
import xml.etree.ElementTree as ET
from pathlib import Path
import os

SETTINGS = {
    'data/system/packages.xml', 'data/system/packages.xml.reservecopy',
    'data/system/packages.list', 'data/system/users/0/package-restrictions.xml',
    'data/system/users/0/package-restrictions.xml.reservecopy',
}
PERMISSIONS = {
    'data/misc/apexdata/com.android.permission/access.abx',
    *(f'data/misc_de/0/apexdata/com.android.permission/{name}'
      for name in ('access.abx', 'runtime-permissions.xml', 'roles.xml')),
}
REQUIRED = SETTINGS | PERMISSIONS | {p + '.reservecopy' for p in PERMISSIONS}
VOLUME_INTERNAL = {'.fseventsd', '.Spotlight-V100', '.Trashes'}
COMPILED = {'.odex', '.oat', '.vdex', '.art'}


def digest(data):
    return hashlib.sha256(data).hexdigest()


def xml_tree(data):
    if not data.startswith(b'ABX\0'):
        def node(e):
            if e.text and not e.text.isspace() or e.tail and not e.tail.isspace():
                raise ValueError('unexpected XML text')
            return {'tag': e.tag, 'attrs': dict(e.attrib), 'children': [node(c) for c in e]}
        return node(ET.fromstring(data))
    at, strings, stack, root = 4, [], [], None
    def take(n):
        nonlocal at
        value = data[at:at + n]
        if len(value) != n:
            raise ValueError('truncated ABX')
        at += n
        return value
    def u16():
        return struct.unpack('>H', take(2))[0]
    def utf():
        return take(u16()).replace(b'\xc0\x80', b'\0').decode('utf-8', 'surrogatepass')
    def intern():
        index = u16()
        if index != 65535:
            return strings[index]
        value = utf()
        strings.append(value)
        return value
    def value(kind):
        if kind == 1:
            return None
        if kind in (2, 3):
            return utf() if kind == 2 else intern()
        if kind in (4, 5):
            raw = take(u16())
            return raw.hex() if kind == 4 else base64.b64encode(raw).decode()
        if kind in (6, 7, 8, 9):
            raw = struct.unpack('>i' if kind < 8 else '>q', take(4 if kind < 8 else 8))[0]
            return format(raw, 'x') if kind in (7, 9) else str(raw)
        if kind in (10, 11):
            return repr(struct.unpack('>f' if kind == 10 else '>d', take(4 if kind == 10 else 8))[0])
        if kind in (12, 13):
            return 'true' if kind == 12 else 'false'
        raise ValueError(f'unknown ABX type {kind}')
    while at < len(data):
        token = take(1)[0]
        event, kind = token & 15, token >> 4
        if event == 2:
            stack.append({'tag': intern(), 'attrs': {}, 'children': []})
        elif event == 15:
            key, val = intern(), value(kind)
            if key in stack[-1]['attrs']:
                raise ValueError('duplicate ABX attribute')
            stack[-1]['attrs'][key] = val
        elif event == 3:
            tag, element = intern(), stack.pop()
            if tag != element['tag']:
                raise ValueError('ABX end tag mismatch')
            if stack:
                stack[-1]['children'].append(element)
            elif root is None:
                root = element
            else:
                raise ValueError('multiple ABX roots')
        elif event == 0:
            pass
        elif event == 1:
            break
        else:
            text = value(kind)
            if text and not text.isspace():
                stack[-1].setdefault('tokens', []).append([event, text])
    if root is None or stack or at != len(data):
        raise ValueError('incomplete or trailing ABX')
    return root


def walk(node):
    yield node
    for child in node['children']:
        yield from walk(child)


def load(reference):
    filename, separator, side = reference.partition('#')
    report = json.loads(Path(filename).read_text())
    if separator:
        return {key: report[key][side] for key in ('inputs', 'inventory', 'xml')} | {
            'texts': report.get('texts', {}).get(side, {})}
    return report


class Normalizer:
    def __init__(self, capture, permission_owners):
        self.capture, self.permission_owners = capture, permission_owners
        self.uids, self.paths, self.cert, self.keys, self.sets, self.domains = {}, {}, {}, {}, {}, {}
        self.stub_roots = {}
        tree = capture['xml']['data/system/packages.xml']
        uid_names = collections.defaultdict(list)
        for node in walk(tree):
            tag, attrs = node['tag'], node['attrs']
            if tag in ('package', 'shared-user') and 'userId' in attrs:
                uid_names[attrs['userId']].append(tag + ':' + attrs['name'])
            if tag == 'package' and attrs.get('codePath', '').startswith('/data/app/'):
                name = attrs['name']
                if name not in ('com.android.chrome', 'com.google.android.webview') and not re.fullmatch(r'com\.google\.android\.trichromelibrary_[0-9]+', name):
                    raise ValueError('unexpected decompressed package owner ' + name)
                path = attrs['codePath']
                self.stub_roots[name] = path.lstrip('/')
                if not re.fullmatch(r'/data/app/~~[^/]+/[^/]+', path):
                    raise ValueError(f'unexpected stub code path {path}')
                self.paths[path] = '/data/app/<container:' + attrs['name'] + '>/<stub:' + attrs['name'] + '>'
                parent = path.rsplit('/', 1)[0]
                canonical = '/data/app/<container:' + attrs['name'] + '>'
                if parent in self.paths and self.paths[parent] != canonical:
                    raise ValueError('stub container has multiple package owners')
                self.paths[parent] = canonical
            if tag == 'cert' and 'key' in attrs:
                old = self.cert.setdefault(attrs['index'], attrs['key'])
                if old != attrs['key']:
                    raise ValueError('certificate index has multiple values')
            if tag == 'public-key':
                self.keys[attrs['identifier']] = attrs['value']
            if tag in ('package', 'updated-package') and 'domainSetId' in attrs:
                value = attrs['domainSetId']
                uuid.UUID(value)
                if uuid.UUID(value).int != 0:
                    self.domains[value] = '<domain:' + attrs['name'] + '>'
        if len(self.stub_roots) != 3 or not all(name in self.stub_roots for name in ('com.android.chrome', 'com.google.android.webview')):
            raise ValueError('three expected decompressed stub owners required')
        self.uids = {key: '|'.join(sorted(names)) for key, names in uid_names.items()
                     if 10000 <= int(key) < 20000}
        for node in walk(tree):
            if node['tag'] == 'keyset':
                self.sets[node['attrs']['identifier']] = digest(json.dumps(sorted(
                    self.keys[c['attrs']['identifier']] for c in node['children'])).encode())

    def uid(self, value, gid=False):
        number = int(value)
        candidate = number - 40000 if gid and 50000 <= number < 60000 else number
        name = self.uids.get(str(candidate))
        return ('shared-app-gid:' if candidate != number else 'app-id:') + name if name else value

    def path(self, value):
        prefix = '/' if value.startswith('/') else ''
        absolute = '/' + value.lstrip('/')
        for old in sorted(self.paths, key=len, reverse=True):
            if absolute == old or absolute.startswith(old + '/'):
                return prefix + (self.paths[old] + absolute[len(old):]).lstrip('/')
        return value

    def tree(self, node, filename):
        node = copy.deepcopy(node)
        settings = filename.startswith('data/system/packages.xml')
        restrictions = 'package-restrictions.xml' in filename
        for item in walk(node):
            tag, attrs = item['tag'], item['attrs']
            for key, val in list(attrs.items()):
                if settings and tag in ('package', 'shared-user', 'updated-package') and key in ('userId', 'sharedUserId'):
                    attrs[key] = self.uid(val)
                if tag == 'app-id' and key == 'id':
                    attrs[key] = self.uid(val)
                if settings and tag in ('package', 'updated-package') and key == 'ut':
                    attrs[key] = '<boot-time>' if int(val, 16) > 0 else val
                if settings and tag in ('package', 'updated-package') and key == 'ft' and attrs.get('codePath', '').startswith('/data/app/'):
                    attrs[key] = '<boot-time>' if int(val, 16) > 0 else val
                if restrictions and tag == 'pkg' and key == 'first-install-time':
                    attrs[key] = '<boot-time>' if int(val, 16) > 0 else val
                if key == 'domainSetId' or tag == 'package-state' and key == 'id':
                    if val in self.domains:
                        attrs[key] = self.domains[val]
                if key in ('codePath', 'nativeLibraryPath', 'resourcePath', 'path', 'dataDir'):
                    attrs[key] = self.path(val)
                if settings and tag == 'cert' and key == 'index':
                    attrs[key] = 'cert:' + digest(self.cert[val].encode())
                    attrs['key'] = self.cert[val]
                if settings and tag in ('public-key', 'key-id') and key == 'identifier':
                    attrs[key] = 'key:' + digest(self.keys[val].encode())
                if settings and tag in ('keyset', 'proper-signing-keyset', 'upgrade-keyset', 'defined-keyset') and key == 'identifier':
                    attrs[key] = 'keyset:' + self.sets[val]
                if tag in ('item', 'permission') and key in ('package', 'packageName') and attrs.get('name') in self.permission_owners:
                    allowed = self.permission_owners[attrs['name']]
                    if val not in allowed:
                        raise ValueError('unexpected duplicate permission owner ' + val)
                    attrs[key] = '<duplicate-owner:' + attrs['name'] + '>'
        return node

    def metadata(self, entry):
        entry = copy.deepcopy(entry)
        guest = entry.get('guest')
        if not guest:
            raise ValueError('missing guest inode metadata')
        # Kernel attrs::apply uses root for absent owners and the physical mode
        # when no guest override exists; host ownership is never guest ownership.
        guest['uid'] = self.uid(guest.get('uid') or 0)
        guest['gid'] = self.uid(guest.get('gid') or 0, True)
        guest['mode'] = guest.get('mode') or entry['host_mode']
        entry['xattrs'].pop('dev.aim.guest-inode', None)
        return {key: entry[key] for key in ('kind', 'host_mode', 'host_uid', 'host_gid', 'guest', 'xattrs')}


def canonical(node):
    children = [canonical(child) for child in node['children']]
    # Reorder only defined named-map members; unknown/ordered children retain
    # their positions, especially preferred resolver and intent-filter records.
    members = {'packages': {'package', 'updated-package', 'shared-user'},
               'keys': {'public-key'}, 'keysets': {'keyset'},
               'permissions': {'item', 'permission'}, 'app-ids': {'app-id'}}
    for tag in members.get(node['tag'], set()):
        positions = [i for i, child in enumerate(node['children']) if child['tag'] == tag]
        ordered = sorted((children[i] for i in positions), key=repr)
        for position, child in zip(positions, ordered):
            children[position] = child
    return node['tag'], sorted(node['attrs'].items()), children, node.get('tokens', [])


def shipped(path, normalizer):
    if path in REQUIRED:
        return True
    if path.startswith('data/dalvik-cache/arm64/'):
        return Path(path).suffix in COMPILED or path.endswith('@classes.dex')
    for root in normalizer.stub_roots.values():
        if path == root + '/base.apk':
            return True
        prefix = root + '/oat/arm64/'
        if path.startswith(prefix) and '/' not in path[len(prefix):]:
            return Path(path).suffix in COMPILED
    return False


def audit(capture, normalizer):
    inventory = capture['inventory']
    files = {p for p, entry in inventory.items() if entry.get('sha256') is not None}
    required_apks = {root + '/base.apk' for root in normalizer.stub_roots.values()}
    missing = sorted((REQUIRED | required_apks) - files)
    extra = sorted(p for p in files if not shipped(p, normalizer) and p.split('/')[0] not in VOLUME_INTERNAL)
    errors = [f'missing shipped file {p}' for p in missing] + [f'unexpected shipped file {p}' for p in extra]
    for node in walk(capture['xml']['data/system/packages.xml']):
        if node['tag'] == 'verifier' and 'device' in node['attrs']:
            errors.append('per-device verifier identity must not be shipped')
    for required in REQUIRED - {'data/system/packages.list'}:
        if required not in capture['xml']:
            errors.append('missing parsed shipped XML ' + required)
    for path in files & REQUIRED:
        if path.endswith('.reservecopy') and inventory[path]['sha256'] != inventory[path.removesuffix('.reservecopy')]['sha256']:
            errors.append('reserve copy differs from primary ' + path)
    result = {}
    for path, entry in inventory.items():
        if path.split('/')[0] in VOLUME_INTERNAL:
            continue
        if path != 'data' and not path.startswith('data/'):
            errors.append('unexpected volume path ' + path)
            continue
        if not isinstance(entry.get('mtime_ns'), int):
            errors.append('missing observed mtime receipt ' + path)
        if entry['kind'] not in (stat.S_IFDIR, stat.S_IFREG):
            errors.append('unexpected shipped inode kind ' + path)
        if entry['kind'] == stat.S_IFDIR and path not in files:
            if not any(p.startswith(path + '/') for p in files if shipped(p, normalizer)):
                errors.append('unexpected empty directory ' + path)
        normalized = normalizer.path(path)
        if normalized in result:
            errors.append('normalized path collision ' + path)
        try:
            result[normalized] = normalizer.metadata(entry)
        except ValueError as error:
            errors.append(path + ': ' + str(error))
    return errors, result


def packages_list(text, normalizer):
    rows = []
    for row in text.splitlines():
        fields = row.split()
        if len(fields) < 6:
            raise ValueError('incomplete packages.list row')
        fields[1] = str(normalizer.uid(fields[1]))
        fields[3] = normalizer.path(fields[3])
        if fields[5] != 'none':
            fields[5] = ','.join(str(normalizer.uid(gid, True)) for gid in fields[5].split(',') if gid)
        rows.append(fields)
    return sorted(rows)


def compare(left, right, permission_owners):
    a, b = Normalizer(left, permission_owners), Normalizer(right, permission_owners)
    errors_a, metadata_a = audit(left, a)
    errors_b, metadata_b = audit(right, b)
    differences = [{'side': 'left', 'error': error} for error in errors_a]
    differences += [{'side': 'right', 'error': error} for error in errors_b]
    for path in sorted(metadata_a.keys() | metadata_b.keys()):
        if metadata_a.get(path) != metadata_b.get(path):
            differences.append({'path': path, 'error': 'metadata or shipped path differs'})
    xml_paths = set(left['xml']) | set(right['xml'])
    for path in sorted(xml_paths):
        if path not in left['xml'] or path not in right['xml']:
            differences.append({'path': path, 'error': 'XML capture missing'})
        elif canonical(a.tree(left['xml'][path], path)) != canonical(b.tree(right['xml'][path], path)):
            differences.append({'path': path, 'error': 'normalized XML differs'})
    compiled, mtimes = [], []
    entries_a = {a.path(p): v for p, v in left['inventory'].items()}
    entries_b = {b.path(p): v for p, v in right['inventory'].items()}
    for path in sorted(entries_a.keys() & entries_b.keys()):
        entry_a, entry_b = entries_a[path], entries_b[path]
        if entry_a.get('mtime_ns') != entry_b.get('mtime_ns'):
            mtimes.append({'path': path, 'left_ns': entry_a.get('mtime_ns'), 'right_ns': entry_b.get('mtime_ns')})
        if entry_a.get('sha256') is None:
            continue
        if path in xml_paths:
            continue
        if path == 'data/system/packages.list':
            if path not in left.get('texts', {}) or path not in right.get('texts', {}):
                differences.append({'path': path, 'error': 'packages.list text capture missing'})
            elif packages_list(left['texts'][path], a) != packages_list(right['texts'][path], b):
                differences.append({'path': path, 'error': 'packages.list differs'})
        elif Path(path).suffix in COMPILED or path.startswith('data/dalvik-cache/arm64/'):
            compiled.append({'path': path, 'bytes_equal': entry_a['sha256'] == entry_b['sha256'],
                             'sizes': [entry_a.get('size'), entry_b.get('size')], 'validity': 'NOT_RUN'})
        elif entry_a['sha256'] != entry_b['sha256'] or entry_a.get('size') != entry_b.get('size'):
            differences.append({'path': path, 'error': 'bytes differ'})
    return {'structure_pass': not differences, 'differences': differences,
            'compiled_outputs': compiled, 'observed_mtime_differences': mtimes,
            'mtime_preservation': 'NOT_PROVEN: source-to-template copy receipts required',
            'metadata_scope': 'effective guest owner/mode and recorded xattrs; absent SELinux labels remain absent',
            'runtime_parity': 'NOT_RUN'}


def capture(root, provenance):
    result = {'inputs': json.loads(provenance.read_text()), 'inventory': {}, 'xml': {}, 'texts': {}}
    for path in sorted(root.rglob('*')):
        meta = path.lstat()
        attrs = {name: os.getxattr(path, name, follow_symlinks=False).hex()
                 for name in os.listxattr(path, follow_symlinks=False)}
        raw = attrs.get('dev.aim.guest-inode')
        guest = None
        if raw:
            raw = bytes.fromhex(raw)
            if len(raw) != 20 or raw[:5] != b'DAGI\1' or raw[5] & ~7 or raw[6:8] != b'\0\0':
                raise ValueError('invalid guest inode metadata')
            values = struct.unpack('<III', raw[8:])
            guest = {key: value if raw[5] & bit else None
                     for key, bit, value in zip(('uid', 'gid', 'mode'), (1, 2, 4), values)}
            if guest['mode'] is not None:
                guest['mode'] = oct(guest['mode'] & 0o7777)
        name = str(path.relative_to(root))
        data = path.read_bytes() if stat.S_ISREG(meta.st_mode) else None
        after = path.lstat()
        if (meta.st_dev, meta.st_ino, meta.st_size, meta.st_mtime_ns) != (after.st_dev, after.st_ino, after.st_size, after.st_mtime_ns):
            raise ValueError('template changed during capture ' + str(path.relative_to(root)))
        result['inventory'][name] = {'kind': stat.S_IFMT(meta.st_mode), 'host_mode': oct(stat.S_IMODE(meta.st_mode)),
            'host_uid': meta.st_uid, 'host_gid': meta.st_gid, 'mtime_ns': meta.st_mtime_ns, 'guest': guest, 'xattrs': attrs,
            'size': len(data) if data is not None else None, 'sha256': digest(data) if data is not None else None}
        if name in REQUIRED and name != 'data/system/packages.list' and data is not None:
            result['xml'][name] = xml_tree(data)
        if name == 'data/system/packages.list' and data is not None:
            result['texts'][name] = data.decode()
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest='command', required=True)
    collect = commands.add_parser('capture', help='read an already mounted isolated template volume')
    collect.add_argument('--root', required=True, type=Path)
    collect.add_argument('--provenance', required=True, type=Path)
    collect.add_argument('--output', required=True, type=Path)
    compare_cli = commands.add_parser('compare')
    compare_cli.add_argument('--native-first', required=True)
    compare_cli.add_argument('--native-second', required=True)
    compare_cli.add_argument('--original')
    compare_cli.add_argument('--permission-owner', action='append', default=[], metavar='PERMISSION=PACKAGE,PACKAGE')
    compare_cli.add_argument('--output', required=True, type=Path)
    args = parser.parse_args()
    try:
        if args.command == 'capture':
            if args.output.resolve().is_relative_to(args.root.resolve()):
                raise ValueError('capture output must be outside the template volume')
            result = capture(args.root, args.provenance)
        else:
            owners = {}
            for item in args.permission_owner:
                name, values = item.split('=', 1)
                owners[name] = set(values.split(','))
                if len(owners[name]) != 2:
                    raise ValueError('duplicate permission owner requires exactly two packages')
            first, second = load(args.native_first), load(args.native_second)
            result = {'native_structure': compare(first, second, owners)}
            inputs_a, inputs_b = first['inputs'], second['inputs']
            same = inputs_a['sku'] == inputs_b['sku'] and inputs_a['inputs'] == inputs_b['inputs']
            result['native_same_inputs'] = same
            result['fresh_first_boot_execution'] = 'requires builder provenance/boot receipts; comparison alone does not prove fresh boot'
            if not same:
                result['native_structure']['differences'].append({'error': 'native template image/SKU/runtime inputs differ'})
                result['native_structure']['structure_pass'] = False
            if args.original:
                original = load(args.original)
                result['original_structure'] = compare(first, original, owners)
                result['original_second_structure'] = compare(second, original, owners)
                result['original_same_image_sku'] = (inputs_a['sku'] == original['inputs']['sku'] and
                    inputs_a['inputs']['overlay_receipt_sha256'] == original['inputs']['inputs']['overlay_receipt_sha256'])
                result['original_scope'] = 'structural correspondence; runtime/CTS parity NOT_RUN'
        with args.output.open('x') as output:
            json.dump(result, output, indent=2, sort_keys=True)
            output.write('\n')
        failed = args.command == 'compare' and any(not row['structure_pass'] for key, row in result.items()
                                                  if key in ('native_structure', 'original_structure', 'original_second_structure'))
        return 1 if failed else 0
    except (ValueError, KeyError, OSError, IndexError, ET.ParseError) as error:
        print(f'template audit: {error}', file=sys.stderr)
        return 2


if __name__ == '__main__':
    sys.exit(main())
