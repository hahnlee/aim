#!/bin/bash
set -euo pipefail
export LC_ALL=C

script_dir="$(cd "$(dirname "$0")" && pwd)"
project_root="$(cd "$script_dir/../.." && pwd)"

jar="${1:-$project_root/_build/bootclasspath/core-icu4j-apex-reconciled.jar}"
foundation="${2:-$project_root/_build/icu-foundation}"
data_file="${3:-$foundation/runtime/i18n/etc/icu/icudt76l.dat}"
icu_root="${4:-$project_root/_aosp/external/icu-graphics}"
common_archive="$foundation/libicuuc-common-darwin.a"
i18n_archive="$foundation/libicui18n-darwin.a"
stubdata_archive="$foundation/libicuuc-stubdata-darwin.a"
init_archive="$foundation/libandroidicuinit-darwin.a"
version_header="$icu_root/icu4c/source/common/unicode/uvernum.h"

fail_inspection() {
  echo "icu-coherence.error=$1" >&2
  exit 3
}

for command in unzip zipinfo nm clang++ xcrun shasum; do
  command -v "$command" >/dev/null || fail_inspection "missing-tool:$command"
done
for input in "$jar" "$data_file" "$common_archive" "$i18n_archive" \
             "$stubdata_archive" "$init_archive" "$version_header"; do
  [[ -f "$input" ]] || fail_inspection "missing-input:$input"
done

temp_root="$(mktemp -d "${TMPDIR:-/tmp}/icu-coherence.XXXXXX")"
cleanup() {
  [[ -n "$temp_root" && "$temp_root" == "${TMPDIR:-/tmp}"/icu-coherence.* ]] &&
    rm -rf "$temp_root"
}
trap cleanup EXIT

entries="$temp_root/jar.entries"
zipinfo -1 "$jar" >"$entries"

java_base=""
java_version_path=""
java_format=""
required_java_classes=(
  android/icu/impl/ICUData
  android/icu/impl/ICUDataVersion
  android/icu/impl/ICUResourceBundle
  android/icu/util/UResourceBundle
  com/android/icu/util/Icu4cMetadata
)

