#!/usr/bin/env bash
set -euo pipefail
root="$(cd "$(dirname "$0")/../.." && pwd)"
task_tmp="$(mktemp -d "${TMPDIR:-/tmp}/scm-provider-owner.XXXXXX")"
trap 'find "$task_tmp" -depth -delete' EXIT
for sanitizer in address undefined thread; do
  clang++ -arch arm64 -std=c++20 -Wall -Wextra -Werror -Wpedantic \
    -g -O1 -fsanitize="$sanitizer" -fno-omit-frame-pointer \
    -I"$root/tools/bionic-socket-broker-adapter/src" \
    "$root/tools/bionic-socket-broker-adapter/src/scm_endpoint_provider.cc" \
    "$root/tools/bionic-socket-broker-adapter/probes/scm_provider_owner.cc" \
    -o "$task_tmp/owner-$sanitizer"
  "$task_tmp/owner-$sanitizer"
done
echo 'scm-endpoint-provider-owner: lifecycle ASan/UBSan/TSan PASS (no managed adoption)'
