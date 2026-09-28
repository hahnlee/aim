#!/bin/bash
set -euo pipefail

root="$(cd "$(dirname "$0")/../.." && pwd)"
tool="$root/tools/darwin-x18-abi"
temporary="$(mktemp -d "${TMPDIR:-/tmp}/darwin-x18-abi.XXXXXX")"
trap 'rm -rf "$temporary"' EXIT

clang -arch arm64 -O2 -Wall -Wextra -Werror \
  "$tool/fixture.c" "$tool/fixture.S" -o "$temporary/default"
if "$temporary/default"; then
  echo 'darwin-x18-abi: modern-SDK fixture unexpectedly preserved x18' >&2
  exit 1
fi

# XNU preserves x18 for macOS tasks whose code-signature SDK predates the
# macOS 13 custom-x18 entitlement boundary. This is a task ABI declaration,
# not an application binary rewrite.
xcrun vtool -set-build-version macos 11.0 12.0 -replace \
  -output "$temporary/preserve-x18" "$temporary/default"
codesign --force --sign - --timestamp=none "$temporary/preserve-x18" >/dev/null
xcrun vtool -show-build "$temporary/preserve-x18" | grep -F 'sdk 12.0' >/dev/null
"$temporary/preserve-x18"

# The installed-app launcher must re-establish this task ABI after any Cargo
# relink, not only at installation time. Exercise the serialized production
# preparation path on a fresh current-SDK executable and race two launchers.
cp "$temporary/default" "$temporary/prepared-host"
"$root/tools/prepare-aim-host.sh" "$temporary/prepared-host" development &
first_prepare=$!
"$root/tools/prepare-aim-host.sh" "$temporary/prepared-host" development &
second_prepare=$!
wait "$first_prepare"
wait "$second_prepare"
xcrun vtool -show-build "$temporary/prepared-host" | grep -F 'sdk 12.0' >/dev/null
codesign --verify --strict "$temporary/prepared-host"
codesign -d --entitlements :- "$temporary/prepared-host" 2>/dev/null |
  grep -F '<key>com.apple.security.cs.allow-jit</key><true/>' >/dev/null
"$temporary/prepared-host"

echo 'darwin-x18-abi: PASS task and installed-launch preparation preserve Android x18'
