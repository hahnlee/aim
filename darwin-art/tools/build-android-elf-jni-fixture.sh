#!/bin/bash
set -euo pipefail
export LC_ALL=C

project_root="$(cd "$(dirname "$0")/.." && pwd)"
fixture_root="$project_root/probes/android-elf-jni-fixture"
build_dir="$project_root/_build/android-elf-jni-fixture"
sdk_root="${ANDROID_SDK_ROOT:-$HOME/Library/Android/sdk}"
ndk_revision="28.2.13676358"
ndk="$sdk_root/ndk/$ndk_revision"
toolchain="$ndk/toolchains/llvm/prebuilt/darwin-x86_64"
android_clang="$toolchain/bin/aarch64-linux-android35-clang"
readelf="$toolchain/bin/llvm-readelf"
nm="$toolchain/bin/llvm-nm"
objdump="$toolchain/bin/llvm-objdump"
host_clang="$(xcrun --find clang)"
host_objdump="$(xcrun --find llvm-objdump)"

fail() { echo "android-elf-jni-fixture: $*" >&2; exit 3; }
missing() { echo "android-elf-jni-fixture: missing $*" >&2; exit 2; }
publish_if_changed() {
  local source="$1"
  local destination="$2"
  if [[ -f "$destination" ]] && cmp -s "$source" "$destination"; then
    return
  fi
  cp "$source" "$destination"
}

for tool in "$android_clang" "$readelf" "$nm" "$objdump" "$host_clang" "$host_objdump"; do
  [[ -x "$tool" ]] || missing "$tool"
done

stage="$(mktemp -d "${TMPDIR:-/tmp}/android-elf-jni-fixture.XXXXXX")"
trap 'rm -rf "$stage"' EXIT
output="$stage/libdarwin-art-jni-fixture.so"
child="$stage/libdarwin-art-jni-child.so"
grandchild="$stage/libdarwin-art-jni-grandchild.so"
host_provider="$stage/libdarwin-art-jni-host.so"
generic_root="$stage/libdarwin-art-generic-root.so"
generic_child="$stage/libdarwin-art-generic-child.so"
generic_grandchild="$stage/libdarwin-art-generic-grandchild.so"

common_flags=(-std=c17 -O2 -fPIC -fvisibility=hidden -fno-builtin -Wall -Wextra -Werror
  -shared -nostdlib -fuse-ld=lld -Wl,--build-id=none -Wl,--hash-style=sysv
  -Wl,-z,now -Wl,-z,norelro -Wl,-z,max-page-size=16384)

"$android_clang" "${common_flags[@]}" -Wl,-soname,libdarwin-art-jni-host.so \
  "$fixture_root/host_provider.c" -lc -o "$host_provider"
"$android_clang" "${common_flags[@]}" \
  -Wl,-soname,libdarwin-art-jni-grandchild.so \
  -Wl,--version-script,"$fixture_root/grandchild.exports.map" \
  "$fixture_root/grandchild.c" -o "$grandchild"
"$android_clang" "${common_flags[@]}" \
  -Wl,-soname,libdarwin-art-jni-child.so \
  -Wl,--version-script,"$fixture_root/child.exports.map" \
  "$fixture_root/child.c" "$grandchild" "$host_provider" -lc -o "$child"
"$android_clang" "${common_flags[@]}" \
  -Wl,-soname,libdarwin-art-jni-fixture.so \
  -Wl,--version-script,"$fixture_root/exports.map" \
  "$fixture_root/native_fixture.c" "$child" "$host_provider" -lc -o "$output"

"$android_clang" "${common_flags[@]}" \
  -Wl,-soname,libdarwin-art-generic-grandchild.so \
  -Wl,--version-script,"$fixture_root/generic_grandchild.exports.map" \
  "$fixture_root/generic_grandchild.c" -o "$generic_grandchild"
"$android_clang" "${common_flags[@]}" \
  -Wl,-soname,libdarwin-art-generic-child.so \
  -Wl,--version-script,"$fixture_root/generic_child.exports.map" \
  "$fixture_root/generic_child.c" "$generic_grandchild" -o "$generic_child"
