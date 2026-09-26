#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
runtime_abi="darwin-art-darwin-native-v1"
apk="$root/_build/simple-apk-fixture/simple-jni.apk"
# The fixture is installed through PackageManagerService in its own profile.
export DARWIN_ART_PROFILE="${DARWIN_ART_PROFILE:-native-audit}"
cache_root="$(mktemp -d "${TMPDIR:-/tmp}/darwin-art-native-cache.XXXXXX")"
fallback_cache_root="$(mktemp -d "${TMPDIR:-/tmp}/darwin-art-native-fallback-cache.XXXXXX")"
converter="$(mktemp "${TMPDIR:-/tmp}/darwin-art-native-converter.XXXXXX")"
run_log="$(mktemp "${TMPDIR:-/tmp}/darwin-art-complete-darwin.XXXXXX")"
fallback_run_log="$(mktemp "${TMPDIR:-/tmp}/darwin-art-native-fallback.XXXXXX")"
logical="libdarwin-art-simple-jni.so"
dylib="libdarwin-art-simple-jni.dylib"
child_dylib="libdarwin-art-simple-zchild.dylib"

cleanup() {
  chmod -R u+w "$cache_root" "$fallback_cache_root" 2>/dev/null || true
  rm -rf "$cache_root" "$fallback_cache_root"
  rm -f "$converter" "$run_log" "$fallback_run_log"
}
trap cleanup EXIT

cat >"$converter" <<'CONVERTER'
#!/bin/sh
set -eu
output=
while [ "$#" -gt 0 ]; do
  case "$1" in
    --output-directory) output=$2 ;;
  esac
  shift 2
done
: "${output:?missing output directory}"
: "${DARWIN_ART_FIXTURE_ROOT:?missing fixture root}"
xcrun clang -std=c17 -O2 -fPIC -fvisibility=hidden -Wall -Wextra -Werror \
  -arch arm64 -dynamiclib \
  -I "$DARWIN_ART_FIXTURE_ROOT/_aosp/libnativehelper/include_jni" \
  -Wl,-install_name,@loader_path/libdarwin-art-simple-zchild.dylib \
  "$DARWIN_ART_FIXTURE_ROOT/tools/fixtures/simple-apk/native_child.c" \
  -o "$output/libdarwin-art-simple-zchild.dylib"
xcrun clang -std=c17 -O2 -fPIC -fvisibility=hidden -Wall -Wextra -Werror \
  -arch arm64 -dynamiclib \
  -I "$DARWIN_ART_FIXTURE_ROOT/_aosp/libnativehelper/include_jni" \
  -Wl,-install_name,@loader_path/libdarwin-art-simple-jni.dylib \
  -L "$output" -ldarwin-art-simple-zchild \
  "$DARWIN_ART_FIXTURE_ROOT/tools/fixtures/simple-apk/native_app.c" \
  -o "$output/libdarwin-art-simple-jni.dylib"
CONVERTER
chmod 0700 "$converter"

"$root/tools/fixtures/simple-apk/build.sh" >/dev/null
cargo build -q --release -p darwin-art-native-artifact --bin darwin-art-native-resolve
resolver="$root/target/release/darwin-art-native-resolve"

# Install through PackageManagerService; NativeLibraryHelper extracts the
# Android graph into the package's nativeLibraryDir.
install_log="$("$root/tools/run-android-apk-app.sh" --install "$apk")"
package="$(sed -n 's/^darwin-art: installed package=//p' <<<"$install_log")"
[[ "$package" == dev.darwinart.simple ]]
ctl="$root/target/release/darwin-artctl"
mount="$("$ctl" ensure)"
source "$root/tools/lib/package-manager-launch.sh"
darwin_art_pm_launcher_info "$ctl" "$mount" "$package"
elf_directory="$pm_native_library_dir"
[[ -f "$elf_directory/$logical" ]]
apk_sha="$(shasum -a 256 "$pm_source_dir" | awk '{print $1}')"
cache="$cache_root/$apk_sha/$runtime_abi"

