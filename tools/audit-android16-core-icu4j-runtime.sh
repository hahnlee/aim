#!/bin/bash
set -euo pipefail
export LC_ALL=C

script_dir="$(cd "$(dirname "$0")" && pwd)"
project_root="$(cd "$script_dir/.." && pwd)"
lock_file="$project_root/upstream/android16-core-icu4j-runtime.lock"

# shellcheck disable=SC1090
source "$lock_file"

fail_gate() {
  echo "core-icu4j-runtime: $1" >&2
  exit 2
}

verify_file() {
  local file="$1" size="$2" sha="$3" actual_size actual_sha
  [[ -f "$file" ]] || fail_gate "missing input $file"
  actual_size="$(stat -f '%z' "$file")"
  [[ "$actual_size" == "$size" ]] ||
    fail_gate "size mismatch file=$file expected=$size actual=$actual_size"
  actual_sha="$(shasum -a 256 "$file" | awk '{print $1}')"
  [[ "$actual_sha" == "$sha" ]] ||
    fail_gate "checksum mismatch file=$file expected=$sha actual=$actual_sha"
}

jar="${1:-}"
[[ -n "$jar" ]] ||
  fail_gate "usage: $0 /path/to/extracted/core-icu4j.jar"
verify_file "$jar" "$CORE_ICU4J_SIZE" "$CORE_ICU4J_SHA256"

if [[ -n "${ANDROID16_SYSTEM_IMAGE:-}" ]]; then
  verify_file "$ANDROID16_SYSTEM_IMAGE" \
    "$SDK_SYSTEM_IMAGE_SIZE" "$SDK_SYSTEM_IMAGE_SHA256"
  geometry_magic="$(dd if="$ANDROID16_SYSTEM_IMAGE" bs=1 \
    skip=$((GPT_SUPER_START_LBA * 512 + 4096)) count=4 2>/dev/null | xxd -p)"
  [[ "$geometry_magic" == "$LP_GEOMETRY_MAGIC_BYTES" ]] ||
    fail_gate "LP geometry magic mismatch: $geometry_magic"
  erofs_magic="$(dd if="$ANDROID16_SYSTEM_IMAGE" bs=1 \
    skip=$((LP_SYSTEM_PHYSICAL_OFFSET + 1024)) count=4 2>/dev/null | xxd -p)"
  [[ "$erofs_magic" == "$EROFS_MAGIC_BYTES" ]] ||
    fail_gate "system EROFS magic mismatch: $erofs_magic"
fi

if [[ -n "${ANDROID16_I18N_APEX:-}" ]]; then
  verify_file "$ANDROID16_I18N_APEX" "$I18N_APEX_SIZE" "$I18N_APEX_SHA256"
  payload_sha="$(unzip -p "$ANDROID16_I18N_APEX" apex_payload.img | \
    shasum -a 256 | awk '{print $1}')"
  [[ "$payload_sha" == "$I18N_APEX_PAYLOAD_SHA256" ]] ||
    fail_gate "APEX payload checksum mismatch expected=$I18N_APEX_PAYLOAD_SHA256 actual=$payload_sha"
fi

if [[ -n "${ANDROID16_ICU_DATA:-}" ]]; then
  verify_file "$ANDROID16_ICU_DATA" "$ICU_DATA_SIZE" "$ICU_DATA_SHA256"
fi

dexdump="${DEXDUMP:-}"
if [[ -z "$dexdump" ]]; then
  sdk_root="${ANDROID_SDK_ROOT:-${ANDROID_HOME:-$HOME/Library/Android/sdk}}"
  dexdump="$(find "$sdk_root/build-tools" -type f -name dexdump 2>/dev/null | sort -V | tail -1)"
fi
[[ -x "$dexdump" ]] || fail_gate "Android SDK dexdump is required"

stage="$(mktemp -d "${TMPDIR:-/tmp}/core-icu4j-runtime.XXXXXX")"
trap 'rm -rf "$stage"' EXIT
unzip -p "$jar" classes.dex > "$stage/classes.dex"
verify_file "$stage/classes.dex" "$CORE_ICU4J_DEX_SIZE" "$CORE_ICU4J_DEX_SHA256"
"$dexdump" -f "$stage/classes.dex" > "$stage/header"
"$dexdump" -d "$stage/classes.dex" > "$stage/disassembly"

grep -F "DEX version '$CORE_ICU4J_DEX_VERSION'" "$stage/header" >/dev/null ||
  fail_gate "DEX version is not $CORE_ICU4J_DEX_VERSION"
