#!/bin/bash
set -euo pipefail

script_dir="$(cd "$(dirname "$0")" && pwd)"
project_root="$(cd "$script_dir/.." && pwd)"
source_root="$project_root/_aosp/art/openjdkjvmti"
build_root="$project_root/_build/openjdkjvmti-darwin"
object_root="$build_root/objects"
output="$build_root/libopenjdkjvmti-darwin.a"
temporary_output="$build_root/libopenjdkjvmti-darwin.a.tmp.$$"
patch_file="$project_root/patches/openjdkjvmti/0001-darwin-monotonic-jvmti-time.patch"
allocator_patch_file="$project_root/patches/openjdkjvmti/0002-darwin-malloc-size.patch"
search_patch_file="$project_root/patches/openjdkjvmti/0003-darwin-in-memory-dex-file.patch"
search_classes_patch_file="$project_root/patches/openjdkjvmti/0004-darwin-search-lazy-system-classes.patch"
runtime_shadow="$project_root/_build/runtime-common/patched-source"
runtime_shadow_identity="$runtime_shadow/.darwin-art-shadow-identity"
[[ -s "$runtime_shadow_identity" &&
   -f "$runtime_shadow/runtime/gc/allocator_type.h" &&
   -f "$runtime_shadow/runtime/jit/jit_options.h" ]] || {
  echo 'openjdkjvmti-darwin: complete canonical ART shadow missing; run prepare-runtime-common-shadow first' >&2
  exit 2
}

# shellcheck disable=SC1090
source "$project_root/upstream/android16-openjdkjvmti.lock"

"$project_root/tools/sync-android16-openjdkjvmti.sh"
mkdir -p "$object_root"
[[ "$(shasum -a 256 "$patch_file" | awk '{print $1}')" == \
   "$DARWIN_MONOTONIC_TIMER_PATCH_SHA256" ]] || {
  echo "openjdkjvmti-darwin: timer patch checksum mismatch" >&2
  exit 3
}
[[ "$(shasum -a 256 "$allocator_patch_file" | awk '{print $1}')" == \
   "$DARWIN_MALLOC_SIZE_PATCH_SHA256" ]] || {
  echo "openjdkjvmti-darwin: allocator patch checksum mismatch" >&2
  exit 4
}
[[ "$(shasum -a 256 "$search_patch_file" | awk '{print $1}')" == \
   "$DARWIN_IN_MEMORY_DEX_PATCH_SHA256" ]] || {
  echo "openjdkjvmti-darwin: in-memory DEX patch checksum mismatch" >&2
  exit 5
}
patched_source_root="$build_root/patched-source"
patched_identity="$(shasum -a 256 "$patch_file" "$allocator_patch_file" \
  "$search_patch_file" "$search_classes_patch_file" \
  "$source_root/ti_timers.cc" "$source_root/ti_allocator.cc" \
  "$source_root/ti_search.cc" | shasum -a 256 | awk '{print $1}')"
if [[ ! -f "$patched_source_root/.darwin-art-identity" ]] ||
   [[ "$(< "$patched_source_root/.darwin-art-identity")" != "$patched_identity" ]]; then
rm -rf "$patched_source_root"
mkdir -p "$patched_source_root"
cp "$source_root/ti_timers.cc" "$patched_source_root/ti_timers.cc"
cp "$source_root/ti_allocator.cc" "$patched_source_root/ti_allocator.cc"
cp "$source_root/ti_search.cc" "$patched_source_root/ti_search.cc"
patch --batch --forward -p1 -d "$patched_source_root" < "$patch_file" >/dev/null
patch --batch --forward -p1 -d "$patched_source_root" < "$allocator_patch_file" >/dev/null
patch --batch --forward -p1 -d "$patched_source_root" < "$search_patch_file" >/dev/null
patch --batch --forward -p1 -d "$patched_source_root" < "$search_classes_patch_file" >/dev/null
printf '%s\n' "$patched_identity" > "$patched_source_root/.darwin-art-identity"
fi

sources=(
  alloc_manager.cc deopt_manager.cc events.cc object_tagging.cc OpenjdkJvmTi.cc
  ti_allocator.cc ti_breakpoint.cc ti_class.cc ti_class_definition.cc
  ti_class_loader.cc ti_ddms.cc ti_dump.cc ti_extension.cc ti_field.cc
  ti_heap.cc ti_jni.cc ti_logging.cc ti_method.cc ti_monitor.cc ti_object.cc
  ti_phase.cc ti_properties.cc ti_search.cc ti_stack.cc ti_redefine.cc
  ti_thread.cc ti_threadgroup.cc ti_timers.cc transform.cc
)

