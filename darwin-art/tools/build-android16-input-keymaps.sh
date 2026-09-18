#!/bin/bash
# Build pinned original Android keyboard-map and framework JNI owners.
# The downstream product graph and registration own integration separately.
set -euo pipefail
export LC_ALL=C
project_root="$(cd "$(dirname "$0")/.." && pwd)"
bash "$project_root/tools/materialize-android16-key-character-map.sh"
native_root="$project_root/_aosp/android16-key-character-map/frameworks/native"
task_stage="$(mktemp -d "${TMPDIR:-/tmp}/darwin-art-input-keymaps.XXXXXX")"
trap 'rm -rf -- "$task_stage"' EXIT
source "$project_root/tools/lib/key-character-map-compile-context.sh"
source "$project_root/tools/lib/key-character-map-jni-compile-context.sh"
objects=()
for unit in KeyCharacterMap InputEventLabels Keyboard Input InputDevice KeyLayoutMap PropertyMap; do
  object="$task_stage/$unit.o"
  xcrun clang++ "${kcm_flags[@]}" -c "$native_root/libs/input/$unit.cpp" -o "$object"
  objects+=("$object")
done
for unit in android_view_KeyCharacterMap android_view_KeyEvent; do
  object="$task_stage/$unit.o"
  xcrun clang++ "${kcm_jni_flags[@]}" -c "$kcm_jni_root/$unit.cpp" -o "$object"
  objects+=("$object")
done
# Narrow guest-filesystem factory uses the matched original map ABI.
object="$task_stage/system_keyboard_maps_jni.o"
xcrun clang++ "${kcm_jni_flags[@]}" -c \
  "$project_root/runtime/framework/input/system_keyboard_maps_jni.cc" -o "$object"
objects+=("$object")
# Fresh archive publication prevents obsolete members surviving a removed unit.
ZERO_AR_DATE=1 xcrun ar rcs "$task_stage/libandroid-input-keymaps.a" "${objects[@]}"
output_root="$project_root/_build/android16-input-keymaps"
mkdir -p "$output_root"
if [[ -f "$output_root/libandroid-input-keymaps.a" ]] &&
   cmp -s "$task_stage/libandroid-input-keymaps.a" "$output_root/libandroid-input-keymaps.a"; then
  echo 'android16-input-keymaps: original archive unchanged'
else
  mv "$task_stage/libandroid-input-keymaps.a" "$output_root/libandroid-input-keymaps.a"
fi
echo 'android16-input-keymaps: PASS original native/JNI archive; product integration separate'
