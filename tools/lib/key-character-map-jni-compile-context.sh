# Extend the original input compilation boundary with matched framework JNI
# headers. Registration and device-map selection remain separate owners.
kcm_jni_root="$project_root/_aosp/android16-key-character-map/frameworks/base/core/jni"
# Keep acquired originals immutable. Apply only the pinned ownership handoff
# fix to a private build shadow shared by archive production and JNI tests.
source "$project_root/upstream/android16-key-character-map.lock"
kcm_handoff_patch="$project_root/patches/frameworks-base/0009-darwin-key-character-map-factory-handoff.patch"
[[ "$(shasum -a 256 "$kcm_handoff_patch" | awk '{print $1}')" == \
   "$DARWIN_KCM_FACTORY_HANDOFF_PATCH_SHA256" ]] || {
  echo 'key-character-map JNI: factory ownership patch checksum mismatch' >&2
  return 3
}
mkdir -p "$task_stage/frameworks-base/core"
cp -R "$kcm_jni_root" "$task_stage/frameworks-base/core/jni"
patch --batch --forward -p1 -d "$task_stage/frameworks-base" < "$kcm_handoff_patch"
kcm_jni_root="$task_stage/frameworks-base/core/jni"
kcm_jni_flags=(
  "${kcm_flags[@]}"
  "-ffile-prefix-map=$task_stage=$project_root/_build/android16-input-keymaps/compile"
  -I"$kcm_jni_root" -I"$kcm_jni_root/include"
  -I"$project_root/_build/nativehelper-foundation/source/libnativehelper/include_platform"
  -I"$project_root/_build/nativehelper-foundation/source/libnativehelper/include_platform_header_only"
  -include android-base/macros.h
)
