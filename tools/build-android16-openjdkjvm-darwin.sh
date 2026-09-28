#!/bin/bash
set -euo pipefail
export LC_ALL=C

script_dir="$(cd "$(dirname "$0")" && pwd)"
project_root="$(cd "$script_dir/.." && pwd)"
lock_file="$project_root/upstream/android16-openjdkjvm-darwin.lock"
source_root="$project_root/_aosp/art-openjdkjvm"
build_dir="$project_root/_build/openjdkjvm-darwin"
patch_file="$project_root/patches/art-openjdkjvm/0001-darwin-jvm-last-error-string.patch"

# shellcheck disable=SC1090
source "$lock_file"

sha256() { shasum -a 256 "$1" | awk '{print $1}'; }
fail() { echo "openjdkjvm: $*" >&2; exit 3; }

materialize() {
  local project="$1" revision="$2" relative="$3" expected="$4"
  local destination="$source_root/$relative"
  if [[ -f "$destination" ]]; then
    [[ "$(sha256 "$destination")" == "$expected" ]] ||
      fail "checksum mismatch: $destination"
    return
  fi
  mkdir -p "$(dirname "$destination")"
  local staged
  staged="$(mktemp "${destination}.download.XXXXXX")"
  curl -fsSL \
    "https://android.googlesource.com/$project/+/$revision/${relative#*/}?format=TEXT" \
    | base64 -D > "$staged"
  [[ "$(sha256 "$staged")" == "$expected" ]] || {
    fail "download checksum mismatch: $relative"
  }
  mv "$staged" "$destination"
}

materialize "$ART_PROJECT" "$ART_REVISION" \
  art/openjdkjvm/Android.bp "$OPENJDKJVM_ANDROID_BP_SHA256"
materialize "$ART_PROJECT" "$ART_REVISION" \
  art/openjdkjvm/OpenjdkJvm.cc "$OPENJDKJVM_CC_SHA256"
materialize "$LIBCORE_PROJECT" "$LIBCORE_REVISION" \
  libcore/ojluni/src/main/native/jvm.h "$JVM_H_SHA256"
materialize "$LIBCORE_PROJECT" "$LIBCORE_REVISION" \
  libcore/ojluni/src/main/native/jvm_md.h "$JVM_MD_H_SHA256"
materialize "$LIBCORE_PROJECT" "$LIBCORE_REVISION" \
  libcore/ojluni/src/main/native/classfile_constants.h \
  "$CLASSFILE_CONSTANTS_H_SHA256"

python3 - "$source_root/art/openjdkjvm/Android.bp" <<'PY'
import re
import sys
from pathlib import Path

text = Path(sys.argv[1]).read_text()
body = text[text.index('name: "libopenjdkjvm_defaults"'):]
sources = re.findall(r'srcs:\s*\[([^]]+)\]', body, re.S)
assert sources, "libopenjdkjvm_defaults srcs missing"
files = re.findall(r'"([^"]+\.(?:cc|cpp|c))"', sources[0])
assert files == ["OpenjdkJvm.cc"], files
assert 'host_supported: true' in body
assert '"libbase"' in body
assert '"libnativehelper_header_only"' in body
assert 'name: "libopenjdkjvm"' in text
assert '"libart"' in text and '"libartbase"' in text
PY

for required in \
  "$patch_file" \
  "$project_root/probes/openjdkjvm_last_error_smoke.cc" \
  "$project_root/_build/runtime-common/patched-source/runtime/runtime.h" \
  "$project_root/_build/runtime-core/patched-source/runtime/mirror/object_reference.h" \
  "$project_root/_build/foundation/patched-source/libartbase/base/globals.h" \
  "$project_root/_aosp/art/runtime" \
  "$project_root/_aosp/art/libartbase" \
  "$project_root/_aosp/system/libbase/include/android-base/logging.h" \
  "$project_root/_aosp/libnativehelper-full/header_only_include/nativehelper/scoped_local_ref.h" \
  "$project_root/_aosp/libnativehelper-full/include_jni/jni.h"; do
  [[ -e "$required" ]] || fail "missing build dependency: $required"
done
[[ "$(sha256 "$patch_file")" == "$DARWIN_PATCH_SHA256" ]] ||
  fail "Darwin patch checksum mismatch"

stage="$(mktemp -d "${TMPDIR:-/tmp}/aim-openjdkjvm.XXXXXX")"
trap 'rm -rf "$stage"' EXIT
patched_root="$stage/source"
mkdir -p "$patched_root/art/openjdkjvm" \
  "$patched_root/libcore/ojluni/src/main/native"
