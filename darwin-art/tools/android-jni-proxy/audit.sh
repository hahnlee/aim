#!/bin/bash
set -euo pipefail
export LC_ALL=C

script_dir="$(cd "$(dirname "$0")" && pwd)"
lock="$script_dir/sources.lock"
fail() { echo "android-jni-proxy: $1" >&2; exit 2; }
[[ -f "$lock" ]] || fail 'missing sources.lock'
# shellcheck disable=SC1090
source "$lock"

ndk_root="${ANDROID_NDK_ROOT:-$HOME/Library/Android/sdk/ndk/$NDK_REVISION}"
toolchain="$ndk_root/toolchains/llvm/prebuilt/darwin-x86_64/bin"
ndk_jni="$ndk_root/toolchains/llvm/prebuilt/darwin-x86_64/sysroot/usr/include/jni.h"
android_cc="$toolchain/aarch64-linux-android35-clang"
readelf="$toolchain/llvm-readelf"
objdump="$toolchain/llvm-objdump"
[[ -f "$ndk_jni" && -x "$android_cc" && -x "$readelf" && -x "$objdump" ]] ||
  fail 'missing pinned NDK toolchain'
[[ "$(shasum -a 256 "$ndk_jni" | awk '{print $1}')" == \
   "$NDK_JNI_HEADER_SHA256" ]] || fail 'NDK jni.h hash mismatch'

temp_root="$(mktemp -d "${TMPDIR:-/tmp}/android-jni-proxy.XXXXXX")"
cleanup() {
  if [[ -n "$temp_root" && "$temp_root" == "${TMPDIR:-/tmp}"/android-jni-proxy.* ]]; then
    find "$temp_root" -depth -delete
  fi
}
trap cleanup EXIT
mkdir -p "$temp_root/android16"

curl -fsSL "$ANDROID16_JNI_HEADER_URL" | base64 --decode >"$temp_root/android16/jni.h"
[[ "$(shasum -a 256 "$temp_root/android16/jni.h" | awk '{print $1}')" == \
   "$ANDROID16_JNI_HEADER_SHA256" ]] || fail 'Android 16 jni.h hash mismatch'
python3 "$script_dir/tools/generate_slots.py" "$temp_root/android16/jni.h" \
  >"$temp_root/android16-slots.h"
diff -u "$script_dir/generated/jni_slots.h" "$temp_root/android16-slots.h" ||
  fail 'generated Android 16 slot table drift'
python3 "$script_dir/tools/generate_slots.py" "$ndk_jni" >"$temp_root/ndk-slots.h"
diff -u "$script_dir/generated/jni_slots.h" "$temp_root/ndk-slots.h" ||
  fail 'NDK compiler header slot table differs from Android 16'
grep -F "$ANDROID16_NATIVE_NAMES_SHA256" "$script_dir/generated/jni_slots.h" >/dev/null ||
  fail 'native slot-name digest drift'
grep -F "$ANDROID16_INVOKE_NAMES_SHA256" "$script_dir/generated/jni_slots.h" >/dev/null ||
  fail 'invoke slot-name digest drift'

host_cc="$(xcrun --find clang)"
sdk_root="$(xcrun --sdk macosx --show-sdk-path)"
host_flags=(-arch arm64 -isysroot "$sdk_root" -std=c17 -O2 -Wall -Wextra
            -Werror -Wpedantic -I"$script_dir/include" -I"$script_dir/generated")
"$host_cc" "${host_flags[@]}" -I"$temp_root/android16" -fsyntax-only \
  "$script_dir/probes/abi_layout.c"
"$android_cc" -std=c17 -Wall -Wextra -Werror -Wpedantic \
  -I"$temp_root/android16" -fsyntax-only "$script_dir/probes/abi_layout.c"

"$host_cc" "${host_flags[@]}" -c "$script_dir/src/proxy.c" \
  -o "$temp_root/proxy.o"
"$host_cc" -arch arm64 -isysroot "$sdk_root" -c \
  "$script_dir/src/aapcs64_call.S" -o "$temp_root/aapcs64_call.o"
[[ "$(nm -u "$temp_root/proxy.o")" == *'_darwin_art_jni_proxy_call_void_method'* ]] ||
  fail 'proxy direct-call entry dependency missing'
