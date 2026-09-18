#!/usr/bin/env bash
set -euo pipefail
[[ $# == 2 && "$1" = /* && -f "$1" && ( "$2" == graphics || "$2" == headless ) ]] || {
  echo 'usage: surface-backing-product-test.sh absolute-runtime.dylib graphics|headless' >&2; exit 64;
}
root="$(cd "$(dirname "$0")/../.." && pwd)"
stage="$(mktemp -d "${TMPDIR:-/tmp}/darwin-art-backing-product.XXXXXX")"
trap 'rm -rf -- "$stage"' EXIT
xcrun clang++ -std=c++20 -fobjc-arc -Wall -Wextra -Werror -I"$root" \
  "$root/tools/tests/surface-backing-product-test.mm" \
  -framework AppKit -framework IOSurface -framework CoreFoundation -o "$stage/test"
"$stage/test" "$1" "$2"