cp "$source_root/art/openjdkjvm/OpenjdkJvm.cc" \
  "$patched_root/art/openjdkjvm/OpenjdkJvm.cc"
cp "$source_root/art/openjdkjvm/Android.bp" \
  "$patched_root/art/openjdkjvm/Android.bp"
cp "$source_root/libcore/ojluni/src/main/native/"*.h \
  "$patched_root/libcore/ojluni/src/main/native/"
patch --batch --forward -p1 -d "$patched_root/art" < "$patch_file" >/dev/null
[[ "$(sha256 "$patched_root/art/openjdkjvm/OpenjdkJvm.cc")" == \
   "$PATCHED_OPENJDKJVM_CC_SHA256" ]] || fail "patched source checksum mismatch"
grep -F '#if defined(__APPLE__)' \
  "$patched_root/art/openjdkjvm/OpenjdkJvm.cc" >/dev/null ||
  fail "Darwin XSI strerror_r patch not applied"
source_exports="$stage/openjdkjvm-source-exports.txt"
python3 - "$patched_root/art/openjdkjvm/OpenjdkJvm.cc" > "$source_exports" <<'PY'
import re
import sys
from pathlib import Path

text = Path(sys.argv[1]).read_text()
names = sorted(set(re.findall(
    r'\b((?:JVM_|jio_)[A-Za-z0-9_]+)\s*\([^;{}]*\)\s*\{', text, re.S)))
print(*names, sep="\n")
PY

ndk_include=""
for candidate in "$HOME"/Library/Android/sdk/ndk/*/toolchains/llvm/prebuilt/*/sysroot/usr/include; do
  if [[ -f "$candidate/elf.h" && -d "$candidate/aarch64-linux-android" ]]; then
    ndk_include="$candidate"
  fi
done
[[ -n "$ndk_include" ]] || fail "Android NDK headers are required"

cc="$(xcrun --find clang++)"
libtool_bin="$(xcrun --find libtool)"
sdk_root="$(xcrun --sdk macosx --show-sdk-path)"
object="$stage/OpenjdkJvm.cc.o"
includes=(
  "$project_root/_build/runtime-common/patched-source/runtime"
  "$project_root/_build/runtime-core/patched-source/runtime"
  "$project_root/_build/foundation/patched-source/libartbase"
  "$project_root/_build/foundation/patched-source/libartbase/base"
  "$project_root/_aosp/art/libartbase"
  "$project_root/_aosp/art/libartbase/base"
  "$project_root/_aosp/art/runtime"
  "$project_root/_aosp/art/runtime/base"
  "$project_root/_aosp/art/runtime/arch/arm64"
  "$project_root/_aosp/art/libdexfile"
  "$project_root/_aosp/art/libprofile"
  "$project_root/_aosp/art/libartpalette/include"
  "$project_root/_aosp/system/libbase/include"
  "$project_root/_aosp/libnativehelper-full/include_jni"
  "$project_root/_aosp/libnativehelper-full/header_only_include"
  "$project_root/_aosp/libnativehelper-full/include_platform_header_only"
  "$project_root/_aosp/libnativehelper-full/include_platform"
  "$project_root/_aosp/external/fmtlib/include"
  "$project_root/_aosp/external/tinyxml2"
  "$project_root/_aosp/external/dlmalloc"
)
compile=(
  "$cc" -std=c++20 -O2 -DNDEBUG -arch arm64 -isysroot "$sdk_root"
  -fPIC -ffunction-sections -fdata-sections -ftrivial-auto-var-init=zero
  -DART_PAGE_SIZE_AGNOSTIC -DBUILDING_LIBART -DUSE_D8_DESUGAR
  -DART_DEFAULT_GC_TYPE_IS_CMS -DART_FRAME_SIZE_LIMIT=1744
  '-DART_BASE_ADDRESS=0x70000000'
  '-DART_BASE_ADDRESS_MIN_DELTA=(-0x1000000)'
  '-DART_BASE_ADDRESS_MAX_DELTA=0x1000000'
  -DART_STACK_OVERFLOW_GAP_arm=8192
  -DART_STACK_OVERFLOW_GAP_arm64=8192
  -DART_STACK_OVERFLOW_GAP_riscv64=8192
  -DART_STACK_OVERFLOW_GAP_x86=8192
  -DART_STACK_OVERFLOW_GAP_x86_64=8192
  -Wall -Wextra -Werror
  -Wno-invalid-offsetof -Wno-unsupported-visibility
  -Wno-deprecated-enum-enum-conversion -Wno-nontrivial-memcall
  -Wno-unused-parameter -Wno-unused-variable
)
for include in "${includes[@]}"; do compile+=("-I$include"); done
compile+=(
  -include base/globals.h
  -include mirror/object_reference.h
  -include mirror/string-inl.h
  -idirafter "$ndk_include/aarch64-linux-android"
  -idirafter "$ndk_include"
  -Wno-macro-redefined
  -c "$patched_root/art/openjdkjvm/OpenjdkJvm.cc" -o "$object"
)
"${compile[@]}"
[[ "$(file "$object")" == *"Mach-O 64-bit object arm64"* ]] ||
  fail "non-arm64 OpenjdkJvm object"

