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
import ctypes
import errno
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
        self.stub_roots, self.stub_apks, self.stub_dirs = {}, {}, set()
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
                # Original decompressFiles removes only .gz; it does not rename to base.apk.
                basename = {'com.android.chrome': 'Chrome.apk', 'com.google.android.webview': 'WebViewGoogle.apk'}.get(name, 'TrichromeLibrary.apk')
                self.stub_apks[path.lstrip('/')] = basename
                self.stub_dirs.add(path.lstrip('/') + '/lib')
                instruction_sets = {'arm64-v8a': 'arm64', 'armeabi-v7a': 'arm', 'armeabi': 'arm', 'x86': 'x86', 'x86_64': 'x86_64', 'arm64-v8a-hwasan': 'arm64', 'riscv64': 'riscv64'}
                for field in ('primaryCpuAbi', 'secondaryCpuAbi'):
                    if field in attrs:
                        self.stub_dirs.add(path.lstrip('/') + '/lib/' + instruction_sets[attrs[field]])
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
        self.domain_owners = self.domain_aliases(tree)
        if len(self.stub_roots) != 3 or not all(name in self.stub_roots for name in ('com.android.chrome', 'com.google.android.webview')):
            raise ValueError('three expected decompressed stub owners required')
        self.uids = {key: '|'.join(sorted(names)) for key, names in uid_names.items()
                     if 10000 <= int(key) < 20000}
        for node in walk(tree):
            if node['tag'] == 'keyset':
                self.sets[node['attrs']['identifier']] = digest(json.dumps(sorted(
                    self.keys[c['attrs']['identifier']] for c in node['children'])).encode())

    def domain_aliases(self, tree):
        owners, ids, settings, seen_ps, seen_dvs = {}, {}, {}, set(), set()

        def identity(owner, value, source):
            if not isinstance(owner, str) or not owner.strip() or owner != owner.strip() or '\0' in owner:
                raise ValueError('missing or invalid domain owner ' + source)
            try:
                parsed = uuid.UUID(value)
            except (ValueError, TypeError, AttributeError) as error:
                raise ValueError('invalid domain UUID ' + source) from error
            if value != str(parsed):
                raise ValueError('noncanonical domain UUID ' + source)
            return owner, str(parsed), parsed.int

        def register(owner, value, nonzero, source):
            if owner in owners and owners[owner] != value:
                raise ValueError('domain owner has conflicting UUID ' + source)
            owners[owner] = value
            # The disabled zero UUID is a sentinel, not a generated owner alias.
            if nonzero:
                if value in ids and ids[value] != owner:
                    raise ValueError('domain UUID has conflicting owner ' + source)
                ids[value] = owner
                self.domains[value] = '<domain:' + owner + '>'

        for node in walk(tree):
            tag, attrs = node['tag'], node['attrs']
            if tag not in ('package', 'updated-package') or 'domainSetId' not in attrs:
                continue
            owner, value, nonzero = identity(attrs.get('name'), attrs['domainSetId'], tag)
            key = (tag, owner)
            if key in seen_ps:
                raise ValueError('duplicate PackageSetting domain owner ' + owner)
            seen_ps.add(key)
            register(owner, value, nonzero, tag + ':' + owner)
            settings[owner] = value
        for node in walk(tree):
            if node['tag'] != 'package-state':
                continue
            attrs = node['attrs']
            owner, value, nonzero = identity(attrs.get('packageName'), attrs.get('id'), 'package-state')
            if owner in seen_dvs:
                raise ValueError('duplicate DVS domain owner ' + owner)
            seen_dvs.add(owner)
            if owner in settings and settings[owner] != value:
                raise ValueError('PackageSetting/DVS domain ID mismatch ' + owner)
            register(owner, value, nonzero, 'package-state:' + owner)
        return owners

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
                    owner = attrs.get('packageName') if tag == 'package-state' else attrs.get('name')
                    try:
                        parsed = uuid.UUID(val)
                    except (ValueError, TypeError, AttributeError) as error:
                        raise ValueError('invalid domain UUID reference ' + filename) from error
                    value = str(parsed)
                    if owner not in self.domain_owners or self.domain_owners[owner] != value:
                        raise ValueError('domain UUID reference owner mismatch ' + filename)
                    if parsed.int != 0:
                        attrs[key] = self.domains[value]
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


