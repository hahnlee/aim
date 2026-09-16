#!/bin/bash
set -euo pipefail
export LC_ALL=C
root="$(cd "$(dirname "$0")/.." && pwd)"
revision=09a271af557444c9a6b3f3146d6d474156fd6cdb
source_dir="$root/_aosp/android16-property-client"
out="$root/_build/android16-property-client"
ndk="${ANDROID_NDK_ROOT:-$HOME/Library/Android/sdk/ndk/28.2.13676358}"
toolchain="$ndk/toolchains/llvm/prebuilt/darwin-x86_64"
[[ "$(shasum -a 256 "$ndk/source.properties" | awk '{print $1}')" == \
  c00aa236fdb205e9be9edd9e2169763e48aca52735efff4e16f34205d49783b5 ]]
mkdir -p "$out"
while IFS=$'\t' read -r local_path upstream sha; do
  [[ "$local_path" != local ]] || continue
  mkdir -p "$(dirname "$source_dir/$local_path")"
  if [[ ! -f "$source_dir/$local_path" ]]; then
    curl -fsSL "https://android.googlesource.com/platform/bionic/+/$revision/$upstream?format=TEXT" |
      base64 -D > "$source_dir/$local_path"
  fi
  [[ "$(shasum -a 256 "$source_dir/$local_path" | awk '{print $1}')" == "$sha" ]] || {
    echo "property-client: source hash mismatch: $local_path" >&2; exit 1;
  }
done < "$root/tools/android16-property-client/sources.tsv"
flags=(-target arm64-apple-macos14 -O2 -std=c++17 -fno-builtin
  -fno-stack-protector -fno-exceptions -fno-rtti -D__ANDROID_API__=36
  -nostdinc -isystem "$toolchain/lib/clang/19/include"
  -I"$source_dir" -isystem "$toolchain/sysroot/usr/include/aarch64-linux-android"
  -isystem "$toolchain/sysroot/usr/include")
for symbol in __errno __system_property_get access socket connect close recv send \
  writev poll strlen strcmp strncmp strlcpy memset atoll \
  async_safe_format_log async_safe_fatal_no_abort abort; do
  flags+=("-D$symbol=darwin_art_property_import_$symbol")
done
flags+=(-D__system_property_set=darwin_art_aosp_system_property_set)
xcrun clang++ "${flags[@]}" -c "$source_dir/system_property_set.cpp" \
  -o "$out/system_property_set.o"
nm -u "$out/system_property_set.o" | awk '/^_/ {print $1}' | sort -u > "$out/undefined.txt"
if grep -Ev '^_darwin_art_property_import_' "$out/undefined.txt"; then
  echo 'property-client: unclassified import' >&2; exit 1
fi
binding_flags=(-target arm64-apple-macos14 -O2 -std=c17 -fno-builtin
  -Wall -Wextra -Werror -Wpedantic)
for provider in bionic-errno-tls bionic-fs-facade bionic-ioctl-facade \
  bionic-libc-leaf-facade bionic-numeric-facade bionic-process-state-facade \
  bionic-socket-broker-adapter; do
  binding_flags+=("-I$root/tools/$provider/include")
done
xcrun clang "${binding_flags[@]}" -c "$root/tools/android16-property-client/bindings.c" \
  -o "$out/bindings.o"
# Reuse the pinned AOSP formatter and reviewed Darwin stderr sink patch.
bash "$root/tools/build-android16-bionic-linker-config.sh"
log_source="$root/_build/bionic-linker-config/source/async_safe_log.cpp"
log_flags=(-std=gnu++20 -target arm64-apple-macos14 -O2 -fno-exceptions -fno-rtti -D__ANDROID_API__=36
  '-D__INTRODUCED_IN(n)=' -D__error=darwin_art_bionic___errno
  -Dstrerror_r=darwin_art_bionic_strerror_r
  -Dandroid_set_abort_message=darwin_art_bionic_android_set_abort_message
  -include "$root/tools/native-loader-policy/bionic_config_types.h"
  -I"$root/_aosp/bionic-linker-config/libc/async_safe/include"
  -I"$root/_aosp/bionic-linker-config/libc"
  -I"$root/_aosp/android16-native-loader-policy"
  -idirafter "$toolchain/sysroot/usr/include"
  -idirafter "$toolchain/sysroot/usr/include/aarch64-linux-android")
for name in format_buffer_va_list format_buffer format_fd_va_list format_fd \
  write_log format_log_va_list format_log fatal_va_list fatal_no_abort; do
  log_flags+=("-Dasync_safe_$name=darwin_art_property_log_$name")
done
xcrun clang++ "${log_flags[@]}" -c "$log_source" -o "$out/async_safe_log.o"
xcrun clang "${binding_flags[@]}" -I"$root/tools/bionic-abort-facade/include" \
  -c "$root/tools/android16-property-client/logging.c" -o "$out/logging.o"
xcrun ld -r -arch arm64 -o "$out/bound-client.o" \
  "$out/system_property_set.o" "$out/bindings.o" "$out/logging.o" "$out/async_safe_log.o"
nm -u "$out/bound-client.o" | awk '/^_/ {print $1}' | sort -u > "$out/bound-undefined.txt"
if grep -Ev '^_darwin_art_bionic_|^(___assert_rtn|___stack_chk_fail|___stack_chk_guard|___tolower|_memcpy|_strlen|_write|_writev)$' \
    "$out/bound-undefined.txt"; then
  echo 'property-client: unexpected client or native logging import' >&2; exit 1
fi
xcrun clang -I"$root/tools/bionic-errno-tls/include" \
  -I"$root/tools/bionic-errno-tls/generated" \
  -c "$root/tools/bionic-errno-tls/src/errno_tls.c" -o "$out/errno-test.o"
xcrun clang -I"$root/tools/bionic-strerror-facade/include" \
  -I"$root/tools/bionic-strerror-facade/generated" \
  -c "$root/tools/bionic-strerror-facade/src/strerror.c" -o "$out/strerror-test.o"
xcrun clang -I"$root/tools/bionic-errno-tls/include" \
  "$root/tools/android16-property-client/logging_test.c" \
  "$out/errno-test.o" "$out/strerror-test.o" "$out/async_safe_log.o" \
  -Wl,-dead_strip -o "$out/logging-test"
"$out/logging-test"
echo 'property-client: client imports bound; service integration pending'
