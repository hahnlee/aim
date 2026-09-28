#!/bin/bash
set -euo pipefail
export LC_ALL=C

script_dir="$(cd "$(dirname "$0")" && pwd)"
crate_root="$(cd "$script_dir/.." && pwd)"
fixture_dir="$script_dir/fixtures"

while IFS=$'\t' read -r expected_sha expected_bytes filename; do
  [[ "$expected_sha" == "sha256" ]] && continue
  fixture="$fixture_dir/$filename"
  [[ -f "$fixture" ]] || { echo "missing fixture: $filename" >&2; exit 3; }
  [[ "$(shasum -a 256 "$fixture" | awk '{print $1}')" == "$expected_sha" ]] || {
    echo "fixture checksum mismatch: $filename" >&2
    exit 3
  }
  [[ "$(wc -c < "$fixture" | tr -d ' ')" == "$expected_bytes" ]] || {
    echo "fixture size mismatch: $filename" >&2
    exit 3
  }
done < "$fixture_dir/SHA256SUMS.tsv"

cargo test --manifest-path "$crate_root/Cargo.toml"
report="$(cargo run --quiet --manifest-path "$crate_root/Cargo.toml" -- \
  "$fixture_dir/libarm64_inspector_smoke.so")"
[[ "$report" == *'"machine":"AArch64"'* ]]
[[ "$report" == *'"soname":"libarm64_inspector_smoke.so"'* ]]
[[ "$report" == *'"versioning":{"versym":true'* ]]
[[ "$report" == *'"name":"FIXTURE_1.0"'* ]]
[[ "$report" == *'"tier1_compatible":true'* ]]
echo "android-arm64-so-inspect: PASS pinned Android arm64 fixture"
