#!/bin/bash
set -euo pipefail

script_dir="$(cd "$(dirname "$0")" && pwd)"
project_root="$(cd "$script_dir/.." && pwd)"
lock_file="$project_root/upstream/android16-icu-runtime-adapters.lock"
foundation_lock="$project_root/upstream/android16-icu-foundation.lock"
icu_root="$project_root/_aosp/external/icu-graphics"
foundation_dir="$project_root/_build/icu-foundation"
build_dir="$project_root/_build/icu-runtime-adapters"

# shellcheck disable=SC1090
source "$lock_file"

fail_gate() {
  echo "icu-runtime-adapters: $1" >&2
  exit 2
}

verify_sha() {
  local file="$1" expected="$2" actual
  [[ -f "$file" ]] || fail_gate "missing pinned input $file"
  actual="$(shasum -a 256 "$file" | awk '{print $1}')"
  [[ "$actual" == "$expected" ]] ||
    fail_gate "checksum mismatch file=$file expected=$expected actual=$actual"
}

grep -Fx "ICU_REVISION=$ICU_REVISION" "$foundation_lock" >/dev/null ||
  fail_gate "ICU foundation revision does not match adapter lock"
verify_sha "$icu_root/icu4c/source/common/unicode/uvernum.h" "$ICU_UVERNUM_SHA256"
verify_sha "$icu_root/icu4c/source/common/unicode/urename.h" "$ICU_URENAME_SHA256"
verify_sha "$icu_root/android_icu4c/include/uconfig_local.h" "$ICU_UCONFIG_LOCAL_SHA256"
verify_sha "$project_root/compat/darwin_icu_natives.cc" "$DARWIN_ICU_NATIVES_CC_SHA256"
verify_sha "$project_root/compat/darwin_icu_natives.h" "$DARWIN_ICU_NATIVES_H_SHA256"
verify_sha "$project_root/compat/darwin_libcore_natives.cc" "$DARWIN_LIBCORE_NATIVES_CC_SHA256"
verify_sha "$project_root/compat/darwin_libcore_natives.h" "$DARWIN_LIBCORE_NATIVES_H_SHA256"
verify_sha "$project_root/compat/darwin_libcore_unicode_natives.cc" "$DARWIN_LIBCORE_UNICODE_NATIVES_CC_SHA256"

common_archive="$foundation_dir/libicuuc-common-darwin.a"
i18n_archive="$foundation_dir/libicui18n-darwin.a"
stubdata_archive="$foundation_dir/libicuuc-stubdata-darwin.a"
init_archive="$foundation_dir/libandroidicuinit-darwin.a"
zlib_archive="$project_root/_build/graphics-codecs/libz-darwin.a"
archives=("$common_archive" "$i18n_archive" "$stubdata_archive" "$init_archive")
counts=("$ICU_COMMON_MEMBER_COUNT" "$ICU_I18N_MEMBER_COUNT" \
        "$ICU_STUBDATA_MEMBER_COUNT" "$ICU_INIT_MEMBER_COUNT")
