#!/bin/bash
set -euo pipefail

project_root="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$project_root"
# Verify every original input before compilation; no replacement parser or
# fabricated input enums are permitted in this component gate.
bash tools/materialize-android16-key-character-map.sh
native_root="$project_root/_aosp/android16-key-character-map/frameworks/native"
task_stage="$(mktemp -d "${TMPDIR:-/tmp}/darwin-art-kcm-compile.XXXXXX")"
trap 'rm -rf -- "$task_stage"' EXIT
source "$project_root/tools/lib/key-character-map-compile-context.sh"
# Upstream Input.h includes these generated contracts only on Linux, although
# its declarations use them on all hosts. Supply the ORIGINAL generated headers
# at the Darwin compilation boundary; do not alter upstream matching policy.
xcrun clang++ "${kcm_flags[@]}" \
  -fsyntax-only "$native_root/libs/input/KeyCharacterMap.cpp" \
  "$project_root/runtime/framework/input/system_keyboard_maps_jni.cc" \
  "$project_root/tools/tests/aosp-key-character-map-test.cc"
echo 'aosp-key-character-map: PASS original evaluator/factory/test compilation only'
# Deliberately not a link, evaluator execution, JNI round-trip, or APK gate.
