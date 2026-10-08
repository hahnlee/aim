#!/usr/bin/env python3
"""Build original AOSP deapexer helpers for Darwin; see #1166.

All checkouts, bootstrap tools and builds live in the specified output tree.
No official CTS archive or shared AOSP checkout is changed.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import urllib.request

ROOT = Path(__file__).resolve().parent.parent


def run(args, cwd, env):
    print('+', ' '.join(map(str, args)), flush=True)
    subprocess.run(list(map(str, args)), cwd=cwd, env=env, check=True)


def archive(name, url, digest, out, env):
    path = out / 'sources' / (name + ('.tar.gz' if url.endswith('.gz') else '.tar.xz'))
    path.parent.mkdir(parents=True, exist_ok=True)
    if not path.exists():
        with urllib.request.urlopen(url) as source, path.open('wb') as dest:
            shutil.copyfileobj(source, dest)
    actual = hashlib.sha256(path.read_bytes()).hexdigest()
    if actual != digest:
        raise RuntimeError(f'{name}: archive sha256 {actual}, expected {digest}')
    dest = out / 'src' / name
    if not dest.exists():
        dest.mkdir(parents=True)
        run(['tar', '-xf', path, '--strip-components=1', '-C', dest], out, env)
    return dest


def checkout(name, project, revision, out, env):
    dest = out / 'src' / name
    if not (dest / '.git').exists():
        dest.mkdir(parents=True, exist_ok=True)
        run(['git', 'init', '-q'], dest, env)
        run(['git', 'remote', 'add', 'origin',
             'https://android.googlesource.com/platform/' + project], dest, env)
        run(['git', 'fetch', '--depth=1', 'origin', revision], dest, env)
        run(['git', 'checkout', '--detach', 'FETCH_HEAD'], dest, env)
    actual = subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=dest).decode().strip()
    dirty = subprocess.check_output(['git', 'status', '--porcelain'], cwd=dest).decode()
    if actual != revision or dirty:
        raise RuntimeError(f'{dest}: requires clean pinned revision {revision}')
    # Bootstrap writes generated files: operate only on a separate owned copy.
    work = out / 'work' / name
    if work.exists():
        shutil.rmtree(work)
    shutil.copytree(dest, work, symlinks=True, ignore=shutil.ignore_patterns('.git'))
    return work


def configure(name, source, options, out, env, jobs, install=True):
    build = out / 'build' / name
    build.mkdir(parents=True, exist_ok=True)
    key = hashlib.sha256((source / 'configure').read_bytes() + json.dumps(options).encode()).hexdigest()
    stamp = build / '.installed'
    if stamp.exists() and stamp.read_text() == key:
        return
    run([source / 'configure', '--prefix=' + str(out / 'prefix'), *options], build, env)
    run(['make', '-j' + str(jobs)], build, env)
    if install:
        run(['make', 'install'], build, env)
    stamp.write_text(key)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, default=ROOT / 'target/aim/cts-host-tools')
    parser.add_argument('--jobs', type=int, default=3)
    args = parser.parse_args()
    out = args.output.resolve()
    if sys.platform != 'darwin':
        parser.error('this recipe produces Darwin host tools')
    if not out.is_relative_to(ROOT / 'target'):
        parser.error('output must be an owned directory below target/')
    out.mkdir(parents=True, exist_ok=True)
    (out / 'provenance.json').unlink(missing_ok=True)
    lock = json.loads((ROOT / 'upstream/cts-host-tools.lock.json').read_text())
    env = os.environ.copy()
    env['PATH'] = str(out / 'prefix/bin') + os.pathsep + env['PATH']
    env['PKG_CONFIG_PATH'] = str(out / 'prefix/lib/pkgconfig')
    env['PKG_CONFIG_LIBDIR'] = str(out / 'prefix/lib/pkgconfig')
    env['ACLOCAL_PATH'] = str(out / 'prefix/share/aclocal')
    env['CPPFLAGS'] = '-I' + str(out / 'prefix/include')
    env['LDFLAGS'] = '-L' + str(out / 'prefix/lib')
    for tool in ['git', 'clang', 'make', 'm4', 'perl']:
        if not shutil.which(tool, path=env['PATH']):
            raise RuntimeError('required bootstrap tool missing: ' + tool)
    sources = {name: archive(name, *pin, out, env) for name, pin in lock['archives'].items()}
    for name in ['m4', 'autoconf', 'automake', 'pkgconf', 'libtool']:
        configure(name, sources[name], [], out, env, args.jobs)
    libtoolize = out / 'prefix/bin/glibtoolize'
    if not libtoolize.exists():
        libtoolize.symlink_to('libtoolize')
    pkg_config = out / 'prefix/bin/pkg-config'
    if not pkg_config.exists():
        pkg_config.symlink_to('pkgconf')
    configure('xz', sources['xz'], ['--disable-shared', '--enable-static'], out, env, args.jobs)
    run([sources['zlib'] / 'configure', '--static', '--prefix=' + str(out / 'prefix')], sources['zlib'], env)
    run(['make', '-j' + str(args.jobs), 'install'], sources['zlib'], env)
    sources.update({name: checkout(name, *pin, out, env) for name, pin in lock['git'].items()})
    run(['make', '-j' + str(args.jobs), 'BUILD_SHARED=no', 'PREFIX=' + str(out / 'prefix'), 'install'],
        sources['lz4'] / 'lib', env)
    run(['make', '-j' + str(args.jobs), 'PREFIX=' + str(out / 'prefix'), 'install-static', 'install-pc', 'install-includes'],
        sources['zstd'] / 'lib', env)
    cmake = sources['cmake'] / 'CMake.app/Contents/bin/cmake'
    run([cmake, '-S', sources['protobuf'], '-B', out / 'build/protobuf',
         '-DCMAKE_INSTALL_PREFIX=' + str(out / 'prefix'), '-Dprotobuf_BUILD_TESTS=OFF',
         '-Dprotobuf_BUILD_SHARED_LIBS=OFF', '-DCMAKE_BUILD_TYPE=Release',
         '-DCMAKE_CXX_FLAGS=-I' + str(sources['protobuf'] / 'config')], out, env)
    run([cmake, '--build', out / 'build/protobuf', '--parallel', args.jobs], out, env)
    run([cmake, '--install', out / 'build/protobuf'], out, env)
    configure('e2fsprogs', sources['e2fsprogs'], ['--disable-nls', '--disable-fsck', '--disable-e2fsck',
              '--disable-uuidd', '--disable-fuse2fs', '--enable-libuuid', '--enable-libblkid'], out, env, args.jobs, install=False)
    uuid = out / 'build/e2fsprogs/lib/uuid'
    (out / 'prefix/include/uuid').mkdir(parents=True, exist_ok=True)
    (out / 'prefix/lib/libuuid.a').unlink(missing_ok=True)
    shutil.copy2(uuid / 'libuuid.a', out / 'prefix/lib/libuuid.a')
    (out / 'prefix/include/uuid/uuid.h').unlink(missing_ok=True)
    shutil.copy2(sources['e2fsprogs'] / 'lib/uuid/uuid.h', out / 'prefix/include/uuid/uuid.h')
    (out / 'prefix/lib/pkgconfig/uuid.pc').unlink(missing_ok=True)
    shutil.copy2(uuid / 'uuid.pc', out / 'prefix/lib/pkgconfig/uuid.pc')
    for shared in (out / 'prefix/lib').glob('libzstd*.dylib'):
        shared.unlink()
    run(['sh', 'autogen.sh'], sources['erofs'], env)
    configure('erofs', sources['erofs'], ['--disable-shared', '--enable-static', '--with-uuid',
              '--without-selinux', '--enable-lz4', '--enable-lzma', '--with-libzstd', '--with-zlib'], out, env, args.jobs)
    config = (out / 'build/erofs/config.h').read_text()
    for macro in ['LZ4_ENABLED', 'HAVE_LIBLZMA', 'HAVE_LIBZSTD', 'HAVE_ZLIB']:
        if '#define ' + macro + ' 1' not in config:
            raise RuntimeError('required EROFS codec missing: ' + macro)
    # Keep the original Python sources and compiler/runtime from the same pin.
    shutil.copytree(sources['protobuf'] / 'python/google', out / 'python/google', dirs_exist_ok=True)
    runtime_protos = sorted((sources['protobuf'] / 'src/google/protobuf').glob('*.proto'))
    run([out / 'prefix/bin/protoc', '-I' + str(sources['protobuf'] / 'src'),
         '--python_out=' + str(out / 'python'), *runtime_protos], out, env)
    for path in ['tools/deapexer.py', 'apexer/apex_manifest.py']:
        shutil.copy2(sources['apex'] / path, out / 'python' / Path(path).name)
    run([out / 'prefix/bin/protoc', '-I' + str(sources['apex'] / 'proto'),
         '--python_out=' + str(out / 'python'), sources['apex'] / 'proto/apex_manifest.proto'], out, env)
    for source, name in [(out / 'build/e2fsprogs/debugfs/debugfs', 'debugfs_static'),
                         (out / 'prefix/bin/fsck.erofs', 'fsck.erofs')]:
        shutil.copy2(source, out / name)
    launcher = '#!/bin/sh\nbase=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)\nexport PYTHONPATH="$base/python"\nexec python3 "$base/python/deapexer.py" --debugfs_path "$base/debugfs_static" --fsckerofs_path "$base/fsck.erofs" "$@"\n'
    (out / 'deapexer').write_text(launcher)
    (out / 'deapexer').chmod(0o755)
    provenance = {'pins': lock, 'recipe_sha256': hashlib.sha256(Path(__file__).read_bytes()).hexdigest(), 'platform': sys.platform, 'python': sys.version,
                  'prerequisites': {name: subprocess.check_output([name, '--version'], env=env, stderr=subprocess.STDOUT).decode(errors='replace').splitlines()[0]
                                    for name in ['clang', 'make', 'm4', 'autoconf', 'automake', 'glibtoolize']},
                  'tools': {name: hashlib.sha256((out / name).read_bytes()).hexdigest()
                            for name in ['deapexer', 'debugfs_static', 'fsck.erofs']},
                  'libraries': {str(path.relative_to(out)): hashlib.sha256(path.read_bytes()).hexdigest()
                                for path in (out / 'prefix/lib').glob('*.a')},
                  'python_runtime': {str(path.relative_to(out)): hashlib.sha256(path.read_bytes()).hexdigest()
                                     for path in (out / 'python').rglob('*') if path.is_file() and '__pycache__' not in path.parts},
                  'protoc_version': subprocess.check_output([out / 'prefix/bin/protoc', '--version']).decode().strip()}
    (out / 'provenance.json').write_text(json.dumps(provenance, indent=2) + '\n')


if __name__ == '__main__':
    main()