def canonical(node, filename, ancestors=()):
    path = ancestors + (node['tag'],)
    maps = {}
    if filename.startswith('data/system/packages.xml'):
        maps = {('packages',): {'package': 'name', 'updated-package': 'name', 'shared-user': 'name'},
                ('packages', 'keyset-settings', 'keys'): {'public-key': 'identifier'},
                ('packages', 'keyset-settings', 'keysets'): {'keyset': 'identifier'},
                ('packages', 'permissions'): {'item': 'name'}}
    elif filename.startswith('data/system/users/0/package-restrictions.xml'):
        maps = {('package-restrictions',): {'pkg': 'name'}}
    elif filename.startswith('data/misc_de/0/apexdata/com.android.permission/access.abx'):
        # Pinned persistence readers assign by key; duplicate rows are last-wins
        # and ambiguous, so fail them before treating writer order as incidental.
        maps = {('access', 'package-versions'): {'package': 'name'},
                ('access', 'package-app-ops'): {'package': 'name'},
                ('access', 'package-app-ops', 'package'): {'app-op': 'name'},
                ('access', 'app-id-app-ops'): {'app-id': 'id'},
                ('access', 'app-id-app-ops', 'app-id'): {'app-op': 'name'},
                ('access', 'app-id-permissions'): {'app-id': 'id'},
                ('access', 'app-id-permissions', 'app-id'): {'permission': 'name'}}
    children = [canonical(child, filename, path) for child in node['children']]
    for tag, key in maps.get(path, {}).items():
        positions = [i for i, child in enumerate(node['children']) if child['tag'] == tag]
        seen = set()
        for position in positions:
            attrs = node['children'][position]['attrs']
            if key not in attrs or attrs[key] in seen:
                raise ValueError('missing or duplicate map key ' + '/'.join(path) + '/' + tag + '@' + key)
            seen.add(attrs[key])
        ordered = sorted((children[i] for i in positions), key=repr)
        for position, child in zip(positions, ordered):
            children[position] = child
    return node['tag'], sorted(node['attrs'].items()), children, node.get('tokens', [])


def shipped(path, normalizer):
    if path in REQUIRED:
        return True
    if path.startswith('data/dalvik-cache/arm64/'):
        return Path(path).suffix in COMPILED or path.endswith('@classes.dex')
    for root, basename in normalizer.stub_apks.items():
        if path == root + '/' + basename:
            return True
        prefix = root + '/oat/arm64/'
        if path.startswith(prefix) and '/' not in path[len(prefix):]:
            return Path(path).name in {Path(basename).stem + suffix for suffix in COMPILED}
    return False


def audit(capture, normalizer):
    inventory = capture['inventory']
    files = {p for p, entry in inventory.items() if entry.get('sha256') is not None}
    required_apks = {root + '/' + name for root, name in normalizer.stub_apks.items()}
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
            if path not in normalizer.stub_dirs and not any(p.startswith(path + '/') for p in files if shipped(p, normalizer)):
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
        elif canonical(a.tree(left['xml'][path], path), path) != canonical(b.tree(right['xml'][path], path), path):
            differences.append({'path': path, 'error': 'normalized XML differs'})
    compiled, mtimes = [], []
    entries_a = {a.path(p): v for p, v in left['inventory'].items() if p.split('/')[0] not in VOLUME_INTERNAL}
    entries_b = {b.path(p): v for p, v in right['inventory'].items() if p.split('/')[0] not in VOLUME_INTERNAL}
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


def xattrs(path):
    if sys.platform != 'darwin':
        if not hasattr(os, 'listxattr') or not hasattr(os, 'getxattr'):
            raise OSError(errno.ENOSYS, 'native xattr capture unavailable', os.fspath(path))
        return {name: os.getxattr(path, name, follow_symlinks=False).hex()
                for name in os.listxattr(path, follow_symlinks=False)}
    # Darwin sys/xattr.h: ssize_t results, position=0, XATTR_NOFOLLOW=0x0001.
    libc = ctypes.CDLL(None, use_errno=True)
    libc.listxattr.argtypes = [ctypes.c_char_p, ctypes.c_void_p, ctypes.c_size_t, ctypes.c_int]
    libc.listxattr.restype = ctypes.c_ssize_t
    libc.getxattr.argtypes = [ctypes.c_char_p, ctypes.c_char_p, ctypes.c_void_p,
                             ctypes.c_size_t, ctypes.c_uint32, ctypes.c_int]
    libc.getxattr.restype = ctypes.c_ssize_t
    encoded = os.fsencode(path)
    if b'\0' in encoded:
        raise ValueError('NUL in xattr path')
    def checked(result):
        if result < 0:
            code = ctypes.get_errno()
            raise OSError(code, os.strerror(code), os.fspath(path))
        return result
    length = checked(libc.listxattr(encoded, None, 0, 1))
    names = ctypes.create_string_buffer(max(length, 1))
    if checked(libc.listxattr(encoded, names, length, 1)) != length:
        raise OSError(errno.EIO, 'xattr name list changed during capture', os.fspath(path))
    raw = names.raw[:length]
    if raw and not raw.endswith(b'\0'):
        raise OSError(errno.EIO, 'unterminated native xattr list', os.fspath(path))
    result = {}
    for name in raw.split(b'\0'):
        if not name:
            continue
        size = checked(libc.getxattr(encoded, name, None, 0, 0, 1))
        value = ctypes.create_string_buffer(max(size, 1))
        if checked(libc.getxattr(encoded, name, value, size, 0, 1)) != size:
            raise OSError(errno.EIO, 'xattr value changed during capture', os.fspath(path))
        result[os.fsdecode(name)] = value.raw[:size].hex()
    return result


def capture(root, provenance):
    result = {'inputs': json.loads(provenance.read_text()), 'inventory': {}, 'xml': {}, 'texts': {}}
    for path in sorted(root.rglob('*')):
        meta = path.lstat()
        attrs = xattrs(path)
        raw = attrs.get('dev.aim.guest-inode')
        guest = None
        if raw is not None:
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