common_flags=(
  -std=c++20 -O2 -fPIC -pthread
  -Wno-invalid-offsetof -Wno-unsupported-visibility
  -DART_PAGE_SIZE_AGNOSTIC -DBUILDING_LIBART -DNDEBUG
  -DART_DEFAULT_GC_TYPE_IS_CMS -DART_USE_READ_BARRIER
  -DART_READ_BARRIER_TYPE_IS_BAKER=1 -DART_FORCE_USE_READ_BARRIER
  -DART_STACK_OVERFLOW_GAP_arm=8192
  -DART_STACK_OVERFLOW_GAP_arm64=8192
  -DART_STACK_OVERFLOW_GAP_riscv64=8192
  -DART_STACK_OVERFLOW_GAP_x86=8192
  -DART_STACK_OVERFLOW_GAP_x86_64=8192
  -include base/globals.h
  -include "$project_root/_build/runtime-common/patched-source/runtime/mirror/object_reference.h"
  -I "$source_root/include"
  -I "$source_root"
  -I "$project_root/_build/runtime-arm64/generated"
  -I "$project_root/_build/runtime-common/patched-source/runtime"
  -I "$project_root/_build/runtime-common/patched-source/runtime/oat"
  -I "$project_root/_aosp/art/runtime/oat"
  -I "$project_root/_build/foundation/patched-source/libartbase"
  -I "$project_root/_build/foundation/patched-source/libartbase/base"
  -I "$project_root/_aosp/art/libartbase"
  -I "$project_root/_aosp/art/libartbase/base"
  -I "$project_root/_aosp/art/runtime"
  -I "$project_root/_aosp/art/runtime/base"
  -I "$project_root/_aosp/art/runtime/arch/arm64"
  -I "$project_root/_aosp/art/cmdline"
  -I "$project_root/_aosp/art/libdexfile"
  -I "$project_root/_aosp/art/libnativebridge/include"
  -I "$project_root/_aosp/system/libbase/include"
  -I "$project_root/_aosp/system/logging/liblog/include"
  -I "$project_root/_aosp/external/fmtlib/include"
  -I "$project_root/_aosp/external/tinyxml2"
  -I "$project_root/_aosp/external/dlmalloc"
  -I "$project_root/_aosp/libnativehelper/include_jni"
  -I "$project_root/_aosp/libnativehelper/header_only_include"
  -I "$project_root/_aosp/libnativehelper-full/include"
)

objects=()
for relative in "${sources[@]}"; do
  source="$source_root/$relative"
  if [[ "$relative" == ti_timers.cc || "$relative" == ti_allocator.cc ||
        "$relative" == ti_search.cc ]]; then
    source="$patched_source_root/$relative"
  fi
  object="$object_root/${relative%.cc}.o"
  objects+=("$object")
  if [[ ! -f "$object" || "$source" -nt "$object" ||
        "$0" -nt "$object" ||
        "$runtime_shadow_identity" -nt "$object" ||
        "$source_root/.darwin-art-identity" -nt "$object" ]]; then
    clang++ "${common_flags[@]}" -c "$source" -o "$object"
  fi
done

rm -f "$temporary_output"
trap 'rm -f "$temporary_output"' EXIT
ar rcs "$temporary_output" "${objects[@]}"
ranlib "$temporary_output"
if [[ ! -f "$output" ]] || ! cmp -s "$temporary_output" "$output"; then
  mv "$temporary_output" "$output"
else
  rm -f "$temporary_output"
fi
file "$output" | grep -q 'current ar archive'
nm -gU "$output" > "$build_root/exports.txt.tmp"
if [[ ! -f "$build_root/exports.txt" ]] || \
   ! cmp -s "$build_root/exports.txt.tmp" "$build_root/exports.txt"; then
  mv "$build_root/exports.txt.tmp" "$build_root/exports.txt"
else
  rm -f "$build_root/exports.txt.tmp"
fi
grep -q ' _ArtPlugin_Initialize$' "$build_root/exports.txt"
grep -q ' _ArtPlugin_Deinitialize$' "$build_root/exports.txt"
echo "openjdkjvmti-darwin: sources=${#sources[@]} output=$output"
