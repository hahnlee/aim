#!/bin/bash
set -euo pipefail

script_dir="$(cd "$(dirname "$0")" && pwd)"
project_root="$(cd "$script_dir/.." && pwd)"
aosp_root="$project_root/_aosp"
lock_file="$project_root/upstream/android16-hwui.lock"
output_dir="$project_root/_build/ui-types-foundation"
object_dir="$output_dir/objects"

lock_value() {
  local key="$1"
  awk -F= -v key="$key" \
    '$1 == key { print substr($0, index($0, "=") + 1); exit }' "$lock_file"
}

require_locked_revision() {
  local project="$1"
  local key="$2"
  local expected="$3"
  local actual
  actual="$(lock_value "$key")"
  if [[ "$actual" != "$expected" ]]; then
    echo "ui-types-foundation: unsupported source lock" >&2
    echo "  project:           $project" >&2
    echo "  lock key:          $key" >&2
    echo "  expected revision: $expected" >&2
    echo "  actual revision:   ${actual:-<missing>}" >&2
    exit 2
  fi
}

verify_file() {
  local project="$1"
  local revision="$2"
  local subtree="$3"
  local file="$4"
  local expected="$5"
  if [[ ! -f "$file" ]]; then
    echo "ui-types-foundation: required revision-locked source is missing" >&2
    echo "  project/subtree: $project/$subtree" >&2
    echo "  revision:        $revision" >&2
    echo "  file:            $file" >&2
    echo "  expected sha256: $expected" >&2
    exit 2
  fi

  local actual
  actual="$(shasum -a 256 "$file" | awk '{print $1}')"
  if [[ "$actual" != "$expected" ]]; then
    echo "ui-types-foundation: revision-locked source checksum mismatch" >&2
    echo "  project/subtree: $project/$subtree" >&2
    echo "  revision:        $revision" >&2
    echo "  file:            $file" >&2
    echo "  expected sha256: $expected" >&2
    echo "  actual sha256:   $actual" >&2
    exit 2
  fi
}

verify_materialized_revision() {
  local project="$1"
  local revision="$2"
  local source_root="$3"
  local marker="$source_root/.source-revision"
  if [[ ! -f "$marker" || "$(<"$marker")" != "$revision" ]]; then
    echo "ui-types-foundation: dependency source revision mismatch" >&2
    echo "  project:           $project" >&2
    echo "  source root:       $source_root" >&2
    echo "  expected revision: $revision" >&2
    if [[ -f "$marker" ]]; then
      echo "  actual revision:   $(<"$marker")" >&2
    else
      echo "  actual revision:   <missing .source-revision>" >&2
    fi
    exit 2
  fi
}

frameworks_native_revision="2827a4a16b0340ecd07c2d5a6c89991799b362bb"
system_core_revision="68be0c2c0006a0740d0b1809abe4717308f90d15"
system_logging_revision="d78b713380007d3c0dde14712cbcbec27f491ad9"
libbase_revision="0ac7ea4a65d981166d6e5617e1e4ef0572f774e5"

require_locked_revision platform/frameworks/native FRAMEWORKS_NATIVE_REVISION \
  "$frameworks_native_revision"
require_locked_revision platform/system/core SYSTEM_CORE_REVISION \
  "$system_core_revision"
require_locked_revision platform/system/logging SYSTEM_LOGGING_REVISION \
  "$system_logging_revision"
require_locked_revision platform/system/libbase LIBBASE_REVISION \
  "$libbase_revision"

ui_root="$aosp_root/frameworks/native/libs/ui"
arect_root="$aosp_root/frameworks/native/libs/arect"
math_root="$aosp_root/frameworks/native/libs/math"
libbase_root="$aosp_root/system/libbase"
libutils_root="$aosp_root/system/core/libutils"
libcutils_root="$aosp_root/system/core/libcutils"
libsystem_root="$aosp_root/system/core/libsystem"
liblog_root="$aosp_root/system/logging/liblog"

# The module manifest and all four entries in libui-types.srcs are content
# bound to android-16.0.0_r1. This is also an explicit guard against silently
# turning this gate into a hand-selected subset when Android.bp changes.
verify_file platform/frameworks/native "$frameworks_native_revision" libs/ui \
  "$ui_root/Android.bp" \
  db6729e7d6e253fac3f942d58185e392b70ad810b7d41a1e60a398c59d3ed31d
verify_file platform/frameworks/native "$frameworks_native_revision" libs/ui \
  "$ui_root/ColorSpace.cpp" \
  785671584331a91a04b2dd52dd700911b6d0b2b7d342947f53849f2a11ba8bbf
verify_file platform/frameworks/native "$frameworks_native_revision" libs/ui \
  "$ui_root/Rect.cpp" \
  a4a75fc5885672247f7910fec0b2f04f8d2c1077da101244578483c9ee0003d1
verify_file platform/frameworks/native "$frameworks_native_revision" libs/ui \
  "$ui_root/Region.cpp" \
  2b502c2d08b306097afd99f45061b00b20b28bcd147bc05c709ed02238e8a94d
verify_file platform/frameworks/native "$frameworks_native_revision" libs/ui \
  "$ui_root/Transform.cpp" \
  eeec64fb96371106c98bd37b125237f269b382865446ee3630da7c364c586b89

# libarect and libmath are the two exported static dependencies in Android.bp.
# Both are header-only archives for this variant, so their manifests and
# exported include trees are inputs; no replacement implementation is made.
verify_file platform/frameworks/native "$frameworks_native_revision" libs/arect \
  "$arect_root/Android.bp" \
  47907bae3bf3b5aac8b70c27f4e38c4fe9fa6905fb3ef43898a04a5ba0924189
