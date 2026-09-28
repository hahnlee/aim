#!/bin/bash
set -euo pipefail
project_root="$(cd "$(dirname "$0")/.." && pwd)"
test_dir="$(mktemp -d "${TMPDIR:-/tmp}/darwin-hwui-jni.XXXXXX")"
trap 'rm -rf -- "$test_dir"' EXIT
"${CXX:-clang++}" -std=c++17 -Wall -Wextra -Werror -pthread \
  -I"$project_root/compat" -I"$project_root/_aosp/libnativehelper/include_jni" \
  "$project_root/probes/hwui_jni_attachment_test.cc" -o "$test_dir/attachment-test"
"$test_dir/attachment-test"
