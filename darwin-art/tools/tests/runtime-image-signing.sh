#!/bin/bash
set -euo pipefail
root="$(cd "$(dirname "$0")/../.." && pwd)"
source "$root/tools/lib/sign-runtime-images.sh"
stage="$(mktemp -d "${TMPDIR:-/tmp}/runtime-image-signing.XXXXXX")"
cleanup() { rm -rf -- "$stage"; }
trap cleanup EXIT
elf="$root/_build/android16-ps16k-r07/extracted/conscrypt/lib64/libssl.so"
[[ "$(/usr/bin/file -b "$elf")" == ELF\ * ]]
mkdir "$stage/runtime"
cp -c "$elf" "$stage/runtime/libguest.so"
before="$(shasum -a 256 "$stage/runtime/libguest.so")"
xcrun clang -dynamiclib -arch arm64 "$root/tools/tests/runtime-signing-fixture.c" \
  -o "$stage/runtime/libnative.so"
codesign --remove-signature "$stage/runtime/libnative.so"
cp "$stage/runtime/libnative.so" "$stage/runtime/libversioned.so.1"
darwin_art_sign_runtime_images "$stage/runtime"
codesign --verify --strict "$stage/runtime/libnative.so"
codesign --verify --strict "$stage/runtime/libversioned.so.1"
[[ "$(shasum -a 256 "$stage/runtime/libguest.so")" == "$before" ]]
# Unknown library formats are errors, not silently skipped input corruption.
cp "$root/tools/tests/runtime-signing-fixture.c" "$stage/runtime/broken.so"
if darwin_art_sign_runtime_images "$stage/runtime"; then
  echo 'invalid library unexpectedly accepted' >&2
  exit 1
fi
echo 'runtime signing: native .so signed, Android ELF unchanged, invalid image rejected PASS'
