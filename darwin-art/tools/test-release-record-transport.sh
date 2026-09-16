#!/bin/bash
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
stage="$(mktemp -d /tmp/darwin-release-record.XXXXXX)"
trap 'rm -rf -- "$stage"' EXIT
xcrun clang++ -std=c++23 -Wall -Wextra -Werror \
  -I"$root/compat" -I"$root/_aosp/system/libbase/include" \
  "$root/compat/surfaceflinger/release_record_transport.cc" \
  "$root/tools/tests/release-record-transport-test.cc" -o "$stage/test"
"$stage/test"