exports="$stage/openjdkjvm-exports.txt"
nm -gjU "$object" | sed 's/^_//' | grep -E '^(JVM_|jio_)' | sort > "$exports"
[[ -s "$exports" ]] || fail "no OpenJDK JVM exports found"
cmp -s "$source_exports" "$exports" ||
  fail "source declarations and compiled public definitions disagree"
grep -Fx 'JVM_GetLastErrorString' "$exports" >/dev/null ||
  fail "JVM_GetLastErrorString export missing"
export_count="$(wc -l < "$exports" | tr -d ' ')"
export_sha="$(sha256 "$exports")"
[[ "$export_count" == "$OPENJDKJVM_EXPORT_COUNT" &&
   "$export_sha" == "$OPENJDKJVM_EXPORT_MANIFEST_SHA256" ]] ||
  fail "export drift count=$export_count sha=$export_sha"

archive="$stage/libopenjdkjvm-darwin.a"
"$libtool_bin" -static -o "$archive" "$object"
[[ "$(lipo -archs "$archive")" == arm64 ]] || fail "non-arm64 archive"
[[ "$({ ar -t "$archive" || true; } | grep -v '^__\.SYMDEF' | wc -l | tr -d ' ')" == \
   "$OPENJDKJVM_SOURCE_COUNT" ]] || fail "archive member count mismatch"

duplicates="$stage/duplicate-providers.txt"
for provider in \
  "$project_root/_build/runtime-bootstrap/libart-runtime-bootstrap-darwin.a" \
  "$project_root/_build/runtime-graphics-bootstrap/libart-runtime-graphics-bootstrap-darwin.a" \
  "$project_root/_build/runtime-core/libart-core-darwin.a" \
  "$project_root/_build/runtime-platform/libart-platform-darwin.a" \
  "$project_root/_build/runtime-arm64/libart-arm64-darwin.a" \
  "$project_root/_build/interpreter-core/libart-interpreter-darwin.a" \
  "$project_root/_build/nativehelper-device-foundation/libnativehelper-device-darwin.a" \
  "$project_root/_build/libbase-foundation/libandroid-base-darwin.a"; do
  [[ -f "$provider" ]] || continue
  comm -12 "$exports" \
    <(nm -gjU "$provider" | sed 's/^_//' | grep -E '^(JVM_|jio_)' | sort -u) \
    | sed "s|^|$provider\t|" >> "$duplicates"
done
[[ ! -s "$duplicates" ]] || {
  cat "$duplicates" >&2
  fail "duplicate OpenJDK JVM providers detected"
}

smoke="$stage/openjdkjvm-last-error-smoke"
"$cc" -std=c++20 -O2 -arch arm64 -isysroot "$sdk_root" \
  -I"$project_root/_aosp/libnativehelper-full/include_jni" \
  "$project_root/probes/openjdkjvm_last_error_smoke.cc" "$archive" \
  -Wl,-dead_strip -o "$smoke"
smoke_output="$("$smoke")"
[[ "$smoke_output" == openjdkjvm-last-error:\ length=*\ nonempty=pass\ zero=pass ]] ||
  fail "last-error smoke failed: $smoke_output"

undefined="$stage/openjdkjvm-undefined.txt"
nm -u "$archive" | sed 's/^[[:space:]]*//' | sort -u > "$undefined"
mkdir -p "$build_dir"
cp "$archive" "$build_dir/libopenjdkjvm-darwin.a"
cp "$exports" "$build_dir/openjdkjvm-exports.txt"
cp "$source_exports" "$build_dir/openjdkjvm-source-exports.txt"
cp "$undefined" "$build_dir/openjdkjvm-undefined.txt"
cp "$duplicates" "$build_dir/duplicate-providers.txt"
cp "$patched_root/art/openjdkjvm/OpenjdkJvm.cc" \
  "$build_dir/OpenjdkJvm.darwin.cc"

echo "openjdkjvm: sources=1 exports=$export_count export_sha=$export_sha duplicate_providers=0 last_error=pass archive=Mach-O-arm64"
