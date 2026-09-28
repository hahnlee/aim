#!/bin/bash
set -euo pipefail
root="$(cd "$(dirname "$0")/../.." && pwd)"
stage="$(mktemp -d "${TMPDIR:-/tmp}/image-decoder-input.XXXXXX")"
trap 'rm -rf -- "$stage"' EXIT
xcrun clang++ -std=c++20 -Wall -Wextra -Werror \
  -I"$root/compat" -I"$root/tools/bionic-fs-facade/include" \
  -I"$root/tools/bionic-ioctl-facade/include" \
  "$root/compat/graphics/image_decoder_input.cc" \
  "$root/tools/tests/image-decoder-input-test.cc" -o "$stage/test"
"$stage/test"
