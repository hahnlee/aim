#!/usr/bin/env bash
set -euo pipefail
root="$(cd "$(dirname "$0")/../.." && pwd)"
stage="$(mktemp -d "${TMPDIR:-/tmp}/verity-utils.XXXXXX")"
trap 'rm -rf -- "$stage"' EXIT
xcrun clang++ -std=c++20 -Wall -Wextra -Werror -Wno-deprecated-declarations \
  -I"$root/_aosp/libnativehelper/include_jni" \
  -Ddarwin_art_libcore_stat=darwin_art_test_libcore_stat \
  "$root/compat/security/verity_utils_jni.cc" "$root/tools/tests/verity-utils-test.cc" \
  "$root/tools/tests/verity-utils-stat-stub.cc" -o "$stage/test"
"$stage/test"
