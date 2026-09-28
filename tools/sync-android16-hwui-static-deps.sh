#!/bin/bash
set -euo pipefail

script_dir="$(cd "$(dirname "$0")" && pwd)"
project_root="$(cd "$script_dir/.." && pwd)"
aosp_root="$project_root/_aosp"
lock_file="$project_root/upstream/android16-hwui-static-foundation.lock"
deps_root="$aosp_root/hwui-static-deps"
build_root="$project_root/_build/hwui-static-deps"

# shellcheck disable=SC1090
source "$lock_file"

download_file() {
  local project="$1" revision="$2" relative="$3" destination="$4"
  [[ -f "$destination" ]] && return
  mkdir -p "$(dirname "$destination")"
  local temporary
  temporary="$(mktemp "${destination}.tmp.XXXXXX")"
  curl -fsSL "https://android.googlesource.com/$project/+/$revision/$relative?format=TEXT" |
    base64 -D > "$temporary"
  mv "$temporary" "$destination"
}

download_subtree() {
  local project="$1" revision="$2" subtree="$3" destination="$4" sentinel="$5"
  [[ -f "$destination/$sentinel" ]] && return
  mkdir -p "$destination"
  local stage
  stage="$(mktemp -d "$deps_root/subtree.XXXXXX")"
  curl -fsSL "https://android.googlesource.com/$project/+archive/$revision/$subtree.tar.gz" |
    tar -xzf - -C "$stage"
  cp -R "$stage/." "$destination/"
  rm -rf "$stage"
  [[ -f "$destination/$sentinel" ]] || {
    echo "hwui-static-deps: incomplete subtree project=$project revision=$revision subtree=$subtree" >&2
    exit 2
  }
}

verify_sha() {
  local file="$1" expected="$2" actual
  actual="$(shasum -a 256 "$file" | awk '{print $1}')"
  [[ "$actual" == "$expected" ]] || {
    echo "hwui-static-deps: checksum mismatch file=$file expected=$expected actual=$actual" >&2
    exit 2
  }
}

mkdir -p "$deps_root" "$build_root"
sysprop_root="$deps_root/system-tools-sysprop"
for file in Android.bp sysprop.proto Common.cpp CodeWriter.cpp CppGen.cpp CppMain.cpp \
  include/Common.h include/CodeWriter.h include/CppGen.h; do
  download_file "$SYSPROP_PROJECT" "$SYSPROP_REVISION" "$file" "$sysprop_root/$file"
done
verify_sha "$sysprop_root/Android.bp" "$SYSPROP_ANDROID_BP_SHA256"
verify_sha "$sysprop_root/sysprop.proto" "$SYSPROP_PROTO_SHA256"
verify_sha "$sysprop_root/Common.cpp" "$SYSPROP_COMMON_CPP_SHA256"
verify_sha "$sysprop_root/CodeWriter.cpp" "$SYSPROP_CODE_WRITER_CPP_SHA256"
verify_sha "$sysprop_root/CppGen.cpp" "$SYSPROP_CPP_GEN_CPP_SHA256"
verify_sha "$sysprop_root/CppMain.cpp" "$SYSPROP_CPP_MAIN_CPP_SHA256"
verify_sha "$sysprop_root/include/Common.h" "$SYSPROP_COMMON_H_SHA256"
verify_sha "$sysprop_root/include/CodeWriter.h" "$SYSPROP_CODE_WRITER_H_SHA256"
verify_sha "$sysprop_root/include/CppGen.h" "$SYSPROP_CPP_GEN_H_SHA256"

download_subtree "$FRAMEWORKS_NATIVE_PROJECT" "$FRAMEWORKS_NATIVE_REVISION" \
  libs/gui/include "$deps_root/frameworks-native/libs/gui/include" gui/Surface.h
download_subtree "$FRAMEWORKS_NATIVE_PROJECT" "$FRAMEWORKS_NATIVE_REVISION" \
  libs/binder/include "$deps_root/frameworks-native/libs/binder/include" binder/IBinder.h
download_subtree "$FRAMEWORKS_NATIVE_PROJECT" "$FRAMEWORKS_NATIVE_REVISION" \
  libs/nativedisplay/include "$deps_root/frameworks-native/libs/nativedisplay/include" apex/display.h
download_subtree "$LIBHARDWARE_PROJECT" "$LIBHARDWARE_REVISION" \
  include_all "$deps_root/libhardware/include_all" hardware/hardware.h
verify_sha "$deps_root/libhardware/include_all/hardware/hardware.h" \
  "$LIBHARDWARE_HARDWARE_H_SHA256"

sysprop_cpp="$build_root/sysprop_cpp"
if [[ ! -x "$sysprop_cpp" ]]; then
  protoc="$(command -v protoc || true)"
  brew_bin="$(command -v brew || true)"
  [[ -n "$protoc" && -n "$brew_bin" ]] || {
    echo "hwui-static-deps: protoc and Homebrew protobuf are required" >&2
    exit 2
  }
  protobuf_root="$(brew --prefix protobuf)"
  abseil_root="$(brew --prefix abseil)"
  base_archive="$project_root/_build/foundation/libandroid-base-darwin.a"
  [[ -f "$base_archive" ]] || {
    echo "hwui-static-deps: missing module archive $base_archive" >&2
    exit 2
  }
  stage="$(mktemp -d "$build_root/stage.XXXXXX")"
  trap 'rm -rf "$stage"' EXIT
  "$protoc" --proto_path="$sysprop_root" --cpp_out="$stage" "$sysprop_root/sysprop.proto"
  cxx="$(command -v clang++)"
  flags=(
    -arch arm64 -std=c++20 -O2 -fPIC -Wno-deprecated-declarations
    -I"$sysprop_root/include" -I"$stage"
    -I"$aosp_root/system/libbase/include" -I"$aosp_root/external/fmtlib/include"
    -I"$protobuf_root/include"
    -I"$abseil_root/include"
  )
  objects=()
  for source in Common.cpp CodeWriter.cpp CppGen.cpp CppMain.cpp; do
    object="$stage/${source%.cpp}.o"
    "$cxx" "${flags[@]}" -c "$sysprop_root/$source" -o "$object"
    objects+=("$object")
  done
  "$cxx" "${flags[@]}" -c "$stage/sysprop.pb.cc" -o "$stage/sysprop.pb.o"
  "$cxx" "${flags[@]}" -c "$aosp_root/external/fmtlib/src/format.cc" \
    -o "$stage/fmt-format.o"
  "$cxx" "${flags[@]}" -c "$aosp_root/system/libbase/result.cpp" \
    -o "$stage/base-result.o"
  "$cxx" "${flags[@]}" -c "$aosp_root/system/libbase/posix_strerror_r.cpp" \
    -o "$stage/base-posix-strerror.o"
  "$cxx" -arch arm64 "${objects[@]}" "$stage/sysprop.pb.o" \
    "$stage/fmt-format.o" "$stage/base-result.o" "$stage/base-posix-strerror.o" \
    "$base_archive" "$protobuf_root/lib/libprotobuf.dylib" \
    "$abseil_root/lib/"libabsl_*.dylib -o "$stage/sysprop_cpp"
  mv "$stage/sysprop_cpp" "$sysprop_cpp"
fi

echo "hwui-static-deps: sparse-source=$deps_root"
echo "hwui-static-deps: sysprop_cpp=$sysprop_cpp"