"$android_clang" "${common_flags[@]}" \
  -Wl,-soname,libdarwin-art-generic-root.so \
  -Wl,--version-script,"$fixture_root/generic_root.exports.map" \
  "$fixture_root/generic_root.c" "$generic_child" -o "$generic_root"

for elf in "$output" "$child"; do
  file "$elf" | grep -F 'ELF 64-bit LSB shared object, ARM aarch64' >/dev/null ||
    fail "output is not AArch64 ELF: $elf"
  "$readelf" -l "$elf" | grep -F 'GNU_RELRO' >/dev/null &&
    fail "graph fixture requires RELRO to remain disabled"
  "$readelf" -l "$elf" | grep -E '^  TLS ' >/dev/null &&
    fail "graph fixture unexpectedly uses TLS"
  "$readelf" -d "$elf" | grep -F 'BIND_NOW' >/dev/null ||
    fail "graph fixture must request immediate binding"
  "$readelf" -d "$elf" | grep -F '(INIT_ARRAY)' >/dev/null ||
    fail "graph fixture lost its constructor"
  "$readelf" -d "$elf" | grep -F '(FINI_ARRAY)' >/dev/null ||
    fail "graph fixture lost its finalizer"
done
root_dynamic="$stage/root.dynamic.txt"
child_dynamic="$stage/child.dynamic.txt"
grandchild_dynamic="$stage/grandchild.dynamic.txt"
"$readelf" -d -r "$output" > "$root_dynamic"
"$readelf" -d -r "$child" > "$child_dynamic"
"$readelf" -d -r "$grandchild" > "$grandchild_dynamic"
[[ "$(grep -c '(NEEDED)' "$root_dynamic")" == 3 ]] ||
  fail "root DT_NEEDED closure is not exactly child+host-provider+libc"
grep -E '\(NEEDED\).*libdarwin-art-jni-child\.so' "$root_dynamic" >/dev/null ||
  fail "root lost child dependency"
grep -E '\(NEEDED\).*libdarwin-art-jni-host\.so' "$root_dynamic" >/dev/null ||
  fail "root lost explicit virtual provider dependency"
grep -E '\(NEEDED\).*libc\.so' "$root_dynamic" >/dev/null ||
  fail "root lost closed Bionic provider dependency"
[[ "$(grep -c '(NEEDED)' "$child_dynamic")" == 3 ]] ||
  fail "child DT_NEEDED closure is not exactly grandchild+host-provider+libc"
grep -E '\(NEEDED\).*libdarwin-art-jni-grandchild\.so' "$child_dynamic" >/dev/null ||
  fail "child lost grandchild dependency"
grep -E '\(NEEDED\).*libdarwin-art-jni-host\.so' "$child_dynamic" >/dev/null ||
  fail "child lost explicit virtual provider dependency"
grep -E '\(NEEDED\).*libc\.so' "$child_dynamic" >/dev/null ||
  fail "child lost Bionic lifecycle provider dependency"
[[ "$(grep -c '(NEEDED)' "$grandchild_dynamic" || true)" == 0 ]] ||
  fail "grandchild unexpectedly gained a dependency"
grep -E 'R_AARCH64_JUMP_SLOT.*DarwinArtFixtureGrandchildValue' "$child_dynamic" >/dev/null ||
  fail "child grandchild-symbol relocation missing"
grep -E 'R_AARCH64_JUMP_SLOT.*DarwinArtFixtureChildValue' "$root_dynamic" >/dev/null ||
  fail "root child-symbol relocation missing"
[[ "$("$nm" -D --defined-only "$output" | awk '$2 == "T" {print $3}' | paste -sd, -)" == \
   'JNI_OnLoad,JNI_OnUnload' ]] || fail "unexpected dynamic exports"
[[ "$("$nm" -D --defined-only "$child" | awk '$2 == "T" {print $3}' | paste -sd, -)" == \
   'DarwinArtFixtureChildValue' ]] || fail "unexpected child dynamic exports"
