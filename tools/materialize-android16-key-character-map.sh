#!/bin/bash
set -euo pipefail

script_dir="$(cd "$(dirname "$0")" && pwd)"
project_root="$(cd "$script_dir/.." && pwd)"
lock_file="$project_root/upstream/android16-key-character-map.lock"
source_root="${ANDROID16_KCM_SOURCE_ROOT:-$project_root/_aosp/android16-key-character-map}"
artifact_root="${ANDROID16_KCM_ARTIFACT_ROOT:-$project_root/_build/android16-key-character-map}"

# shellcheck disable=SC1090
source "$lock_file"

sha256() { /usr/bin/shasum -a 256 "$1" | /usr/bin/awk '{print $1}'; }

fail() {
  echo "key-character-map materialize: $*" >&2
  exit 3
}

assert_safe_path() {
  local path="$1" current next
  [[ "$path" = /* ]] || fail "path must be absolute: $path"
  current="$path"
  while :; do
    if [[ -L "$current" ]]; then
      # macOS exposes the system temporary directory through this fixed alias;
      # validate the alias target, but reject every caller-controlled symlink.
      [[ "$current" == "/tmp" && "$(readlink "$current")" == "private/tmp" ]] || \
        fail "symlink path component: $current"
      current="/private/tmp"
    fi
    if [[ -e "$current" && ! -d "$current" ]]; then
      fail "non-directory path component: $current"
    fi
    [[ "$current" == "/" ]] && break
    next="$(dirname "$current")"
    [[ "$next" != "$current" ]] || break
    current="$next"
  done
}

assert_regular_output() {
  local output="$1"
  assert_safe_path "$(dirname "$output")"
  [[ ! -L "$output" ]] || fail "refusing symlink output: $output"
  if [[ -e "$output" && ! -f "$output" ]]; then
    fail "output is not a regular file: $output"
  fi
}

publish_new_file() {
  local temporary="$1" output="$2"
  assert_regular_output "$output"
  if [[ -e "$output" ]]; then
    [[ "$(sha256 "$output")" == "$(sha256 "$temporary")" ]] || \
      fail "existing output checksum mismatch: $output"
    /bin/rm -f "$temporary"
    return
  fi
  # The temporary file is in the destination directory. A hard link gives
  # create-without-replace semantics even if another process races us.
  /bin/ln "$temporary" "$output" || fail "output appeared during publication: $output"
  /bin/rm -f "$temporary"
}

materialize_remote() {
  local project="$1" revision="$2" relative="$3" expected="$4" output="$5"
  assert_regular_output "$output"
  if [[ -f "$output" ]]; then
    [[ "$(sha256 "$output")" == "$expected" ]] || \
      fail "existing output checksum mismatch: $output"
    return
  fi
  assert_safe_path "$(dirname "$output")"
  /bin/mkdir -p "$(dirname "$output")"
  local temporary
  temporary="$(/usr/bin/mktemp "$(dirname "$output")/.download.XXXXXX")"
  if ! /usr/bin/curl -fsSL \
      "https://android.googlesource.com/$project/+/$revision/$relative?format=TEXT" \
      | /usr/bin/base64 -D > "$temporary"; then
    /bin/rm -f "$temporary"
    fail "download failed: $project@$revision:$relative"
  fi
  [[ "$(sha256 "$temporary")" == "$expected" ]] || {
    local actual
    actual="$(sha256 "$temporary")"
    /bin/rm -f "$temporary"
    fail "checksum mismatch: $relative expected=$expected actual=$actual"
  }
  publish_new_file "$temporary" "$output"
}

materialize_local() {
  local input="$1" expected="$2" output="$3"
  [[ -f "$input" && ! -L "$input" ]] || fail "missing source file: $input"
  [[ "$(sha256 "$input")" == "$expected" ]] || fail "source checksum mismatch: $input"
  assert_regular_output "$output"
  if [[ -f "$output" ]]; then
    [[ "$(sha256 "$output")" == "$expected" ]] || \
      fail "existing resource checksum mismatch: $output"
    return
  fi
  assert_safe_path "$(dirname "$output")"
  /bin/mkdir -p "$(dirname "$output")"
  local temporary
  temporary="$(/usr/bin/mktemp "$(dirname "$output")/.copy.XXXXXX")"
  /bin/cp "$input" "$temporary"
  publish_new_file "$temporary" "$output"
}

base_root="$source_root/frameworks/base"
native_root="$source_root/frameworks/native"
bionic_root="$source_root/bionic"
system_core_root="$source_root/system/core"
assert_safe_path "$source_root"
assert_safe_path "$artifact_root"

materialize_remote "$FRAMEWORKS_BASE_PROJECT" "$FRAMEWORKS_BASE_REVISION" \
  "$KCM_BASE_JNI_PATH" "$KCM_BASE_JNI_SHA256" "$base_root/$KCM_BASE_JNI_PATH"
materialize_remote "$FRAMEWORKS_BASE_PROJECT" "$FRAMEWORKS_BASE_REVISION" \
  "$KCM_BASE_KEY_EVENT_CPP_PATH" "$KCM_BASE_KEY_EVENT_CPP_SHA256" "$base_root/$KCM_BASE_KEY_EVENT_CPP_PATH"
materialize_remote "$FRAMEWORKS_BASE_PROJECT" "$FRAMEWORKS_BASE_REVISION" \
  "$KCM_BASE_KEY_EVENT_H_PATH" "$KCM_BASE_KEY_EVENT_H_SHA256" "$base_root/$KCM_BASE_KEY_EVENT_H_PATH"
materialize_remote "$FRAMEWORKS_BASE_PROJECT" "$FRAMEWORKS_BASE_REVISION" \
  "$KCM_BASE_PARCEL_CPP_PATH" "$KCM_BASE_PARCEL_CPP_SHA256" "$base_root/$KCM_BASE_PARCEL_CPP_PATH"
materialize_remote "$FRAMEWORKS_BASE_PROJECT" "$FRAMEWORKS_BASE_REVISION" \
  "$KCM_BASE_PARCEL_H_PATH" "$KCM_BASE_PARCEL_H_SHA256" "$base_root/$KCM_BASE_PARCEL_H_PATH"
materialize_remote "$FRAMEWORKS_BASE_PROJECT" "$FRAMEWORKS_BASE_REVISION" \
  "$KCM_BASE_HELPERS_PATH" "$KCM_BASE_HELPERS_SHA256" "$base_root/$KCM_BASE_HELPERS_PATH"
materialize_remote "$FRAMEWORKS_BASE_PROJECT" "$FRAMEWORKS_BASE_REVISION" \
  "$KCM_BASE_JNI_WRAPPERS_PATH" "$KCM_BASE_JNI_WRAPPERS_SHA256" "$base_root/$KCM_BASE_JNI_WRAPPERS_PATH"
materialize_remote "$FRAMEWORKS_BASE_PROJECT" "$FRAMEWORKS_BASE_REVISION" \
  "$KCM_BASE_ANDROID_RUNTIME_H_PATH" "$KCM_BASE_ANDROID_RUNTIME_H_SHA256" "$base_root/$KCM_BASE_ANDROID_RUNTIME_H_PATH"
materialize_remote "$FRAMEWORKS_BASE_PROJECT" "$FRAMEWORKS_BASE_REVISION" \
  "$KCM_BASE_ANDROID_RUNTIME_LOG_H_PATH" "$KCM_BASE_ANDROID_RUNTIME_LOG_H_SHA256" "$base_root/$KCM_BASE_ANDROID_RUNTIME_LOG_H_PATH"
materialize_remote "$FRAMEWORKS_BASE_PROJECT" "$FRAMEWORKS_BASE_REVISION" \
  "$KCM_GENERIC_PATH" "$KCM_GENERIC_SHA256" "$base_root/$KCM_GENERIC_PATH"
materialize_remote "$FRAMEWORKS_BASE_PROJECT" "$FRAMEWORKS_BASE_REVISION" \
  "$KCM_VIRTUAL_PATH" "$KCM_VIRTUAL_SHA256" "$base_root/$KCM_VIRTUAL_PATH"

materialize_remote "$FRAMEWORKS_NATIVE_PROJECT" "$FRAMEWORKS_NATIVE_REVISION" \
  "$KCM_NATIVE_CPP_PATH" "$KCM_NATIVE_CPP_SHA256" "$native_root/$KCM_NATIVE_CPP_PATH"
materialize_remote "$FRAMEWORKS_NATIVE_PROJECT" "$FRAMEWORKS_NATIVE_REVISION" \
  "$KCM_NATIVE_LABELS_CPP_PATH" "$KCM_NATIVE_LABELS_CPP_SHA256" "$native_root/$KCM_NATIVE_LABELS_CPP_PATH"
materialize_remote "$FRAMEWORKS_NATIVE_PROJECT" "$FRAMEWORKS_NATIVE_REVISION" \
  "$KCM_NATIVE_KEYBOARD_CPP_PATH" "$KCM_NATIVE_KEYBOARD_CPP_SHA256" "$native_root/$KCM_NATIVE_KEYBOARD_CPP_PATH"
materialize_remote "$FRAMEWORKS_NATIVE_PROJECT" "$FRAMEWORKS_NATIVE_REVISION" \
  "$KCM_NATIVE_H_PATH" "$KCM_NATIVE_H_SHA256" "$native_root/$KCM_NATIVE_H_PATH"
materialize_remote "$FRAMEWORKS_NATIVE_PROJECT" "$FRAMEWORKS_NATIVE_REVISION" \
  "$KCM_NATIVE_LABELS_H_PATH" "$KCM_NATIVE_LABELS_H_SHA256" "$native_root/$KCM_NATIVE_LABELS_H_PATH"
materialize_remote "$FRAMEWORKS_NATIVE_PROJECT" "$FRAMEWORKS_NATIVE_REVISION" \
  "$KCM_NATIVE_KEYBOARD_H_PATH" "$KCM_NATIVE_KEYBOARD_H_SHA256" "$native_root/$KCM_NATIVE_KEYBOARD_H_PATH"
materialize_remote "$FRAMEWORKS_NATIVE_PROJECT" "$FRAMEWORKS_NATIVE_REVISION" \
  "$KCM_NATIVE_INPUT_CPP_PATH" "$KCM_NATIVE_INPUT_CPP_SHA256" "$native_root/$KCM_NATIVE_INPUT_CPP_PATH"
materialize_remote "$FRAMEWORKS_NATIVE_PROJECT" "$FRAMEWORKS_NATIVE_REVISION" \
  "$KCM_NATIVE_INPUT_H_PATH" "$KCM_NATIVE_INPUT_H_SHA256" "$native_root/$KCM_NATIVE_INPUT_H_PATH"
materialize_remote "$FRAMEWORKS_NATIVE_PROJECT" "$FRAMEWORKS_NATIVE_REVISION" \
  "$KCM_NATIVE_INPUT_DEVICE_H_PATH" "$KCM_NATIVE_INPUT_DEVICE_H_SHA256" "$native_root/$KCM_NATIVE_INPUT_DEVICE_H_PATH"
materialize_remote "$FRAMEWORKS_NATIVE_PROJECT" "$FRAMEWORKS_NATIVE_REVISION" \
  "$KCM_NATIVE_HMAC_KEY_MANAGER_H_PATH" "$KCM_NATIVE_HMAC_KEY_MANAGER_H_SHA256" "$native_root/$KCM_NATIVE_HMAC_KEY_MANAGER_H_PATH"
materialize_remote "$FRAMEWORKS_NATIVE_PROJECT" "$FRAMEWORKS_NATIVE_REVISION" \
  "$KCM_NATIVE_INPUT_DEVICE_CPP_PATH" "$KCM_NATIVE_INPUT_DEVICE_CPP_SHA256" "$native_root/$KCM_NATIVE_INPUT_DEVICE_CPP_PATH"
materialize_remote "$FRAMEWORKS_NATIVE_PROJECT" "$FRAMEWORKS_NATIVE_REVISION" \
  "$KCM_NATIVE_KEY_LAYOUT_MAP_CPP_PATH" "$KCM_NATIVE_KEY_LAYOUT_MAP_CPP_SHA256" "$native_root/$KCM_NATIVE_KEY_LAYOUT_MAP_CPP_PATH"
materialize_remote "$FRAMEWORKS_NATIVE_PROJECT" "$FRAMEWORKS_NATIVE_REVISION" \
  "$KCM_NATIVE_PROPERTY_MAP_CPP_PATH" "$KCM_NATIVE_PROPERTY_MAP_CPP_SHA256" "$native_root/$KCM_NATIVE_PROPERTY_MAP_CPP_PATH"
materialize_remote "$FRAMEWORKS_NATIVE_PROJECT" "$FRAMEWORKS_NATIVE_REVISION" \
  "$KCM_NATIVE_KEY_LAYOUT_MAP_H_PATH" "$KCM_NATIVE_KEY_LAYOUT_MAP_H_SHA256" "$native_root/$KCM_NATIVE_KEY_LAYOUT_MAP_H_PATH"
materialize_remote "$FRAMEWORKS_NATIVE_PROJECT" "$FRAMEWORKS_NATIVE_REVISION" \
  "$KCM_NATIVE_PROPERTY_MAP_H_PATH" "$KCM_NATIVE_PROPERTY_MAP_H_SHA256" "$native_root/$KCM_NATIVE_PROPERTY_MAP_H_PATH"
materialize_remote "$BIONIC_UAPI_PROJECT" "$BIONIC_UAPI_REVISION" \
  "$KCM_BIONIC_INPUT_H_PATH" "$KCM_BIONIC_INPUT_H_SHA256" "$bionic_root/$KCM_BIONIC_INPUT_H_PATH"
materialize_remote "$BIONIC_UAPI_PROJECT" "$BIONIC_UAPI_REVISION" \
  "$KCM_BIONIC_INPUT_EVENT_CODES_H_PATH" "$KCM_BIONIC_INPUT_EVENT_CODES_H_SHA256" "$bionic_root/$KCM_BIONIC_INPUT_EVENT_CODES_H_PATH"
materialize_remote "$SYSTEM_CORE_PROJECT" "$SYSTEM_CORE_REVISION" \
  "$KCM_SYSTEM_CORE_GENERATOR_PATH" "$KCM_SYSTEM_CORE_GENERATOR_SHA256" "$system_core_root/$KCM_SYSTEM_CORE_GENERATOR_PATH"
materialize_remote "$SYSTEM_CORE_PROJECT" "$SYSTEM_CORE_REVISION" \
  "$KCM_SYSTEM_CORE_TOOLBOX_BP_PATH" "$KCM_SYSTEM_CORE_TOOLBOX_BP_SHA256" "$system_core_root/$KCM_SYSTEM_CORE_TOOLBOX_BP_PATH"
materialize_remote "$FRAMEWORKS_NATIVE_PROJECT" "$FRAMEWORKS_NATIVE_REVISION" \
  "$KCM_NATIVE_POINTER_ICON_AIDL_PATH" "$KCM_NATIVE_POINTER_ICON_AIDL_SHA256" "$native_root/$KCM_NATIVE_POINTER_ICON_AIDL_PATH"
materialize_remote "$FRAMEWORKS_NATIVE_PROJECT" "$FRAMEWORKS_NATIVE_REVISION" \
  "$KCM_NATIVE_MOTION_FLAG_AIDL_PATH" "$KCM_NATIVE_MOTION_FLAG_AIDL_SHA256" "$native_root/$KCM_NATIVE_MOTION_FLAG_AIDL_PATH"
materialize_remote "$FRAMEWORKS_NATIVE_PROJECT" "$FRAMEWORKS_NATIVE_REVISION" \
  "$KCM_NATIVE_INPUT_CONSTANTS_AIDL_PATH" "$KCM_NATIVE_INPUT_CONSTANTS_AIDL_SHA256" "$native_root/$KCM_NATIVE_INPUT_CONSTANTS_AIDL_PATH"

for path in \
  "$KCM_BASE_JNI_PATH:$KCM_BASE_JNI_SHA256" \
  "$KCM_BASE_KEY_EVENT_CPP_PATH:$KCM_BASE_KEY_EVENT_CPP_SHA256" \
  "$KCM_BASE_KEY_EVENT_H_PATH:$KCM_BASE_KEY_EVENT_H_SHA256" \
  "$KCM_BASE_PARCEL_CPP_PATH:$KCM_BASE_PARCEL_CPP_SHA256" \
  "$KCM_BASE_PARCEL_H_PATH:$KCM_BASE_PARCEL_H_SHA256" \
  "$KCM_BASE_HELPERS_PATH:$KCM_BASE_HELPERS_SHA256" \
  "$KCM_BASE_JNI_WRAPPERS_PATH:$KCM_BASE_JNI_WRAPPERS_SHA256" \
  "$KCM_BASE_ANDROID_RUNTIME_H_PATH:$KCM_BASE_ANDROID_RUNTIME_H_SHA256" \
  "$KCM_BASE_ANDROID_RUNTIME_LOG_H_PATH:$KCM_BASE_ANDROID_RUNTIME_LOG_H_SHA256" \
  "$KCM_GENERIC_PATH:$KCM_GENERIC_SHA256" \
  "$KCM_VIRTUAL_PATH:$KCM_VIRTUAL_SHA256"; do
  relative="${path%%:*}" expected="${path##*:}"
  materialize_local "$base_root/$relative" "$expected" "$artifact_root/aosp/frameworks/base/$relative"
done
for path in \
  "$KCM_NATIVE_CPP_PATH:$KCM_NATIVE_CPP_SHA256" \
  "$KCM_NATIVE_LABELS_CPP_PATH:$KCM_NATIVE_LABELS_CPP_SHA256" \
  "$KCM_NATIVE_KEYBOARD_CPP_PATH:$KCM_NATIVE_KEYBOARD_CPP_SHA256" \
  "$KCM_NATIVE_H_PATH:$KCM_NATIVE_H_SHA256" \
  "$KCM_NATIVE_LABELS_H_PATH:$KCM_NATIVE_LABELS_H_SHA256" \
  "$KCM_NATIVE_KEYBOARD_H_PATH:$KCM_NATIVE_KEYBOARD_H_SHA256" \
  "$KCM_NATIVE_INPUT_CPP_PATH:$KCM_NATIVE_INPUT_CPP_SHA256" \
  "$KCM_NATIVE_INPUT_H_PATH:$KCM_NATIVE_INPUT_H_SHA256" \
  "$KCM_NATIVE_INPUT_DEVICE_H_PATH:$KCM_NATIVE_INPUT_DEVICE_H_SHA256" \
  "$KCM_NATIVE_HMAC_KEY_MANAGER_H_PATH:$KCM_NATIVE_HMAC_KEY_MANAGER_H_SHA256" \
  "$KCM_NATIVE_INPUT_DEVICE_CPP_PATH:$KCM_NATIVE_INPUT_DEVICE_CPP_SHA256" \
  "$KCM_NATIVE_KEY_LAYOUT_MAP_CPP_PATH:$KCM_NATIVE_KEY_LAYOUT_MAP_CPP_SHA256" \
  "$KCM_NATIVE_PROPERTY_MAP_CPP_PATH:$KCM_NATIVE_PROPERTY_MAP_CPP_SHA256" \
  "$KCM_NATIVE_KEY_LAYOUT_MAP_H_PATH:$KCM_NATIVE_KEY_LAYOUT_MAP_H_SHA256" \
  "$KCM_NATIVE_PROPERTY_MAP_H_PATH:$KCM_NATIVE_PROPERTY_MAP_H_SHA256" \
  "$KCM_NATIVE_POINTER_ICON_AIDL_PATH:$KCM_NATIVE_POINTER_ICON_AIDL_SHA256" \
  "$KCM_NATIVE_MOTION_FLAG_AIDL_PATH:$KCM_NATIVE_MOTION_FLAG_AIDL_SHA256" \
  "$KCM_NATIVE_INPUT_CONSTANTS_AIDL_PATH:$KCM_NATIVE_INPUT_CONSTANTS_AIDL_SHA256"; do
  relative="${path%%:*}" expected="${path##*:}"
  materialize_local "$native_root/$relative" "$expected" "$artifact_root/aosp/frameworks/native/$relative"
done
for path in \
  "$KCM_BIONIC_INPUT_H_PATH:$KCM_BIONIC_INPUT_H_SHA256" \
  "$KCM_BIONIC_INPUT_EVENT_CODES_H_PATH:$KCM_BIONIC_INPUT_EVENT_CODES_H_SHA256"; do
  relative="${path%%:*}" expected="${path##*:}"
  materialize_local "$bionic_root/$relative" "$expected" "$artifact_root/aosp/bionic/$relative"
done
for path in \
  "$KCM_SYSTEM_CORE_GENERATOR_PATH:$KCM_SYSTEM_CORE_GENERATOR_SHA256" \
  "$KCM_SYSTEM_CORE_TOOLBOX_BP_PATH:$KCM_SYSTEM_CORE_TOOLBOX_BP_SHA256"; do
  relative="${path%%:*}" expected="${path##*:}"
  materialize_local "$system_core_root/$relative" "$expected" "$artifact_root/aosp/system/core/$relative"
done

materialize_local "$base_root/$KCM_GENERIC_PATH" "$KCM_GENERIC_SHA256" \
  "$artifact_root/$KCM_RESOURCE_INSTALL_ROOT/$KCM_RESOURCE_GENERIC_NAME"
materialize_local "$base_root/$KCM_VIRTUAL_PATH" "$KCM_VIRTUAL_SHA256" \
  "$artifact_root/$KCM_RESOURCE_INSTALL_ROOT/$KCM_RESOURCE_VIRTUAL_NAME"

if /usr/bin/find "$source_root" "$artifact_root" -type l -print -quit | /usr/bin/grep -q .; then
  fail "unexpected symlink in materialized tree"
fi
if /usr/bin/find "$source_root" "$artifact_root" \( -name .git -o -name .gitmodules \) -print -quit | /usr/bin/grep -q .; then
  fail "unexpected Git metadata in materialized tree"
fi

/bin/mkdir -p "$source_root" "$artifact_root"
metadata_tmp="$(/usr/bin/mktemp "$artifact_root/.manifest.XXXXXX")"
{
  printf '%s\n' "base=$FRAMEWORKS_BASE_PROJECT@$FRAMEWORKS_BASE_REVISION"
  printf '%s\n' "native=$FRAMEWORKS_NATIVE_PROJECT@$FRAMEWORKS_NATIVE_REVISION"
  printf '%s\n' "resource_root=/$KCM_RESOURCE_INSTALL_ROOT"
  printf '%s\n' "Generic.kcm=$KCM_GENERIC_SHA256"
  printf '%s\n' "Virtual.kcm=$KCM_VIRTUAL_SHA256"
} > "$metadata_tmp"
publish_new_file "$metadata_tmp" "$artifact_root/manifest"
metadata_tmp="$(/usr/bin/mktemp "$source_root/.source-revision.XXXXXX")"
{
  printf '%s\n' "base=$FRAMEWORKS_BASE_REVISION"
  printf '%s\n' "native=$FRAMEWORKS_NATIVE_REVISION"
} > "$metadata_tmp"
publish_new_file "$metadata_tmp" "$source_root/.source-revision"

# Additive UAPI provenance must not overwrite an already published immutable
# frameworks/resources manifest. Keep this separately pinned owner explicit.
metadata_tmp="$(/usr/bin/mktemp "$artifact_root/.bionic-manifest.XXXXXX")"
printf '%s\n' "bionic=$BIONIC_UAPI_PROJECT@$BIONIC_UAPI_REVISION" > "$metadata_tmp"
publish_new_file "$metadata_tmp" "$artifact_root/bionic-manifest"
metadata_tmp="$(/usr/bin/mktemp "$artifact_root/.system-core-manifest.XXXXXX")"
{
  printf '%s\n' "system-core=$SYSTEM_CORE_PROJECT@$SYSTEM_CORE_REVISION"
  printf '%s\n' "generator=$KCM_SYSTEM_CORE_GENERATOR_PATH"
  printf '%s\n' "filegroup=$KCM_INPUT_LABELS_FILEGROUP"
  printf '%s\n' "inputs=$KCM_BIONIC_INPUT_H_PATH,$KCM_BIONIC_INPUT_EVENT_CODES_H_PATH"
} > "$metadata_tmp"
publish_new_file "$metadata_tmp" "$artifact_root/system-core-manifest"

while IFS= read -r -d '' file; do
  /bin/chmod 0644 "$file"
  /usr/bin/touch -t 200801010000 "$file"
done < <(/usr/bin/find "$source_root" "$artifact_root/aosp" -type f ! -name manifest ! -name .source-revision -print0)
/bin/chmod 0444 "$artifact_root/$KCM_RESOURCE_INSTALL_ROOT/$KCM_RESOURCE_GENERIC_NAME" \
  "$artifact_root/$KCM_RESOURCE_INSTALL_ROOT/$KCM_RESOURCE_VIRTUAL_NAME" "$artifact_root/manifest"
/bin/chmod 0444 "$artifact_root/bionic-manifest"
/usr/bin/touch -t 200801010000 "$artifact_root/bionic-manifest"
/bin/chmod 0444 "$artifact_root/system-core-manifest"
/usr/bin/touch -t 200801010000 "$artifact_root/system-core-manifest"
/bin/chmod 0644 "$source_root/.source-revision"
/usr/bin/touch -t 200801010000 "$source_root/.source-revision" "$artifact_root/manifest" \
  "$artifact_root/$KCM_RESOURCE_INSTALL_ROOT/$KCM_RESOURCE_GENERIC_NAME" \
  "$artifact_root/$KCM_RESOURCE_INSTALL_ROOT/$KCM_RESOURCE_VIRTUAL_NAME"

echo "key-character-map materialize: base=$FRAMEWORKS_BASE_REVISION native=$FRAMEWORKS_NATIVE_REVISION"
echo "key-character-map materialize: source=$source_root"
echo "key-character-map materialize: package=$artifact_root/$KCM_RESOURCE_INSTALL_ROOT"