ar="$(xcrun --find ar)"
for ((index = 0; index < ${#archives[@]}; ++index)); do
  archive="${archives[$index]}"
  [[ -f "$archive" ]] || fail_gate "missing ICU foundation archive $archive"
  [[ "$(lipo -archs "$archive")" == arm64 ]] || fail_gate "not arm64: $archive"
  members="$("$ar" -t "$archive" | grep -v '^__\.SYMDEF' | wc -l | tr -d ' ')"
  [[ "$members" == "${counts[$index]}" ]] ||
    fail_gate "archive member count mismatch file=$archive expected=${counts[$index]} actual=$members"
done
[[ -f "$zlib_archive" && "$(lipo -archs "$zlib_archive")" == arm64 ]] ||
  fail_gate "Android zlib provider archive is missing or not arm64: $zlib_archive"

source_data="$icu_root/icu4c/source/stubdata/$ICU_DATA_FILE"
verify_sha "$source_data" "$ICU_DATA_SHA256"
[[ "$(stat -f '%z' "$source_data")" == "$ICU_DATA_SIZE" ]] ||
  fail_gate "ICU runtime data size mismatch"

cxx="$(command -v clang++ || true)"
[[ -n "$cxx" ]] || fail_gate "clang++ is required"
sdk_root="$(xcrun --sdk macosx --show-sdk-path)"
mkdir -p "$build_dir"
stage="$(mktemp -d "$build_dir/stage.XXXXXX")"
trap 'rm -rf "$stage"' EXIT

flags=(
  -arch arm64 -isysroot "$sdk_root" -std=c++20 -O2 -fPIC
  -fvisibility=hidden -Wall -Wextra -Werror -Wno-unused-parameter
  -DANDROID
  -I"$project_root/compat"
  -I"$project_root/_aosp/libnativehelper/include_jni"
  -I"$icu_root/android_icu4c/include"
  -I"$icu_root/icu4c/source/common"
  -I"$icu_root/icu4c/source/i18n"
  -I"$icu_root/libandroidicuinit/include"
  -I"$project_root/_aosp/external/zlib"
)

adapter_objects=()
for source in darwin_icu_natives.cc darwin_libcore_natives.cc \
             darwin_libcore_unicode_natives.cc; do
  object="$stage/${source%.cc}.o"
  echo "icu-runtime-adapters: compile $source"
  "$cxx" "${flags[@]}" -c "$project_root/compat/$source" -o "$object"
  file "$object" | grep -F 'Mach-O 64-bit object arm64' >/dev/null ||
    fail_gate "adapter object is not Mach-O arm64: $object"
  adapter_objects+=("$object")
done

icu76_symbols="$(nm -u "${adapter_objects[@]}" | awk '$1 ~ /^_/ {print $1}' | \
  grep '_76$' | sort -u)"
icu76_count="$(printf '%s\n' "$icu76_symbols" | sed '/^$/d' | wc -l | tr -d ' ')"
icu76_manifest_sha="$(printf '%s\n' "$icu76_symbols" | shasum -a 256 | awk '{print $1}')"
[[ "$icu76_count" == "$ADAPTER_ICU76_UNDEFINED_COUNT" &&
   "$icu76_manifest_sha" == "$ADAPTER_ICU76_UNDEFINED_MANIFEST_SHA256" ]] ||
  fail_gate "ICU76 adapter closure mismatch count=$icu76_count sha=$icu76_manifest_sha"
if nm "${adapter_objects[@]}" | grep -E '_78($|[^0-9])' >/dev/null; then
  fail_gate "adapter object contains ICU78 symbols"
fi

probe_object="$stage/android16_icu_runtime_adapter_smoke.o"
"$cxx" "${flags[@]}" -c \
  "$project_root/probes/android16_icu_runtime_adapter_smoke.cc" -o "$probe_object"
framework_stub_object="$stage/android16_icu_runtime_adapter_framework_stub.o"
"$cxx" "${flags[@]}" -c \
  "$project_root/probes/android16_icu_runtime_adapter_framework_stub.cc" \
  -o "$framework_stub_object"
executable="$stage/android16-icu-runtime-adapter-smoke"
link_map="$stage/android16-icu-runtime-adapter-smoke.map"
"$cxx" -arch arm64 -isysroot "$sdk_root" "$probe_object" \
  "$framework_stub_object" \
  "${adapter_objects[@]}" "$i18n_archive" "$common_archive" \
  -Wl,-force_load,"$init_archive" "$stubdata_archive" "$zlib_archive" \
  -Wl,-map,"$link_map" -o "$executable"

[[ "$(lipo -archs "$executable")" == arm64 ]] ||
  fail_gate "smoke executable is not arm64"
if nm "$executable" | grep -E '_78($|[^0-9])' >/dev/null; then
  fail_gate "linked executable contains ICU78 symbols"
fi
definitions="$(nm -gU "$executable")"
for symbol in _u_getVersion_76 _ucnv_open_76 _u_charName_76 \
              __Z16android_icu_initv _icudt76_dat; do
  grep -F "$symbol" <<<"$definitions" >/dev/null ||
    fail_gate "linked executable lacks $symbol"
done
dependencies="$(otool -L "$executable")"
if grep -E '/opt/homebrew|/usr/local|libicu(uc|i18n|data)' <<<"$dependencies" >/dev/null; then
  echo "$dependencies" >&2
  fail_gate "forbidden host ICU dependency"
fi
for consumed_archive in "$common_archive" "$i18n_archive" \
                        "$stubdata_archive" "$init_archive" "$zlib_archive"; do
  grep -F "$consumed_archive(" "$link_map" >/dev/null ||
    fail_gate "link map did not consume $consumed_archive"
done
if grep -E '/opt/homebrew|/usr/local|libicu(uc|i18n|data)[^[:space:]]*\.dylib' "$link_map" >/dev/null; then
  fail_gate "forbidden host ICU provider in link map"
fi

runtime="$stage/runtime"
mkdir -p "$runtime/i18n/etc/icu" "$runtime/data" "$runtime/tzdata"
cp "$source_data" "$runtime/i18n/etc/icu/$ICU_DATA_FILE"
verify_sha "$runtime/i18n/etc/icu/$ICU_DATA_FILE" "$ICU_DATA_SHA256"

# Android 16 keeps the Olson database and ICU timezone resources in the
# timezone APEX, separate from the i18n APEX / ICU common data.  The managed
# TimeZoneDataFiles owner selects format-major 9, so preserve that exact APEX
# layout under ANDROID_TZDATA_ROOT instead of falling back to host zoneinfo.
android_sdk="${ANDROID_SDK_ROOT:-${ANDROID_HOME:-$HOME/Library/Android/sdk}}"
if [[ -n "${AIM_ANDROID16_SYSTEM_IMAGE:-}" ]]; then
  system_image="$AIM_ANDROID16_SYSTEM_IMAGE"
else
  system_image=""
  for candidate in \
    "$android_sdk/system-images/android-36/google_apis_playstore/arm64-v8a/system.img" \
    "$android_sdk/system-images/android-36/google_apis_playstore_ps16k/arm64-v8a/system.img"; do
    if [[ -f "$candidate" ]]; then
      system_image="$candidate"
      break
    fi
  done
fi
[[ -f "$system_image" ]] ||
  fail_gate "Android 16 system image is required for timezone APEX data"
timezone_apex="$stage/com.google.android.tzdata6.apex"
cargo run --quiet --release \
  --manifest-path "$project_root/tools/super-i18n-apex-extract/Cargo.toml" -- \
  "$system_image" "$timezone_apex" com.google.android.tzdata6.apex >/dev/null
timezone_files=(
  tz_version tzdata telephonylookup.xml tzlookup.xml
  icu/metaZones.res icu/timezoneTypes.res icu/windowsZones.res icu/zoneinfo64.res
)
for relative in "${timezone_files[@]}"; do
  destination="$runtime/tzdata/etc/tz/versioned/9/$relative"
  mkdir -p "$(dirname "$destination")"
  cargo run --quiet \
    --manifest-path "$project_root/tools/apex-ext2-extract/Cargo.toml" -- \
    "$timezone_apex" "$destination" "/etc/tz/versioned/9/$relative" >/dev/null
  [[ -s "$destination" ]] || fail_gate "empty timezone APEX file: $relative"
done
[[ "$(stat -f '%z' "$runtime/tzdata/etc/tz/versioned/9/tz_version")" == 17 ]] ||
  fail_gate "unexpected Android 16 tz_version size"
[[ "$(head -c 6 "$runtime/tzdata/etc/tz/versioned/9/tzdata")" == "tzdata" ]] ||
  fail_gate "unexpected Android 16 tzdata magic"

output="$(ANDROID_DATA="$runtime/data" ANDROID_TZDATA_ROOT="$runtime/tzdata" \
  ANDROID_I18N_ROOT="$runtime/i18n" "$executable")"
[[ "$output" == "$EXPECTED_SMOKE_PREFIX"* && "$output" == *' data=76.'* ]] ||
  fail_gate "unexpected smoke output: $output"

mv "$executable" "$build_dir/android16-icu-runtime-adapter-smoke"
for object in "${adapter_objects[@]}"; do
  mv "$object" "$build_dir/$(basename "$object")"
done
rm -rf "$build_dir/runtime"
mv "$runtime" "$build_dir/runtime"

echo "icu-runtime-adapters: $output"
echo "icu-runtime-adapters: icu76-closure=$icu76_count icu78=0"
echo "icu-runtime-adapters: executable=$build_dir/android16-icu-runtime-adapter-smoke"
