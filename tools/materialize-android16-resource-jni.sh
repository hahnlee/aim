#!/bin/bash
set -euo pipefail

script_dir="$(cd "$(dirname "$0")" && pwd)"
project_root="$(cd "$script_dir/.." && pwd)"
lock_file="$project_root/upstream/android16-resource-jni.lock"
destination="$project_root/_aosp/frameworks/base/core/jni"

# shellcheck disable=SC1090
source "$lock_file"

sha256() { shasum -a 256 "$1" | awk '{print $1}'; }

materialize() {
  local relative="$1" expected="$2"
  local output="$destination/$relative"
  if [[ -f "$output" && "$(sha256 "$output")" == "$expected" ]]; then
    return
  fi
  mkdir -p "$(dirname "$output")"
  local stage
  stage="$(mktemp "${output}.download.XXXXXX")"
  curl -fsSL \
    "https://android.googlesource.com/$FRAMEWORKS_BASE_PROJECT/+/$FRAMEWORKS_BASE_REVISION/core/jni/$relative?format=TEXT" \
    | base64 -D > "$stage"
  local actual
  actual="$(sha256 "$stage")"
  if [[ "$actual" != "$expected" ]]; then
    echo "resource-jni materialize: checksum mismatch: $relative" >&2
    echo "expected=$expected actual=$actual" >&2
    exit 3
  fi
  mv "$stage" "$output"
}

materialize Android.bp "$ANDROID_BP_SHA256"
materialize AndroidRuntime.cpp "$ANDROID_RUNTIME_CPP_SHA256"
materialize android_content_res_ApkAssets.cpp "$APK_ASSETS_CPP_SHA256"
materialize android_content_res_ApkAssets.h "$APK_ASSETS_H_SHA256"
materialize android_util_AssetManager.cpp "$ASSET_MANAGER_CPP_SHA256"
materialize android_util_StringBlock.cpp "$STRING_BLOCK_CPP_SHA256"
materialize android_util_XmlBlock.cpp "$XML_BLOCK_CPP_SHA256"
materialize core_jni_helpers.h "$CORE_JNI_HELPERS_SHA256"
materialize jni_wrappers.h "$JNI_WRAPPERS_SHA256"
materialize include/android_runtime/AndroidRuntime.h "$ANDROID_RUNTIME_H_SHA256"
materialize include/android_runtime/android_util_AssetManager.h \
  "$ANDROID_UTIL_ASSET_MANAGER_H_SHA256"

printf '%s\n' "$FRAMEWORKS_BASE_REVISION" > "$destination/.source-revision"
echo "resource-jni materialize: revision=$FRAMEWORKS_BASE_REVISION files=11 git-metadata=0"
