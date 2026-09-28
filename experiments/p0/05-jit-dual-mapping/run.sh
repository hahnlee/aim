#!/usr/bin/env bash
# Build P0 experiment 5, sign copies of the test binary several ways
# (ad-hoc, our own binaries only) and run the mechanism matrix under each.
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
out="${P0_OUT:-${TMPDIR:-/tmp}/aim-p0}/05-jit-dual-mapping"
mkdir -p "$out"
clang -O2 -Wall -o "$out/jit" "$here/jit.c"
ent="$here/entitlements"
repo_ent="$here/../../../config/aim-host.entitlements"

variant() {  # name, codesign args...
  local name="$1"; shift
  cp "$out/jit" "$out/jit-$name"
  if [[ $# -gt 0 ]]; then codesign --force --sign - "$@" "$out/jit-$name" 2>/dev/null; fi
  "$out/jit-$name" "$name" ${COSTS:+costs}
  echo
}
variant linker-adhoc                                   # what clang produces: ad-hoc, no hardened runtime
variant hardened --options runtime
variant hardened+allow-jit --options runtime --entitlements "$ent/allow-jit.plist"
variant hardened+allow-unsigned-exec --options runtime --entitlements "$ent/allow-unsigned-executable-memory.plist"
variant hardened+all-three --options runtime --entitlements "$ent/all-three.plist"
COSTS=1 variant hardened+repo-host-entitlements --options runtime --entitlements "$repo_ent"

echo "EL0 system instructions Linux guests execute directly:"
clang -O2 -Wall -o "$out/el0_insns" "$here/el0_insns.c"
"$out/el0_insns"
