#!/bin/bash
set -euo pipefail
root="$(cd "$(dirname "$0")/../.." && pwd)"
host="${1:-$root/target/debug/darwin-art-host}"
source "$root/tools/lib/package-system-root.sh"
source "$root/tools/lib/runtime-system-image.sh"
stage="$(mktemp -d "${TMPDIR:-/tmp}/system-image-install.XXXXXX")"
cleanup() {
  find "$stage" -type d -exec chmod u+w {} +
  rm -rf -- "$stage"
}
trap cleanup EXIT
archive="$stage/system-root.tar"
native_baseline="$(bash "$root/tools/prepare-runtime-system-root.sh")"
darwin_art_package_system_root "$native_baseline" "$archive" \
  "$root/_build/android16-system-fonts" "$root/_prebuilt/android-16/resources/framework-res.apk"
first="$("$host" --prepare-system-image "$archive" "$stage/store")"
inode="$(stat -f %i "$first")"
second="$("$host" --prepare-system-image "$archive" "$stage/store")"
[[ "$first" == "$second" && "$(stat -f %i "$second")" == "$inode" ]]
# Exercise the launch selector with the actual installer, not a fake host.
# Hardlinks avoid making additional archive copies in the temporary fixture.
mkdir -p "$stage/runtime/android" "$stage/runtime/_build/android-system-image"
ln "$archive" "$stage/runtime/android/system-root.tar"
ln "$archive" "$stage/runtime/_build/android-system-image/system-root.tar"
for mode in packaged development; do
  selected="$(darwin_art_prepare_runtime_system_image "$stage/runtime" "$host" "$mode" "$stage/store")"
  [[ "$selected" == "$first" && "$(stat -f %i "$selected")" == "$inode" ]]
done
if darwin_art_prepare_runtime_system_image "$stage/runtime" "$host" invalid "$stage/store"; then exit 1; fi
rm "$stage/runtime/android/system-root.tar"
# Packaged launch must not silently use the available development artifact.
if darwin_art_prepare_runtime_system_image "$stage/runtime" "$host" packaged "$stage/store"; then exit 1; fi
ln -s "$archive" "$stage/runtime/android/system-root.tar"
if darwin_art_prepare_runtime_system_image "$stage/runtime" "$host" packaged "$stage/store"; then exit 1; fi
cmp "$native_baseline/system/lib64/libc++.so" "$first/system/lib64/libc++.so"
cmp "$root/_prebuilt/android-16/resources/framework-res.apk" "$first/system/framework/framework-res.apk"
diff -qr "$root/_build/android16-system-fonts/system/fonts" "$first/system/fonts"
[[ -z "$(find "$first" \( -type f -o -type d \) -perm +222 -print -quit)" ]]
"$host" --prepare-system-image "$archive" "$stage/race" > "$stage/first" &
first_pid=$!
"$host" --prepare-system-image "$archive" "$stage/race" > "$stage/second" &
second_pid=$!
wait "$first_pid"
wait "$second_pid"
cmp "$stage/first" "$stage/second"
[[ -z "$(find "$stage/race" -name '.install-*' -print -quit)" ]]
ln -s "$archive" "$stage/link.tar"
if "$host" --prepare-system-image "$stage/link.tar" "$stage/store"; then exit 1; fi
if "$host" --prepare-system-image "$root/tools/tests/runtime-signing-fixture.c" "$stage/invalid"; then exit 1; fi
[[ -z "$(find "$stage/invalid" -name '.install-*' -print -quit)" ]]
# The former linker-only artifact is not sufficient for app startup.
/usr/bin/tar -cf "$stage/incomplete.tar" -C "$native_baseline" apex system linkerconfig
if "$host" --prepare-system-image "$stage/incomplete.tar" "$stage/incomplete-store"; then exit 1; fi
[[ -z "$(find "$stage/incomplete-store" -name '.install-*' -print -quit)" ]]
receipt="${first%/root}/receipt"
chmod u+w "$receipt"
printf '%s' bad > "$receipt"
if "$host" --prepare-system-image "$archive" "$stage/store"; then exit 1; fi
[[ "$(cat "$receipt")" == bad && -d "$first" ]]
echo 'system image install: original bytes, read-only reuse, concurrent publication, failed-stage cleanup, corruption preserved/rejected PASS'
