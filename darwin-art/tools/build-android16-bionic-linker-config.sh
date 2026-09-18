#!/bin/bash
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
src="$root/_aosp/bionic-linker-config"
out="$root/_build/bionic-linker-config"
mkdir -p "$src" "$out"
while read -r relative expected; do
  [[ -n "$relative" && "$relative" != \#* ]] || continue
  destination="$src/$relative"
  mkdir -p "$(dirname "$destination")"
  if [[ ! -f "$destination" ]]; then
    staged="$(mktemp "$src/source.XXXXXX")"
    trap 'rm -f -- "$staged"' EXIT
    curl -fsSL "https://android.googlesource.com/platform/bionic/+/09a271af557444c9a6b3f3146d6d474156fd6cdb/$relative?format=TEXT" | base64 -D > "$staged"
    [[ "$(shasum -a 256 "$staged" | awk '{print $1}')" == "$expected" ]]
    mv "$staged" "$destination"
  fi
  [[ "$(shasum -a 256 "$destination" | awk '{print $1}')" == "$expected" ]]
done < "$root/upstream/android16-bionic-linker-config.sources"
ndk="${ANDROID_NDK_HOME:-$HOME/Library/Android/sdk/ndk/28.2.13676358}"
mkdir -p "$out/source"
[[ "$(shasum -a 256 "$root/patches/native-loader-policy/bionic-config-filesystem.patch" | awk '{print $1}')" == b72e2c2d8c3a6b8199d08928c8e09d0eaf0eb4108b5465360c8baaa0c16c6499 ]]
cp "$src/linker/linker_config.cpp" "$src/linker/linker_utils.cpp" "$out/source/"
cp "$src/libc/async_safe/async_safe_log.cpp" "$src/linker/linker_debug.cpp" "$out/source/"
[[ "$(shasum -a 256 "$root/patches/native-loader-policy/async-safe-darwin.patch" | awk '{print $1}')" == 19c08a28c54ba29f9d28c0d4ed6338ddf9eb1f803f639f12b3b1223b8c6dbe2a ]]
repo_root="$(git -C "$root" rev-parse --show-toplevel)"
git -C "$repo_root" apply --unidiff-zero --directory="${out#"$repo_root/"}/source" \
  "$root/patches/native-loader-policy/bionic-config-filesystem.patch"
git -C "$repo_root" apply --unidiff-zero --directory="${out#"$repo_root/"}/source" \
  "$root/patches/native-loader-policy/async-safe-darwin.patch"
flags=(-std=gnu++20 -arch arm64 -O2 -D__ANDROID_API__=36 '-D__INTRODUCED_IN(n)='
  -Dandroid_set_abort_message=darwin_art_bionic_android_set_abort_message
  -include "$root/tools/native-loader-policy/bionic_config_types.h"
  -I"$src/linker" -I"$src/libc/async_safe/include"
  -I"$src/libc"
  -I"$root/_aosp/android16-native-loader-policy" -I"$root/_aosp/system/libbase/include"
  -I"$root/compat" -I"$root/include" -I"$root/tools/bionic-fs-facade/include"
  -I"$root/tools/bionic-errno-tls/include"
  -I"$root/tools/bionic-stat-facade/include" -I"$root/tools/bionic-ioctl-facade/include"
  -idirafter "$ndk/toolchains/llvm/prebuilt/darwin-x86_64/sysroot/usr/include"
  -idirafter "$ndk/toolchains/llvm/prebuilt/darwin-x86_64/sysroot/usr/include/aarch64-linux-android")
for unit in linker_config linker_utils linker_debug async_safe_log; do
  xcrun clang++ "${flags[@]}" -c "$out/source/$unit.cpp" -o "$out/$unit.o"
done
xcrun clang++ "${flags[@]}" -include "$root/compat/loader/warning_basename.h" \
  -Dbasename=darwin_art_linker_gnu_basename \
  -c "$src/linker/linker_dlwarning.cpp" \
  -MMD -MF "$out/linker_dlwarning.o.d" -o "$out/linker_dlwarning.o"
for unit in guest_config linker_config_fs; do
  xcrun clang++ "${flags[@]}" -c "$root/compat/filesystem/$unit.cc" -o "$out/$unit.o"
done
xcrun libtool -static -o "$out/libbionic-linker-config.a" "$out/linker_config.o" "$out/linker_utils.o" "$out/linker_debug.o" "$out/async_safe_log.o" "$out/guest_config.o" "$out/linker_config_fs.o" "$out/linker_dlwarning.o"
xcrun nm -u "$out/libbionic-linker-config.a" > "$out/required-symbols.txt"
if grep -E '^_(access|stat|realpath)(\$.*)?$' "$out/required-symbols.txt"; then
  echo 'host filesystem path import leaked into bionic config' >&2; exit 1
fi
if grep 'ReadFileToString' "$out/required-symbols.txt"; then
  echo 'host pathname reader leaked into bionic config' >&2; exit 1
fi
echo 'Bionic config guest filesystem import gate PASS; property/logging/startup linkage pending'