definitions="$(nm -gU "$temp_root/proxy.o")"
grep -F ' _darwin_art_jni_proxy_init' <<<"$definitions" >/dev/null ||
  fail 'missing proxy init export'
grep -F ' _darwin_art_jni_proxy_java_vm' <<<"$definitions" >/dev/null ||
  fail 'missing proxy JavaVM export'
if awk '$2 ~ /^[TDS]$/ {print $3}' <<<"$definitions" |
   grep -Ev '^_darwin_art_jni_proxy_' >/dev/null; then
  fail 'unprefixed proxy definition escaped'
fi
if rg -n 'dlsym|JNIEnvExt|JavaVMExt|ArtMethod|art::' "$script_dir/src" \
   "$script_dir/include" >/dev/null; then
  fail 'ART table exposure or host symbol lookup entered proxy'
fi

"$host_cc" "${host_flags[@]}" "$script_dir/src/proxy.c" \
  "$script_dir/src/aapcs64_call.S" \
  "$script_dir/probes/fake_backend.c" "$script_dir/probes/native_smoke.c" \
  -o "$temp_root/native-smoke"
"$temp_root/native-smoke"
"$host_cc" "${host_flags[@]}" -O1 -g -fsanitize=address,undefined \
  "$script_dir/src/proxy.c" "$script_dir/probes/fake_backend.c" \
  "$script_dir/src/aapcs64_call.S" \
  "$script_dir/probes/native_smoke.c" -o "$temp_root/native-smoke-sanitized"
"$temp_root/native-smoke-sanitized" >/dev/null

fixture="$temp_root/libjni_proxy_fixture.so"
"$android_cc" -std=c17 -O2 -fPIC -fno-stack-protector -Wall -Wextra \
  -Werror -Wpedantic -I"$temp_root/android16" -shared -nostdlib \
  -Wl,-soname,libjni_proxy_fixture.so -Wl,-z,now -Wl,-z,norelro \
  -Wl,--hash-style=sysv "$script_dir/probes/fixture.c" -o "$fixture"
"$readelf" -h "$fixture" | grep -F 'Machine:                           AArch64' >/dev/null ||
  fail 'fixture is not Android AArch64 ELF'
"$readelf" --dyn-syms --wide "$fixture" >"$temp_root/dynsyms"
grep -E 'GLOBAL DEFAULT +[0-9]+ JNI_OnLoad$' "$temp_root/dynsyms" >/dev/null ||
  fail 'fixture lacks JNI_OnLoad export'
grep -E 'GLOBAL DEFAULT +[0-9]+ jni_proxy_fixture_run$' "$temp_root/dynsyms" >/dev/null ||
  fail 'fixture lacks runner export'
awk '$7=="UND" && $8!="" {print $8}' "$temp_root/dynsyms" | sort -u \
  >"$temp_root/undefined"
[[ "$(cat "$temp_root/undefined")" == 'darwin_art_jni_fixture_vm' ]] ||
  fail 'fixture import namespace drift'
"$objdump" --disassemble-symbols=JNI_OnLoad "$fixture" >"$temp_root/onload.asm"
[[ "$(grep -Ec '[[:space:]]blr[[:space:]]' "$temp_root/onload.asm")" -ge 8 ]] ||
  fail 'JNI_OnLoad no longer performs the expected indirect JNI slot calls'

CARGO_TARGET_DIR="$temp_root/cargo-target" cargo run --quiet \
  --manifest-path "$script_dir/Cargo.toml" -- "$fixture"
CARGO_TARGET_DIR="$temp_root/cargo-target" cargo clippy --quiet \
  --manifest-path "$script_dir/Cargo.toml" -- -D warnings
cargo fmt --manifest-path "$script_dir/Cargo.toml" -- --check

echo 'android-jni-proxy: PASS Android16 slots=233+8 populated=229 (reserved=4) variadic+va_list+nonvirtual=Android-ABI calls+strings+refs+classes+methods+fields+arrays+direct-buffer+exceptions=current-env-forwarded ELF=AArch64 proxy-only E2E ASAN+UBSAN'
