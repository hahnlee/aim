#!/bin/bash
set -euo pipefail
export LC_ALL=C

script_dir="$(cd "$(dirname "$0")" && pwd)"
tool_root="$(cd "$script_dir/.." && pwd)"
project_root="$(cd "$tool_root/../.." && pwd)"
image="${1:-$HOME/Library/Android/sdk/system-images/android-36/google_apis_playstore/arm64-v8a/system.img}"
[[ -f "$image" ]] || { echo "missing API 36 image: $image" >&2; exit 3; }
image_sha="$(shasum -a 256 "$image" 2>/dev/null | awk '{print $1}')"
case "$image_sha" in
  2ebdd63ebcbdc24d2190d1b67e4b78b1aa900a501449ec2a34ac46b3e13f1cfb)
    image_variant=4k
    expected_apex_sha=d102f218f387f7e1740df71bbb155b81adb47c95062e0fb6d92098c6e64fcd5c
    expected_apex_size=37642240
    ;;
  3dc5d638594ec40913b77f684497e992e12ecd2554447871f22d4b2f10851485)
    image_variant=16k
    expected_apex_sha=f8eeaedb2a6a1d8edbf100c2d780c00377b3006ead7e7c2b0e26324c60ef3def
    expected_apex_size=37634048
    ;;
  *)
    echo "unsupported API 36 system image SHA-256: $image_sha" >&2
    exit 3
    ;;
esac
expected_jar_sha=f649d9357d00e94dc7820cc41bea24125b8410e00c3e33ee9e7c0dba0ad9047c

temp_root="$(mktemp -d "${TMPDIR:-/tmp}/super-i18n-apex-test.XXXXXX")"
cleanup() {
  [[ -n "$temp_root" && "$temp_root" == "${TMPDIR:-/tmp}"/super-i18n-apex-test.* ]] &&
    rm -rf "$temp_root"
}
trap cleanup EXIT

cargo run --quiet --release --manifest-path "$tool_root/Cargo.toml" -- \
  "$image" "$temp_root/com.android.i18n.apex" >"$temp_root/report"
grep -Fx "target.path=/system/apex/com.android.i18n.apex target.bytes=$expected_apex_size target.sha256=$expected_apex_sha" \
  "$temp_root/report" >/dev/null
[[ "$(shasum -a 256 "$temp_root/com.android.i18n.apex" | awk '{print $1}')" == "$expected_apex_sha" ]]

cargo run --quiet --manifest-path "$project_root/tools/apex-ext2-extract/Cargo.toml" -- \
  "$temp_root/com.android.i18n.apex" "$temp_root/core-icu4j.jar" >/dev/null
[[ "$(shasum -a 256 "$temp_root/core-icu4j.jar" | awk '{print $1}')" == "$expected_jar_sha" ]]

dexdump="${ICU_DEXDUMP:-$HOME/Library/Android/sdk/build-tools/36.0.0/dexdump}"
[[ -x "$dexdump" ]] || { echo "missing API 36 dexdump: $dexdump" >&2; exit 3; }
unzip -p "$temp_root/core-icu4j.jar" classes.dex >"$temp_root/classes.dex"
"$dexdump" -d "$temp_root/classes.dex" >"$temp_root/dexdump"
grep -A5 "name          : 'ICU_BASE_NAME'" "$temp_root/dexdump" |
  grep -F 'value         : "android/icu/impl/data/icudata"' >/dev/null
grep -A5 "name          : 'ICU_DATA_VERSION_PATH'" "$temp_root/dexdump" |
  grep -F 'value         : "76b"' >/dev/null

echo "super-i18n-apex API36: PASS image=$image_variant apex=$expected_apex_sha core-icu4j=$expected_jar_sha VersionInfo=76b"
