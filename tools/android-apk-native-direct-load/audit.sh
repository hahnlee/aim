#!/bin/bash
set -euo pipefail
export LC_ALL=C

tool_root="$(cd "$(dirname "$0")" && pwd)"
project_root="$(cd "$tool_root/../.." && pwd)"
loader_root="$project_root/crates/aim-elf-loader"
source "$tool_root/source.lock"

fail() { echo "apk-native-direct-load: $*" >&2; exit 3; }
sha() { shasum -a 256 "$1" | awk '{print $1}'; }
for command in curl base64 shasum python3 xcrun cargo; do
  command -v "$command" >/dev/null || fail "missing command: $command"
done

stage="$(mktemp -d "${TMPDIR:-/tmp}/apk-native-direct-load.XXXXXX")"
trap 'chmod -R u+w "$stage" 2>/dev/null || true; rm -rf "$stage"' EXIT

sources=(
  "art/libnativeloader/native_loader.cpp"
  "art/libnativeloader/native_loader_namespace.cpp"
  "build/tools/zipalign/ZipAlign.cpp"
  "build/tools/zipalign/ZipAlignMain.cpp"
)
hashes=(
  "$NATIVE_LOADER_CPP_SHA256"
  "$NATIVE_LOADER_NAMESPACE_CPP_SHA256"
  "$ZIP_ALIGN_CPP_SHA256"
  "$ZIP_ALIGN_MAIN_CPP_SHA256"
)
for index in "${!sources[@]}"; do
  item="${sources[$index]}"
  project="${item%%/*}"
  relative="${item#*/}"
  revision="$ART_REVISION"
  [[ "$project" == build ]] && revision="$BUILD_REVISION"
  destination="$stage/$(basename "$relative")"
  curl -fsSL "https://android.googlesource.com/platform/$project/+/$revision/$relative?format=TEXT" |
    base64 -D > "$destination"
  [[ "$(sha "$destination")" == "${hashes[$index]}" ]] || fail "source hash drift: $item"
done
grep -F 'return pageSize;' "$stage/ZipAlign.cpp" >/dev/null || fail "zipalign .so page rule drift"
grep -F 'if (pEntry->isCompressed() || isDirectory(pEntry))' "$stage/ZipAlign.cpp" >/dev/null || \
  fail "zipalign stored-only alignment rule drift"
grep -F 'Valid values for <pagesize_kb> are 4, 16' "$stage/ZipAlignMain.cpp" >/dev/null || \
  fail "zipalign page-size CLI rule drift"
grep -F 'Result<void*> NativeLoaderNamespace::Load(const char* lib_name) const' \
  "$stage/native_loader_namespace.cpp" >/dev/null || fail "NativeLoader namespace load drift"
grep -F 'android_dlopen_ext(lib_name, RTLD_NOW, &extinfo)' \
  "$stage/native_loader_namespace.cpp" >/dev/null || fail "NativeLoader RTLD_NOW drift"

sdk_root="${ANDROID_SDK_ROOT:-$HOME/Library/Android/sdk}"
ndk="$sdk_root/ndk/28.2.13676358"
clang="$ndk/toolchains/llvm/prebuilt/darwin-x86_64/bin/aarch64-linux-android35-clang"
readelf="$ndk/toolchains/llvm/prebuilt/darwin-x86_64/bin/llvm-readelf"
[[ -x "$clang" && -x "$readelf" ]] || fail "pinned NDK r28c toolchain is missing"
fixture_dir="$stage/fixtures"
mkdir -p "$fixture_dir"
flags=(-shared -fPIC -O2 -nostdlib -fuse-ld=lld -Wl,--hash-style=sysv
  -Wl,--build-id=none -Wl,-z,now -Wl,-z,norelro -Wl,-z,max-page-size=16384)
grandchild="$fixture_dir/libapk-direct-grandchild.so"
child="$fixture_dir/libapk-direct-child.so"
root="$fixture_dir/libapk-direct-root.so"
"$clang" "${flags[@]}" -Wl,-soname,libapk-direct-grandchild.so \
  -Wl,--version-script,"$tool_root/fixtures/grandchild.map" \
  "$tool_root/fixtures/grandchild.c" -o "$grandchild"