[[ "$("$nm" -D --defined-only "$grandchild" | awk '$2 == "T" {print $3}' | paste -sd, -)" == \
   'DarwinArtFixtureGrandchildValue' ]] || fail "unexpected grandchild dynamic exports"

generic_root_dynamic="$stage/generic-root.dynamic.txt"
generic_child_dynamic="$stage/generic-child.dynamic.txt"
generic_grandchild_dynamic="$stage/generic-grandchild.dynamic.txt"
"$readelf" -d -r "$generic_root" > "$generic_root_dynamic"
"$readelf" -d -r "$generic_child" > "$generic_child_dynamic"
"$readelf" -d -r "$generic_grandchild" > "$generic_grandchild_dynamic"
[[ "$(grep -c '(NEEDED)' "$generic_root_dynamic")" == 1 ]] &&
  grep -E '\(NEEDED\).*libdarwin-art-generic-child\.so' "$generic_root_dynamic" >/dev/null ||
  fail "generic root dependency drift"
[[ "$(grep -c '(NEEDED)' "$generic_child_dynamic")" == 1 ]] &&
  grep -E '\(NEEDED\).*libdarwin-art-generic-grandchild\.so' "$generic_child_dynamic" >/dev/null ||
  fail "generic child dependency drift"
[[ "$(grep -c '(NEEDED)' "$generic_grandchild_dynamic" || true)" == 0 ]] ||
  fail "generic grandchild dependency drift"
[[ "$("$nm" -D --defined-only "$generic_root" | awk '$2 == "T" {print $3}' | paste -sd, -)" == \
   'JNI_OnLoad' ]] || fail "generic root must export JNI_OnLoad without JNI_OnUnload"
! grep -E '__errno|strlen|libc\.so|libdl\.so|liblog\.so|libm\.so' \
  "$generic_root_dynamic" "$generic_child_dynamic" "$generic_grandchild_dynamic" >/dev/null ||
  fail "generic graph unexpectedly depends on a Bionic provider"

relocations="$stage/relocations.txt"
"$readelf" -r "$output" > "$relocations"
if awk '/R_AARCH64_/ && $3 != "R_AARCH64_RELATIVE" &&
                         $3 != "R_AARCH64_ABS64" &&
                         $3 != "R_AARCH64_GLOB_DAT" &&
                         $3 != "R_AARCH64_JUMP_SLOT" {bad=1}
        END {exit bad}' "$relocations"; then
  :
else
  fail "fixture has an unsupported relocation"
fi
grep -E 'R_AARCH64_JUMP_SLOT.*(DarwinArtFixtureChildValue|darwin_art_fixture_record_lifecycle)' \
  "$relocations" >/dev/null || fail "graph imports did not produce eager PLT relocations"
for symbol in __errno close ioctl open read sendfile sscanf strftime_l strlen swprintf vsscanf; do
  grep -E "R_AARCH64_JUMP_SLOT.*${symbol}" "$relocations" >/dev/null ||
    fail "Bionic provider relocation missing: $symbol"
done
for dynamic in "$root_dynamic" "$child_dynamic"; do
  grep -E 'R_AARCH64_JUMP_SLOT.*__cxa_atexit' "$dynamic" >/dev/null ||
    fail "Bionic per-DSO lifecycle relocation missing: $dynamic"
done
for elf in "$output" "$child"; do
  "$readelf" --dyn-syms --wide "$elf" |
    awk '$7 != "UND" && $8 == "__dso_handle" { found=1 } END { exit found }' ||
    fail "__dso_handle escaped into the dynamic symbol table: $elf"
done

disassembly="$stage/disassembly.txt"
"$objdump" -d "$output" > "$disassembly"
spill_disassembly="$stage/native-spill-disassembly.txt"
sed -n '/<NativeSpill>:/,/^$/p' "$disassembly" > "$spill_disassembly"
[[ -s "$spill_disassembly" ]] || fail "NativeSpill was not retained for PCS audit"
grep -Eq 'ldr[[:space:]]+x[0-9]+, \[sp\]$' "$spill_disassembly" ||
  fail "NativeSpill Android PCS reference is not at sp+0"