classes="$(awk '/class_defs_size/{print $3; exit}' "$stage/header")"
methods="$(awk '/method_ids_size/{print $3; exit}' "$stage/header")"
[[ "$classes" == "$CORE_ICU4J_CLASS_COUNT" ]] ||
  fail_gate "class count mismatch expected=$CORE_ICU4J_CLASS_COUNT actual=$classes"
[[ "$methods" == "$CORE_ICU4J_METHOD_COUNT" ]] ||
  fail_gate "method count mismatch expected=$CORE_ICU4J_METHOD_COUNT actual=$methods"

version_class="$stage/version-info"
awk "/Class descriptor  : 'Landroid\/icu\/util\/VersionInfo;'/{found=1} found{print} found && /^Class #[0-9]+/{if (++classes_seen == 2) exit}" \
  "$stage/disassembly" > "$version_class"
grep -F "value         : \"$CORE_ICU4J_DATA_PATH\"" "$version_class" >/dev/null ||
  fail_gate "VersionInfo data path is not $CORE_ICU4J_DATA_PATH"
clinit="$stage/version-clinit"
awk '/android.icu.util.VersionInfo.<clinit>/{found=1} found{print} found && /catches       :/{exit}' \
  "$stage/disassembly" > "$clinit"
grep -E "const/16 v[0-9]+, #int $CORE_ICU4J_ICU_MAJOR //" "$clinit" >/dev/null ||
  fail_gate "VersionInfo does not initialize ICU major $CORE_ICU4J_ICU_MAJOR"
grep -F "const/4 v0, #int $CORE_ICU4J_ICU_MINOR //" "$clinit" >/dev/null ||
  fail_gate "VersionInfo does not initialize ICU minor $CORE_ICU4J_ICU_MINOR"
grep -F 'invoke-static {v2, v0, v1, v1}, Landroid/icu/util/VersionInfo;.getInstance:(IIII)' \
  "$clinit" >/dev/null ||
  fail_gate "VersionInfo ICU tuple is not $CORE_ICU4J_ICU_MAJOR.$CORE_ICU4J_ICU_MINOR"
grep -F 'UNICODE_16_0:Landroid/icu/util/VersionInfo;' "$clinit" >/dev/null ||
  fail_gate "VersionInfo does not initialize Unicode 16.0"
grep -F 'ICU_VERSION:Landroid/icu/util/VersionInfo;' "$clinit" >/dev/null ||
  fail_gate "VersionInfo ICU_VERSION initialization is missing"

registrars=(
  Lcom/android/icu/text/TimeZoneNamesNative\;
  Lcom/android/i18n/timezone/internal/Memory\;
  Lcom/android/i18n/util/ATrace\;
  Lcom/android/i18n/util/Log\;
  Lcom/android/icu/util/CaseMapperNative\;
  Lcom/android/icu/util/Icu4cMetadata\;
  Lcom/android/icu/util/LocaleNative\;
  Lcom/android/icu/util/UResourceBundleNative\;
  Lcom/android/icu/util/regex/PatternNative\;
  Lcom/android/icu/util/regex/MatcherNative\;
  Lcom/android/icu/charset/NativeConverter\;
)
for descriptor in "${registrars[@]}"; do
  count="$(grep -Fxc "  Class descriptor  : '$descriptor'" "$stage/disassembly" || true)"
  [[ "$count" == 1 ]] ||
    fail_gate "registrar target count mismatch descriptor=$descriptor actual=$count"
done

config_sha="$(unzip -p "$jar" android/icu/ICUConfig.properties | \
  shasum -a 256 | awk '{print $1}')"
[[ "$config_sha" == "$CORE_ICU4J_CONFIG_SHA256" ]] ||
  fail_gate "ICUConfig source resource mismatch"
compat_sha="$(unzip -p "$jar" com/android/i18n/system/ZygoteHooks_compat_config.xml | \
  shasum -a 256 | awk '{print $1}')"
[[ "$compat_sha" == "$CORE_ICU4J_ZYGOTE_COMPAT_SHA256" ]] ||
  fail_gate "ZygoteHooks compat resource mismatch"

echo "core-icu4j-runtime: ICU 76.1 DEX $CORE_ICU4J_DEX_VERSION classes=$classes methods=$methods registrars=${#registrars[@]}"
echo "core-icu4j-runtime: jar=$jar sha256=$CORE_ICU4J_SHA256"