"$clang" "${flags[@]}" -Wl,-soname,libapk-direct-child.so \
  -Wl,--version-script,"$tool_root/fixtures/child.map" \
  "$tool_root/fixtures/child.c" "$grandchild" -o "$child"
"$clang" "${flags[@]}" -Wl,-soname,libapk-direct-root.so \
  -Wl,--version-script,"$tool_root/fixtures/root.map" \
  "$tool_root/fixtures/root.c" "$child" -o "$root"
for elf in "$root" "$child" "$grandchild"; do
  file "$elf" | grep -F 'ELF 64-bit LSB shared object, ARM aarch64' >/dev/null || \
    fail "fixture is not AArch64 ELF: $elf"
done
grep -E '\(NEEDED\).*libapk-direct-child\.so' < <("$readelf" -d "$root") >/dev/null || \
  fail "root lost child dependency"
grep -E '\(NEEDED\).*libapk-direct-grandchild\.so' < <("$readelf" -d "$child") >/dev/null || \
  fail "child lost grandchild dependency"

variants=(valid misaligned deflated encrypted descriptor zip64 traversal duplicate crc local-mismatch mutable)
for variant in "${variants[@]}"; do
  python3 "$tool_root/make_fixture.py" "$stage/$variant.apk" "$variant" \
    "$root" "$child" "$grandchild"
done

cargo build --quiet --release --manifest-path "$loader_root/Cargo.toml" --lib
staticlib="$project_root/target/release/libaim_elf_loader.a"
compile=(xcrun clang++ -std=c++17 -arch arm64 -Wall -Wextra -Werror
  -I "$loader_root/include" "$tool_root/direct_load.cc" "$staticlib"
  -framework Security -framework CoreFoundation -liconv)
"${compile[@]}" -o "$stage/direct-load"

output="$("$stage/direct-load" "$stage/valid.apk" libapk-direct-root.so)"
grep -F 'apk-native-direct-load: PASS source=readonly-mmap fd-slices=3 copy=0 extract=0 alignment=16384 crc=verified graph=root+child+grandchild ctor=dependency-first JNI_OnLoad=0x00010006 unload=success fallback=none' \
  <<< "$output" >/dev/null || fail "positive gate output mismatch: $output"

check_reject() {
  local variant="$1" expected="$2"
  shift 2
  if "$stage/direct-load" "$stage/$variant.apk" libapk-direct-root.so "$@" \
      >"$stage/$variant.stdout" 2>"$stage/$variant.stderr"; then
    fail "$variant unexpectedly loaded"
  fi
  grep -F "$expected" "$stage/$variant.stderr" >/dev/null || \
    fail "$variant returned wrong error: $(cat "$stage/$variant.stderr")"
}
check_reject misaligned 'not 16 KiB aligned'
check_reject deflated 'not STORED'
check_reject encrypted 'encrypted or masked'
check_reject descriptor 'data descriptor is forbidden'
check_reject zip64 'ZIP64 entry is forbidden'
check_reject traversal 'unsafe ZIP path'
check_reject duplicate 'duplicate ZIP entry'
check_reject crc 'native entry CRC mismatch'
check_reject local-mismatch 'central/local metadata mismatch'
check_reject mutable 'mutable APK changed after validation' --inject-mutation

"${compile[@]}" -fsanitize=address,undefined -fno-omit-frame-pointer \
  -o "$stage/direct-load-asan"
ASAN_OPTIONS=detect_leaks=0:abort_on_error=1 UBSAN_OPTIONS=halt_on_error=1 \
  "$stage/direct-load-asan" "$stage/valid.apk" libapk-direct-root.so >/dev/null
"${compile[@]}" -fsanitize=thread -fno-omit-frame-pointer -o "$stage/direct-load-tsan"
TSAN_OPTIONS=halt_on_error=1 \
  "$stage/direct-load-tsan" "$stage/valid.apk" libapk-direct-root.so >/dev/null

printf '%s\n' "$output"
echo "apk-native-direct-load: PASS source-lock=$ANDROID_TAG files=3 page=16KiB negatives=misaligned+deflated+encrypted+descriptor+ZIP64+traversal+duplicate+CRC+local-mismatch+mutable-race sanitizers=ASan+UBSan+TSan runtime-files-modified=0"