resolution="$(DARWIN_ART_FIXTURE_ROOT="$root" "$resolver" \
  "$apk_sha" "$runtime_abi" "$elf_directory" "$cache" "$converter")"
grep -F 'native_backend=darwin conversion=published:2' <<<"$resolution" >/dev/null
grep -F 'backend=darwin libraries=2' <<<"$resolution" >/dev/null
file "$cache/$dylib" | grep -F 'Mach-O 64-bit dynamically linked shared library arm64' >/dev/null
file "$cache/$child_dylib" | grep -F 'Mach-O 64-bit dynamically linked shared library arm64' >/dev/null
otool -L "$cache/$dylib" | grep -F '/usr/lib/libSystem.B.dylib' >/dev/null
otool -L "$cache/$dylib" | grep -F '@loader_path/libdarwin-art-simple-zchild.dylib' >/dev/null
! otool -L "$cache/$dylib" | grep -F '.so' >/dev/null

if ! DARWIN_ART_APK_MANAGED_NATIVE_LOAD=0 \
  DARWIN_ART_NATIVE_CACHE_ROOT="$cache_root" \
  DARWIN_ART_NATIVE_CONVERTER="$converter" \
  DARWIN_ART_FIXTURE_ROOT="$root" \
  "$root/tools/run-android-apk-app.sh" --package "$package" 0 >"$run_log" 2>&1; then
  cat "$run_log" >&2
  exit 1
fi
grep -F 'DARWIN native loader: complete graph root=' "$run_log" >/dev/null
grep -F 'ART Android APK JNI: JavaVMExt+NativeBridge load ok' "$run_log" >/dev/null

# A converter that exits successfully without the exact output graph is still
# an incomplete conversion: its private stage is deleted, one graph-level ELF
# decision is cached, and the runtime loads the complete original Android
# graph without considering any Darwin member.
fallback="$fallback_cache_root/$apk_sha/$runtime_abi"
fallback_first="$("$resolver" "$apk_sha" "$runtime_abi" "$elf_directory" "$fallback" /usr/bin/true)"
grep -F 'native_backend=elf conversion=attempted:incomplete-conversion:2' \
  <<<"$fallback_first" >/dev/null
fallback_second="$("$resolver" "$apk_sha" "$runtime_abi" "$elf_directory" "$fallback" /usr/bin/true)"
grep -F 'native_backend=elf conversion=cached:cached-incomplete-conversion:2' \
  <<<"$fallback_second" >/dev/null
[[ ! -d "$fallback" ]]
[[ -f "$fallback.elf-fallback" ]]

if ! DARWIN_ART_APK_MANAGED_NATIVE_LOAD=0 \
  DARWIN_ART_NATIVE_CACHE_ROOT="$fallback_cache_root" \
  DARWIN_ART_NATIVE_CONVERTER=/usr/bin/true \
  "$root/tools/run-android-apk-app.sh" --package "$package" 0 >"$fallback_run_log" 2>&1; then
  cat "$fallback_run_log" >&2
  exit 1
fi
grep -F 'native_backend=elf conversion=cached:cached-incomplete-conversion:2' \
  "$fallback_run_log" >/dev/null
grep -F 'native-resolve: PASS backend=elf libraries=2' \
  "$fallback_run_log" >/dev/null
grep -F 'DARWIN ELF loader: graph loaded root=libdarwin-art-simple-jni.so sources=2' \
  "$fallback_run_log" >/dev/null
grep -F 'ART Android APK JNI: JavaVMExt+NativeBridge load ok' \
  "$fallback_run_log" >/dev/null
! grep -F 'DARWIN native loader: complete graph root=' "$fallback_run_log" >/dev/null

echo "$resolution"
echo "complete-darwin-apk-native: PASS apk=$apk_sha logical=$logical dyld=complete elf_fallback=complete atomic_graph=1 JNI_OnLoad=0x00010006 nativeAnswer=42"
