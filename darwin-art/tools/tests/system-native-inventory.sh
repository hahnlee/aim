#!/bin/bash
set -euo pipefail
root="$(cd "$(dirname "$0")/../.." && pwd)"
source "$root/tools/lib/system-native-inventory.sh"
stage="$(mktemp -d "${TMPDIR:-/tmp}/native-inventory-test.XXXXXX")"
trap 'rm -rf -- "$stage"' EXIT
mkdir -p "$stage/input/system/lib64"
file="$stage/input/system/lib64/libtest.so"
printf 'fixture bytes\n' > "$file"
digest="$(shasum -a 256 "$file")"
printf '%s /system/lib64/libtest.so\n' "${digest%% *}" > "$stage/inventory"
verify() { darwin_art_verify_system_native_inventory "$stage/input" "$stage/inventory"; }
verify
printf 'changed bytes\n' > "$file"
if verify; then echo 'accepted changed bytes' >&2; exit 1; fi
mv "$file" "$stage/other"
if verify; then echo 'accepted missing file' >&2; exit 1; fi
ln -s "$stage/other" "$file"
if verify; then echo 'accepted symlink' >&2; exit 1; fi
echo 'system native inventory: exact bytes, changed/missing/symlink rejection PASS'
