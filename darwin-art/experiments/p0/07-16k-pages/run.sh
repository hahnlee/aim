#!/usr/bin/env bash
# Build and run P0 experiment 7: plain ad-hoc signature and hardened with the
# repo's host entitlements (ad-hoc, our own test binary only).
set -uo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
out="${P0_OUT:-${TMPDIR:-/tmp}/darwin-art-p0}/07-16k-pages"
mkdir -p "$out"
clang -O2 -Wall -o "$out/pages" "$here/pages.c" || exit 1
cp "$out/pages" "$out/pages-hardened"
codesign --force --sign - --options runtime --entitlements "$here/../../../config/darwin-art-host.entitlements" "$out/pages-hardened"
echo "== linker ad-hoc signature"
"$out/pages"
echo
echo "== hardened runtime + config/darwin-art-host.entitlements"
"$out/pages-hardened"
echo
echo "== low 4 GiB and __PAGEZERO"
bash "$here/lowmem.sh"
