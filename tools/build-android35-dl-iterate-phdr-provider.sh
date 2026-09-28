#!/bin/bash
set -euo pipefail
export LC_ALL=C

project_root="$(cd "$(dirname "$0")/.." && pwd)"
source "$project_root/upstream/android35-dl-iterate-phdr-provider.lock"
module_root="$project_root/tools/android-dl-iterate-phdr-provider"
runner_root="$project_root/tools/android-dl-iterate-phdr-runner"
source_root="$project_root/_aosp/bionic-dl-iterate-phdr"
build_dir="$project_root/_build/dl-iterate-phdr-provider"
ndk_root="${ANDROID_NDK_ROOT:-$HOME/Library/Android/sdk/ndk/$NDK_REVISION}"
toolchain="$ndk_root/toolchains/llvm/prebuilt/darwin-x86_64"
libdl_stub="$toolchain/sysroot/usr/lib/aarch64-linux-android/$ANDROID_API/libdl.so"
ndk_link_h="$toolchain/sysroot/usr/include/link.h"
android_clang="$toolchain/bin/aarch64-linux-android${ANDROID_API}-clang"
readelf="$toolchain/bin/llvm-readelf"
elf_nm="$toolchain/bin/llvm-nm"

fail() { echo "dl-iterate-phdr-provider: $*" >&2; exit 3; }
missing() { echo "dl-iterate-phdr-provider: missing $*" >&2; exit 2; }
sha() { shasum -a 256 "$1" | awk '{print $1}'; }

for input in "$libdl_stub" "$ndk_link_h"; do
  [[ -f "$input" ]] || missing "$input"
done
for tool in "$android_clang" "$readelf" "$elf_nm"; do
  [[ -x "$tool" ]] || missing "$tool"
done
[[ "$(stat -f '%z' "$libdl_stub")" == "$NDK_LIBDL_STUB_SIZE" ]] ||
  fail "NDK libdl stub size mismatch"
[[ "$(sha "$libdl_stub")" == "$NDK_LIBDL_STUB_SHA256" ]] ||
  fail "NDK libdl stub SHA mismatch"
[[ "$(sha "$ndk_link_h")" == "$NDK_LINK_H_SHA256" ]] ||
  fail "NDK link.h SHA mismatch"

files=(
  linker/linker.cpp
  linker/dlfcn.cpp
  libc/include/link.h
  libdl/libdl.cpp
)
hashes=(
  "$BIONIC_LINKER_CPP_SHA256"
  "$BIONIC_DLFCN_CPP_SHA256"
  "$BIONIC_LINK_H_SHA256"
  "$BIONIC_LIBDL_CPP_SHA256"
)
[[ "${#files[@]}" == "$LOCKED_BIONIC_SOURCE_COUNT" ]] ||
  fail "locked Bionic source count mismatch"
for index in "${!files[@]}"; do
  relative="${files[$index]}"
  destination="$source_root/$relative"
  expected="${hashes[$index]}"
  if [[ ! -f "$destination" ]]; then
    mkdir -p "$(dirname "$destination")"
    staged="$(mktemp "${TMPDIR:-/tmp}/dl-phdr-source.XXXXXX")"
    curl -fsSL "https://android.googlesource.com/$BIONIC_PROJECT/+/$BIONIC_REVISION/$relative?format=TEXT" |
      base64 -D > "$staged"
    [[ "$(sha "$staged")" == "$expected" ]] ||
      fail "download SHA mismatch: $relative"
    mv "$staged" "$destination"
  fi
  [[ "$(sha "$destination")" == "$expected" ]] ||
    fail "Bionic source SHA mismatch: $relative"
done
printf '%s\n' "$BIONIC_REVISION" > "$source_root/.source-revision"
[[ -z "$(find "$source_root" \( -name .git -o -name .gitmodules \) -print -quit)" ]] ||
  fail "Git metadata is forbidden in sparse source"

python3 - "$source_root" <<'PY'
import sys
from pathlib import Path

root = Path(sys.argv[1])
linker = (root / "linker/linker.cpp").read_text()
dlfcn = (root / "linker/dlfcn.cpp").read_text()
header = (root / "libc/include/link.h").read_text()
libdl = (root / "libdl/libdl.cpp").read_text()
for token in ("dlpi_addr", "dlpi_name", "dlpi_phdr", "dlpi_phnum",
              "dlpi_adds", "dlpi_subs", "dlpi_tls_modid", "dlpi_tls_data"):
    assert token in linker, token
assert "do_dl_iterate_phdr" in linker
assert "if (rv != 0)" in linker and "return rv;" in linker
assert "g_dl_mutex" in dlfcn and "dl_iterate_phdr" in dlfcn
assert "recursive" in dlfcn.lower() or "recursive" in linker.lower()
assert "__loader_dl_iterate_phdr" in libdl and "dl_iterate_phdr" in libdl
assert "struct dl_phdr_info" in header and "dlpi_adds" in header
print("dl-iterate-phdr-provider: upstream-semantics=PASS sources=4")
PY

[[ "$(sha "$module_root/fixture/target.c")" == "$TARGET_C_SHA256" ]] ||
  fail "target fixture source SHA mismatch"
[[ "$(sha "$module_root/fixture/target.map")" == "$TARGET_MAP_SHA256" ]] ||
  fail "target map SHA mismatch"
[[ "$(sha "$module_root/fixture/helper.c")" == "$HELPER_C_SHA256" ]] ||
  fail "helper fixture source SHA mismatch"
[[ "$(sha "$module_root/fixture/helper.map")" == "$HELPER_MAP_SHA256" ]] ||
  fail "helper map SHA mismatch"

