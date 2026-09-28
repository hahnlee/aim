#!/usr/bin/env bash
# P0 experiment 7b driver: lowmem.c under several __PAGEZERO sizes.
set -uo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
out="${P0_OUT:-${TMPDIR:-/tmp}/aim-p0}/07-16k-pages"
mkdir -p "$out"
clang -O2 -o "$out/low-default" "$here/lowmem.c" || exit 1
echo "__PAGEZERO default (4 GiB):"
"$out/low-default"
for z in 0x4000 0x100000 0x10000000 0x80000000 0xffffc000; do
  clang -O2 -Wl,-pagezero_size,"$z" -o "$out/low-$z" "$here/lowmem.c" || exit 1
  echo "__PAGEZERO $z:"
  "$out/low-$z"
  echo "  exit status $?"
done
