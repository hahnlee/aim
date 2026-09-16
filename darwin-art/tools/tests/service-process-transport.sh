#!/bin/bash
set -euo pipefail
root="$(cd "$(dirname "$0")/../.." && pwd)"
stage="$(mktemp -d "${TMPDIR:-/tmp}/service-process-transport.XXXXXX")"
trap 'rm -rf -- "$stage"' EXIT
xcrun clang++ -std=c++20 -Wall -Wextra -Werror -fsanitize=address,undefined \
  -I"$root" -I"$root/compat" -I"$root/probes" \
  -I"$root/_build/nativehelper-foundation/source/libnativehelper/include_jni" \
  "$root/tools/tests/service-process-transport-test.cc" \
  "$root/runtime/framework/os/service_process_transport.cc" \
  -o "$stage/test"
"$stage/test"
