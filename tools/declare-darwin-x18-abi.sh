#!/bin/bash
set -euo pipefail

host="${1:-}"
[[ -n "$host" && -f "$host" ]] || {
  echo "usage: $0 <aim-host>" >&2
  exit 64
}

vtool="$(xcrun -f vtool)"
stage="$(mktemp "$(dirname "$host")/.aim-host-x18.XXXXXX")"
cleanup() {
  rm -f "$stage"
}
trap cleanup EXIT

# XNU preserves x18 across scheduling for macOS tasks whose Mach-O SDK is
# earlier than 13.0. Android arm64 DSOs may use x18 as a general register,
# while a current-SDK Darwin task loses it on a context switch. Declare the
# host process's guest-code ABI before code signing; no APK bytes are changed.
"$vtool" -set-build-version macos 11.0 12.0 -replace \
  -output "$stage" "$host"
chmod "$(stat -f '%Lp' "$host")" "$stage"
mv "$stage" "$host"
trap - EXIT

"$vtool" -show-build "$host" | grep -F 'sdk 12.0' >/dev/null || {
  echo "aim host x18 ABI declaration failed: $host" >&2
  exit 70
}
