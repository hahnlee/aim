#!/bin/bash
set -euo pipefail
project_root="$(cd "$(dirname "$0")/../.." && pwd)"
bash "$project_root/tools/materialize-android16-key-character-map.sh"
native_root="$project_root/_aosp/android16-key-character-map/frameworks/native"
task_stage="$(mktemp -d "${TMPDIR:-/tmp}/darwin-art-kcm-jni.XXXXXX")"
trap 'rm -rf -- "$task_stage"' EXIT
source "$project_root/tools/lib/key-character-map-compile-context.sh"
source "$project_root/tools/lib/key-character-map-jni-compile-context.sh"
for unit in android_view_KeyCharacterMap android_view_KeyEvent; do
  xcrun clang++ "${kcm_jni_flags[@]}" -c "$kcm_jni_root/$unit.cpp" \
    -o "$task_stage/$unit.o"
done
xcrun clang++ "${kcm_jni_flags[@]}" -c \
  "$project_root/runtime/framework/input/system_keyboard_maps_jni.cc" \
  -o "$task_stage/system_keyboard_maps_jni.o"
echo 'aosp-key-character-map: PASS original JNI and factory object compilation only'
# Does not establish product linking, native registration, ART or physical input.
