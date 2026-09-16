#!/bin/bash
set -euo pipefail
root="$(cd "$(dirname "$0")/../.." && pwd)"
stage="$(mktemp -d "${TMPDIR:-/tmp}/media-format-contract.XXXXXX")"
trap 'rm -rf -- "$stage"' EXIT
inputs=("$root/compat/media/format.cc")
if [[ "${1:-}" == "--runtime" ]]; then
  inputs=(-DDARWIN_ART_TEST_MEDIA_COMPOSITE
    -Wl,-rpath,"$root/_build/runtime-graphics-link-probe"
    "$root/_build/runtime-graphics-link-probe/libdarwin_art_runtime_graphics.dylib")
elif [[ $# != 0 ]]; then
  echo "usage: $0 [--runtime]" >&2
  exit 2
fi
xcrun clang++ -std=c++20 -Wall -Wextra -Werror -fsanitize=address,undefined \
  -I"$root/_aosp/frameworks/av/media/ndk/include" \
  "${inputs[@]}" \
  "$root/tools/tests/media-format-contract-test.cc" -o "$stage/test"
"$stage/test"