stage="$(mktemp -d "${TMPDIR:-/tmp}/dl-phdr-provider.XXXXXX")"
trap 'rm -rf "$stage"' EXIT
cxx="$(xcrun --find clang++)"
ar="$(xcrun --find ar)"
host_nm="$(xcrun --find nm)"
macos_sdk="$(xcrun --sdk macosx --show-sdk-path)"
host_flags=(-std=c++20 -arch arm64 -isysroot "$macos_sdk" -O2 -Wall -Wextra -Werror
            -fvisibility=hidden -fvisibility-inlines-hidden -I"$module_root/include")
"$cxx" "${host_flags[@]}" -c "$module_root/src/provider.cc" -o "$stage/provider.o"
"$ar" rcs "$stage/libaim-dl-phdr.a" "$stage/provider.o"
file "$stage/provider.o" | grep -F 'Mach-O 64-bit object arm64' >/dev/null ||
  fail "provider object is not Darwin arm64"
for symbol in aim_dl_phdr_bind_source aim_bionic_dl_iterate_phdr aim_dl_phdr_resolve; do
  "$host_nm" -gU "$stage/provider.o" | awk '{print $3}' | grep -Fx "_$symbol" >/dev/null ||
    fail "missing provider definition: $symbol"
done
if "$host_nm" -u "$stage/provider.o" | grep -E '(_dyld_|_dlopen|_dlsym|_dlclose)' >/dev/null; then
  fail "forbidden dyld/dlsym fallback dependency"
fi

target="$stage/libdl-phdr-target.so"
helper="$stage/libdl-phdr-helper.so"
common_android=(-std=c17 -O2 -mno-outline-atomics -fPIC -fvisibility=hidden
                -Wall -Wextra -Werror -shared -nostdlib -fuse-ld=lld
                -Wl,--build-id=none -Wl,--hash-style=sysv -Wl,-z,now
                -Wl,-z,norelro -Wl,-z,max-page-size=16384)
"$android_clang" "${common_android[@]}" \
  -Wl,-soname,libdl-phdr-target.so \
  -Wl,--version-script,"$module_root/fixture/target.map" \
  "$module_root/fixture/target.c" "$libdl_stub" -o "$target"
"$android_clang" "${common_android[@]}" \
  -Wl,-soname,libdl-phdr-helper.so \
  -Wl,--version-script,"$module_root/fixture/helper.map" \
  "$module_root/fixture/helper.c" -o "$helper"
for elf in "$target" "$helper"; do
  file "$elf" | grep -F 'ELF 64-bit LSB shared object, ARM aarch64' >/dev/null ||
    fail "fixture is not Android arm64 ELF: $elf"
done
[[ "$(sha "$target")" == "$TARGET_ELF_SHA256" ]] || fail "target ELF SHA mismatch"
[[ "$(sha "$helper")" == "$HELPER_ELF_SHA256" ]] || fail "helper ELF SHA mismatch"
[[ "$("$readelf" -d "$target" | awk '/\(NEEDED\)/{gsub(/[][]/,"",$NF); print $NF}')" == "libdl.so" ]] ||
  fail "target dependency is not exactly libdl.so"
[[ -z "$("$readelf" -d "$helper" | awk '/\(NEEDED\)/{print}')" ]] ||
  fail "helper unexpectedly has a DT_NEEDED dependency"
[[ "$("$readelf" --dyn-syms --wide "$target" | awk '$7=="UND" && $5=="GLOBAL"{print $8}')" == "dl_iterate_phdr@LIBC" ]] ||
  fail "target undefined/version manifest mismatch"
[[ "$("$elf_nm" -D --defined-only "$target" | awk '$2=="T"{n++}END{print n+0}')" == 3 ]] ||
  fail "target export count mismatch"
[[ "$("$elf_nm" -D --defined-only "$helper" | awk '$2=="T"{n++}END{print n+0}')" == 1 ]] ||
  fail "helper export count mismatch"
target_value="$("$elf_nm" -D --defined-only "$target" | awk '$3=="phdr_fixture_run"{sub(/^0+/,"",$1); print "0x"$1}')"
helper_value="$("$elf_nm" -D --defined-only "$helper" | awk '$3=="phdr_helper_marker"{sub(/^0+/,"",$1); print "0x"$1}')"
[[ "$target_value" == "$TARGET_RUN_SYMBOL_VALUE" ]] || fail "target symbol value drift"
[[ "$helper_value" == "$HELPER_MARKER_SYMBOL_VALUE" ]] || fail "helper symbol value drift"

export AIM_DL_PHDR_PROVIDER_LIBDIR="$stage"
export CARGO_TARGET_DIR="$stage/cargo-target"
cargo build --quiet --locked --manifest-path "$runner_root/Cargo.toml" --features provider-link
runner="$CARGO_TARGET_DIR/debug/android-dl-iterate-phdr-runner"
output="$("$runner" "$target" "$helper" "$target_value" "$helper_value")"
grep -F 'ELF-callback=PT_LOAD+SONAME records=2->1' <<< "$output" >/dev/null ||
  fail "Android callback did not inspect both real images"
grep -F 'snapshot=leased concurrent-unpublish=stable reentrant=max2 early-stop=37' <<< "$output" >/dev/null ||
  fail "snapshot/reentrancy/early-stop contract failed"
grep -F 'resolver=libdl.so@LIBC-only negatives=3 dyld-fallback=0 adds=2 subs=1 info-size=64' <<< "$output" >/dev/null ||
  fail "closed resolver/counter/layout contract failed"

mkdir -p "$build_dir"
cp "$stage/libaim-dl-phdr.a" "$build_dir/"
cp "$target" "$helper" "$runner" "$build_dir/"
printf '%s\n' "$output"
echo "dl-iterate-phdr-provider: PASS images=2 Android-ELF=arm64 host=Darwin-arm64 resolver=closed runtime-files-modified=0"