if grep -Fx 'classes.dex' "$entries" >/dev/null; then
  java_format="dex"
  dexdump="${ICU_DEXDUMP:-}"
  if [[ -z "$dexdump" ]]; then
    dexdump="$(command -v dexdump || true)"
  fi
  if [[ -z "$dexdump" ]]; then
    sdk_root="${ANDROID_SDK_ROOT:-${ANDROID_HOME:-$HOME/Library/Android/sdk}}"
    dexdump="$(find "$sdk_root/build-tools" -type f -name dexdump -perm -111 2>/dev/null | sort | tail -1)"
  fi
  [[ -x "$dexdump" ]] || fail_inspection 'missing-tool:dexdump'
  unzip -p "$jar" classes.dex >"$temp_root/classes.dex"
  "$dexdump" -d "$temp_root/classes.dex" >"$temp_root/dexdump.txt"

  extract_dex_static_string() {
    local descriptor="$1" field="$2"
    awk -v descriptor="$descriptor" -v field="$field" '
      /^Class #[0-9]+/ { in_class=0; wanted=0 }
      index($0, "Class descriptor  : '\''L" descriptor ";'\''") { in_class=1 }
      in_class && index($0, "name          : '\''" field "'\''") { wanted=1; next }
      in_class && wanted && $0 ~ /value         : "/ {
        value=$0
        sub(/^.*value         : "/, "", value)
        sub(/".*$/, "", value)
        print value
        exit
      }
    ' "$temp_root/dexdump.txt"
  }

  java_base="$(extract_dex_static_string android/icu/impl/ICUData ICU_BASE_NAME)"
  java_version_path="$(extract_dex_static_string android/icu/util/VersionInfo ICU_DATA_VERSION_PATH)"
  for class in "${required_java_classes[@]}"; do
    grep -F "Class descriptor  : 'L$class;'" "$temp_root/dexdump.txt" >/dev/null ||
      fail_inspection "missing-java-class:$class"
    echo "java.class.$class=present"
  done
elif grep -Fx 'android/icu/impl/ICUData.class' "$entries" >/dev/null; then
  java_format="class"
  command -v javap >/dev/null || fail_inspection 'missing-tool:javap'
  javap -classpath "$jar" -private -constants android.icu.impl.ICUData \
    android.icu.util.VersionInfo >"$temp_root/javap.txt"
  java_base="$(sed -n 's/.*ICU_BASE_NAME = "\([^"]*\)";.*/\1/p' "$temp_root/javap.txt")"
  java_version_path="$(sed -n 's/.*ICU_DATA_VERSION_PATH = "\([^"]*\)";.*/\1/p' "$temp_root/javap.txt")"
  for class in "${required_java_classes[@]}"; do
    grep -Fx "$class.class" "$entries" >/dev/null ||
      fail_inspection "missing-java-class:$class"
    echo "java.class.$class=present"
  done
else
  fail_inspection 'jar-has-neither-classes.dex-nor-ICUData.class'
fi

[[ "$java_base" =~ ^android/icu/impl/data/icudt([0-9]+)(b|l)$ ]] ||
  fail_inspection "invalid-java-base:$java_base"
java_major="${BASH_REMATCH[1]}"
java_endian="${BASH_REMATCH[2]}"
[[ "$java_version_path" == "${java_major}${java_endian}" ]] ||
  fail_inspection "java-version-path-disagrees:$java_version_path"

symbol_versions="$(nm -gU "$common_archive" | awk '{print $NF}' | \
  grep -E '^_u_getVersion_[0-9]+$' | sort -u || true)"
[[ "$(printf '%s\n' "$symbol_versions" | sed '/^$/d' | wc -l | tr -d ' ')" == 1 ]] ||
  fail_inspection 'ambiguous-u_getVersion-symbol'
symbol_major="${symbol_versions##*_}"

header_library_version="$(awk '$1=="#define" && $2=="U_ICU_VERSION" {gsub(/"/, "", $3); print $3}' "$version_header")"
header_data_version="$(awk '$1=="#define" && $2=="U_ICU_DATA_VERSION" {gsub(/"/, "", $3); print $3}' "$version_header")"
[[ "$header_library_version" =~ ^([0-9]+)(\.[0-9]+){1,3}$ ]] ||
  fail_inspection 'invalid-U_ICU_VERSION-header'
[[ "${BASH_REMATCH[1]}" == "$symbol_major" ]] ||
  fail_inspection 'library-header-symbol-major-mismatch'
[[ "$header_data_version" =~ ^([0-9]+)(\.[0-9]+){1,3}$ ]] ||
  fail_inspection 'invalid-U_ICU_DATA_VERSION-header'

data_name="$(basename "$data_file")"
[[ "$data_name" =~ ^icudt([0-9]+)(b|l)\.dat$ ]] ||
  fail_inspection "invalid-data-filename:$data_name"
data_name_major="${BASH_REMATCH[1]}"

sdk="$(xcrun --sdk macosx --show-sdk-path)"
clang++ -arch arm64 -isysroot "$sdk" -std=c++20 -DANDROID \
  -I"$icu_root/android_icu4c/include" \
  -I"$icu_root/icu4c/source/common" \
  -I"$icu_root/icu4c/source/i18n" \
  -I"$icu_root/libandroidicuinit/include" \
  "$script_dir/native_probe.cpp" "$i18n_archive" "$common_archive" \
  -Wl,-force_load,"$init_archive" "$stubdata_archive" \
  -o "$temp_root/native-probe"

mkdir -p "$temp_root/runtime/i18n/etc/icu" "$temp_root/runtime/data" \
         "$temp_root/runtime/tzdata"
absolute_data="$(cd "$(dirname "$data_file")" && pwd)/$(basename "$data_file")"
ln -s "$absolute_data" \
  "$temp_root/runtime/i18n/etc/icu/icudt${symbol_major}l.dat"

set +e
ANDROID_DATA="$temp_root/runtime/data" \
ANDROID_TZDATA_ROOT="$temp_root/runtime/tzdata" \
ANDROID_I18N_ROOT="$temp_root/runtime/i18n" \
  "$temp_root/native-probe" >"$temp_root/native.out" 2>"$temp_root/native.err"
probe_status=$?
set -e
cat "$temp_root/native.out"
if [[ "$probe_status" != 0 ]]; then
  cat "$temp_root/native.err" >&2
  fail_inspection "native-probe-exit:$probe_status"
fi

read_field() {
  local key="$1" value count
  count="$(grep -c "^$key=" "$temp_root/native.out" || true)"
  [[ "$count" == 1 ]] || fail_inspection "missing-or-duplicate-field:$key"
  value="$(sed -n "s/^$key=//p" "$temp_root/native.out")"
  printf '%s' "$value"
}

library_version="$(read_field 'icu4c.library.version')"
data_version="$(read_field 'icu4c.data.version')"
library_major="${library_version%%.*}"
data_major="${data_version%%.*}"

echo "java.jar=$jar"
echo "java.jar.sha256=$(shasum -a 256 "$jar" | awk '{print $1}')"
echo "java.jar.format=$java_format"
echo "java.icu_base_name=$java_base"
echo "java.icu_data_version_path=$java_version_path"
echo "icu4c.archive.symbol_major=$symbol_major"
echo "icu4c.header.library_version=$header_library_version"
echo "icu4c.header.data_version=$header_data_version"
echo "icu4c.data.file=$data_file"
echo "icu4c.data.sha256=$(shasum -a 256 "$data_file" | awk '{print $1}')"
echo "icu4c.data.filename_major=$data_name_major"

mismatches=()
[[ "$java_major" == "$library_major" ]] ||
  mismatches+=("java($java_major)!=library($library_major)")
[[ "$java_major" == "$data_major" ]] ||
  mismatches+=("java($java_major)!=data($data_major)")
[[ "$library_major" == "$data_major" ]] ||
  mismatches+=("library($library_major)!=data($data_major)")
[[ "$library_version" == "$header_library_version" ]] ||
  mismatches+=("runtime-library($library_version)!=header-library($header_library_version)")
[[ "$data_version" == "$header_data_version" ]] ||
  mismatches+=("runtime-data($data_version)!=header-data($header_data_version)")
[[ "$data_name_major" == "$data_major" ]] ||
  mismatches+=("data-filename($data_name_major)!=data($data_major)")

if ((${#mismatches[@]} != 0)); then
  printf 'icu-coherence.result=mismatch:%s\n' "$(IFS=,; echo "${mismatches[*]}")" >&2
  exit 2
fi

echo 'icu-coherence.result=coherent'