grep -Eq 'ldr[[:space:]]+w[0-9]+, \[sp, #0x8\]$' "$spill_disassembly" ||
  fail "NativeSpill Android PCS f4 is not at sp+8"
grep -Eq 'ldr[[:space:]]+w[0-9]+, \[sp, #0x10\]$' "$spill_disassembly" ||
  fail "NativeSpill Android PCS f5 is not at sp+16"
grep -Eq 'ldr[[:space:]]+x[0-9]+, \[sp, #0x18\]$' "$spill_disassembly" ||
  fail "NativeSpill Android PCS d4 is not at sp+24"
spill_stack_loads="$(grep -Ec 'ldr[[:space:]]+[wx][0-9]+, \[sp(, #[^]]+)?\]$' "$spill_disassembly")"
[[ "$spill_stack_loads" == 4 ]] ||
  fail "NativeSpill has $spill_stack_loads stack loads, expected exactly 4"

darwin_object="$stage/native-fixture-darwin.o"
"$host_clang" -arch arm64 -std=c17 -O2 -fvisibility=hidden \
  -I"$toolchain/sysroot/usr/include" -c "$fixture_root/native_fixture.c" \
  -o "$darwin_object"
darwin_disassembly="$stage/darwin-disassembly.txt"
"$host_objdump" -d "$darwin_object" > "$darwin_disassembly"
darwin_spill="$stage/native-spill-darwin-disassembly.txt"
sed -n '/<_NativeSpill>:/,/^$/p' "$darwin_disassembly" > "$darwin_spill"
[[ -s "$darwin_spill" ]] || fail "Darwin NativeSpill was not retained for PCS audit"
grep -Eq 'ldr[[:space:]]+x[0-9]+, \[sp\]$' "$darwin_spill" ||
  fail "NativeSpill Darwin PCS reference is not at sp+0"
grep -Eq 'ldp[[:space:]]+w[0-9]+, w[0-9]+, \[sp, #0x8\]$' "$darwin_spill" ||
  fail "NativeSpill Darwin PCS f4/f5 pair is not at sp+8/sp+12"
grep -Eq 'ldr[[:space:]]+x[0-9]+, \[sp, #0x10\]$' "$darwin_spill" ||
  fail "NativeSpill Darwin PCS d4 is not at sp+16"
darwin_stack_loads="$(grep -Ec '(ldr[[:space:]]+[wx][0-9]+|ldp[[:space:]]+w[0-9]+, w[0-9]+), \[sp(, #[^]]+)?\]$' "$darwin_spill")"
[[ "$darwin_stack_loads" == 3 ]] ||
  fail "Darwin NativeSpill has $darwin_stack_loads stack loads, expected exactly 3"