verify_file platform/frameworks/native "$frameworks_native_revision" libs/math \
  "$math_root/Android.bp" \
  20812ce8a2865b488f989915f9dd61719b99d8d4eaca4aadc3bdba1515f75334

verify_materialized_revision platform/frameworks/native \
  "$frameworks_native_revision" "$ui_root"
verify_materialized_revision platform/frameworks/native \
  "$frameworks_native_revision" "$arect_root"
verify_materialized_revision platform/frameworks/native \
  "$frameworks_native_revision" "$math_root"

verify_materialized_revision platform/system/libbase "$libbase_revision" \
  "$libbase_root"
verify_materialized_revision platform/system/core "$system_core_revision" \
  "$libutils_root"
verify_materialized_revision platform/system/core "$system_core_revision" \
  "$libcutils_root"
verify_materialized_revision platform/system/core "$system_core_revision" \
  "$libsystem_root"
verify_materialized_revision platform/system/logging "$system_logging_revision" \
  "$liblog_root"

required_include_dirs=(
  "$ui_root/include"
  "$ui_root/include_mock"
  "$ui_root/include_private"
  "$ui_root/include_types"
  "$arect_root/include"
  "$math_root/include"
  "$libbase_root/include"
  "$libutils_root/include"
  "$libcutils_root/include"
  "$libsystem_root/include"
  "$liblog_root/include"
)
for include_dir in "${required_include_dirs[@]}"; do
  if [[ ! -d "$include_dir" ]]; then
    echo "ui-types-foundation: required exported include tree is missing" >&2
    echo "  framework revision: $frameworks_native_revision" >&2
    echo "  include tree:       $include_dir" >&2
    exit 2
  fi
done

cxx="$(command -v clang++ || true)"
if [[ -z "$cxx" ]]; then
  echo "ui-types-foundation: clang++ is required" >&2
  exit 2
fi
libtool_bin="$(xcrun --find libtool)"

common_flags=(
  -std=gnu++20
  -arch arm64
  -O2
  -fPIC
  -fno-rtti
  -fvisibility=hidden
  -Wall
  -Werror
  -Wextra
  # Apple Clang diagnoses two unchanged AOSP libmath header constructs that
  # Android's pinned Clang accepts. These do not alter the libui-types module
  # flags or ABI; they are narrowly scoped host-toolchain compatibility flags.
  -Wno-deprecated-literal-operator
  -Wno-invalid-specialization
  -I"$ui_root/include"
  -I"$ui_root/include_mock"
  -I"$ui_root/include_private"
  -I"$ui_root/include_types"
  -I"$arect_root/include"
  -I"$math_root/include"
  -I"$libbase_root/include"
  -I"$libutils_root/include"
  -I"$libcutils_root/include"
  -I"$libsystem_root/include"
  -I"$liblog_root/include"
)

# Complete libui-types.srcs from the checksum-locked Android.bp above.
sources=(
  ColorSpace.cpp
  Rect.cpp
  Region.cpp
  Transform.cpp
)

mkdir -p "$object_dir"
objects=()
for source in "${sources[@]}"; do
  object="$object_dir/${source%.cpp}.o"
  echo "ui-types-foundation: compile $source"
  "$cxx" "${common_flags[@]}" -c "$ui_root/$source" -o "$object"
  objects+=("$object")
done

archive="$output_dir/libui-types.a"
"$libtool_bin" -static -o "$archive" "${objects[@]}"

for object in "${objects[@]}"; do
  kind="$(file "$object")"
  if [[ "$kind" != *"Mach-O 64-bit object arm64"* ]]; then
    echo "ui-types-foundation: unexpected object format: $kind" >&2
    exit 3
  fi
done
file "$archive" | grep -F 'current ar archive' >/dev/null
lipo -info "$archive" | grep -F 'architecture: arm64' >/dev/null

expected_members="$(printf '%s\n' ColorSpace.o Rect.o Region.o Transform.o | sort)"
actual_members="$(ar -t "$archive" | grep -v '^__.SYMDEF' | sort)"
if [[ "$actual_members" != "$expected_members" ]]; then
  echo "ui-types-foundation: archive member set does not match Android.bp srcs" >&2
  diff -u <(printf '%s\n' "$expected_members") \
    <(printf '%s\n' "$actual_members") >&2 || true
  exit 3
fi

definitions="$(nm -gUC "$archive")"
representative_definitions=(
  'android::Rect::makeInvalid()'
  'android::Region::set(android::Rect const&)'
  'android::ui::Transform::inverse() const'
  'android::ColorSpace::sRGB()'
)
for definition in "${representative_definitions[@]}"; do
  if ! grep -F " T $definition" <<<"$definitions" >/dev/null; then
    echo "ui-types-foundation: missing representative definition" >&2
    echo "  definition: $definition" >&2
    echo "  archive:    $archive" >&2
    exit 3
  fi
done

# A static foundation archive is expected to retain its libbase/libutils
# dependency closure. Count it without satisfying any symbol through a local
# shim; the next module-level link gate owns that closure.
undefined_count="$(nm -u "$archive" | awk '$1 ~ /^_/ { print $1 }' | sort -u | wc -l | tr -d ' ')"

echo "ui-types-foundation: objects=${#objects[@]} architecture=arm64"
echo "ui-types-foundation: representative-definitions=${#representative_definitions[@]}"
echo "ui-types-foundation: undefined-static-closure=$undefined_count"
echo "ui-types-foundation: archive=$archive"
