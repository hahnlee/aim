#!/usr/bin/env bash
set -euo pipefail

host="${1:-}"
mode="${2:-development}"
locked="${3:-}"
[[ -n "$host" && -x "$host" ]] || {
  echo "usage: $0 HOST [development|packaged]" >&2
  exit 64
}
[[ "$mode" == "development" || "$mode" == "packaged" ]] || {
  echo "unsupported aim host mode: $mode" >&2
  exit 64
}

if [[ "$locked" != "--locked" ]]; then
  # Cargo may replace the shared host after package installation. Serialize
  # the atomic ABI/signature update so concurrent app launches never execute
  # an intermediate file. Keep the lock outside an app bundle: a lock file
  # next to Contents/MacOS/aim-host becomes a sealed resource and
  # invalidates the nested Host.app signature.
  lock_root="${AIM_LOCK_ROOT:-${TMPDIR:-/tmp}}"
  lock_name="$(printf '%s' "$host" | cksum | awk '{print $1}')"
  exec lockf -k "$lock_root/aim-host-x18.$lock_name.lock" \
    "$0" "$host" "$mode" --locked
fi

root="$(cd "$(dirname "$0")/.." && pwd)"
has_x18_abi() {
  xcrun vtool -show-build "$host" 2>/dev/null | grep -F 'sdk 12.0' >/dev/null
}
has_development_entitlement() {
  codesign -d --entitlements :- "$host" 2>/dev/null |
    grep -F '<key>com.apple.security.cs.allow-jit</key><true/>' >/dev/null
}

if [[ "$mode" == "packaged" ]]; then
  has_x18_abi || {
    echo "packaged aim host lacks the Android x18 task ABI: $host" >&2
    exit 70
  }
  has_development_entitlement || {
    echo "packaged aim host lacks the allow-jit entitlement: $host" >&2
    exit 71
  }
  codesign --verify --strict "$host"
  exit 0
fi

if ! has_x18_abi; then
  "$root/tools/declare-darwin-x18-abi.sh" "$host"
fi

signature_ok=0
if codesign --verify --strict "$host" >/dev/null 2>&1 &&
  has_development_entitlement; then
  signature_ok=1
fi
if [[ "$signature_ok" != "1" ]]; then
  codesign --force --sign - --options runtime \
    --entitlements "$root/config/aim-host.entitlements" "$host" >/dev/null
fi

has_x18_abi
codesign --verify --strict "$host"