mkdir -p "$build_dir"
publish_if_changed "$output" "$build_dir/libdarwin-art-jni-fixture.so"
publish_if_changed "$child" "$build_dir/libdarwin-art-jni-child.so"
publish_if_changed "$grandchild" "$build_dir/libdarwin-art-jni-grandchild.so"
publish_if_changed "$host_provider" "$build_dir/libdarwin-art-jni-host.so"
publish_if_changed "$generic_root" "$build_dir/libdarwin-art-generic-root.so"
publish_if_changed "$generic_child" "$build_dir/libdarwin-art-generic-child.so"
publish_if_changed "$generic_grandchild" "$build_dir/libdarwin-art-generic-grandchild.so"
fixture_sha="$(shasum -a 256 "$output" | awk '{print $1}')"
fixture_size="$(stat -f '%z' "$output")"
child_sha="$(shasum -a 256 "$child" | awk '{print $1}')"
child_size="$(stat -f '%z' "$child")"
grandchild_sha="$(shasum -a 256 "$grandchild" | awk '{print $1}')"
grandchild_size="$(stat -f '%z' "$grandchild")"
generated_dir="$build_dir/generated"
mkdir -p "$generated_dir"
identity="$stage/darwin_art_elf_jni_fixture_identity.h"
{
  echo '#ifndef DARWIN_ART_ELF_JNI_FIXTURE_IDENTITY_H_'
  echo '#define DARWIN_ART_ELF_JNI_FIXTURE_IDENTITY_H_'
  echo '#include <stddef.h>'
  echo "inline constexpr size_t kDarwinArtElfJniFixtureSize = ${fixture_size}u;"
  echo "inline constexpr char kDarwinArtElfJniFixtureSha256[] = \"${fixture_sha}\";"
  echo 'inline constexpr char kDarwinArtElfJniFixtureSoname[] = "libdarwin-art-jni-fixture.so";'
  echo 'inline constexpr char kDarwinArtElfJniChildFilename[] = "libdarwin-art-jni-child.so";'
  echo 'inline constexpr char kDarwinArtElfJniChildSoname[] = "libdarwin-art-jni-child.so";'
  echo "inline constexpr size_t kDarwinArtElfJniChildSize = ${child_size}u;"
  echo "inline constexpr char kDarwinArtElfJniChildSha256[] = \"${child_sha}\";"
  echo 'inline constexpr char kDarwinArtElfJniGrandchildFilename[] = "libdarwin-art-jni-grandchild.so";'
  echo 'inline constexpr char kDarwinArtElfJniGrandchildSoname[] = "libdarwin-art-jni-grandchild.so";'
  echo "inline constexpr size_t kDarwinArtElfJniGrandchildSize = ${grandchild_size}u;"
  echo "inline constexpr char kDarwinArtElfJniGrandchildSha256[] = \"${grandchild_sha}\";"
  echo 'inline constexpr char kDarwinArtElfJniHostProviderSoname[] = "libdarwin-art-jni-host.so";'
  echo 'inline constexpr char kDarwinArtElfJniFixtureSpillSignature[] ='
  echo '    "(ZBCSIJLjava/lang/Object;FDFDFDFDFFD)J";'
  echo 'inline constexpr size_t kDarwinArtElfJniFixtureAndroidRefStackOffset = 0u;'
  echo 'inline constexpr size_t kDarwinArtElfJniFixtureAndroidF4StackOffset = 8u;'
  echo 'inline constexpr size_t kDarwinArtElfJniFixtureAndroidF5StackOffset = 16u;'
  echo 'inline constexpr size_t kDarwinArtElfJniFixtureAndroidD4StackOffset = 24u;'
  echo 'inline constexpr size_t kDarwinArtElfJniFixtureDarwinRefStackOffset = 0u;'
  echo 'inline constexpr size_t kDarwinArtElfJniFixtureDarwinF4StackOffset = 8u;'
  echo 'inline constexpr size_t kDarwinArtElfJniFixtureDarwinF5StackOffset = 12u;'
  echo 'inline constexpr size_t kDarwinArtElfJniFixtureDarwinD4StackOffset = 16u;'
  echo '#endif'
} > "$identity"
publish_if_changed "$identity" "$generated_dir/darwin_art_elf_jni_fixture_identity.h"
echo "android-elf-jni-fixture: PASS graph=root+child+grandchild+virtual-provider+libc ctor=child-first cxa=root-first-before-fini fini=root-first bionic=__errno+strlen+open+read+close+sendfile+sscanf+vsscanf+swprintf+ioctl+strftime_l+__cxa_atexit random=ctor sendfile=/data-create-truncate+null+explicit-offset+reopen ioctl=RNDGETENTCNT scanf=binary128+va_list32 swprintf=Android-wchar32 strftime=UTC dso-handle=local exports=JNI_OnLoad+JNI_OnUnload relro=0 tls=0 register=generic1+fixture8+foreign-reject pcs=android(ref@0,f4@8,f5@16,d4@24)+darwin(ref@0,f4@8,f5@12,d4@16) root_size=$fixture_size root_sha256=$fixture_sha child_size=$child_size child_sha256=$child_sha grandchild_size=$grandchild_size grandchild_sha256=$grandchild_sha"
