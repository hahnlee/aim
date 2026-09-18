#!/bin/bash
# Host process provisioning only. Android service policy remains in framework
# owners. The metadata below is existing bootstrap migration debt, not a new
# substitute for AOSP SystemServer. Source with system-private-data.sh and
# system-service-environment.sh.
darwin_art_start_runtime_system_service() (
  set -euo pipefail
  local host="$1" profile_mount="$2" system_server_root="$3" archive="$4"
  local store="$5" framework_res="$6" support_dex="$7"
  shift 7
  [[ "$#" == 5 ]] || { echo "invalid system runtime command" >&2; return 64; }
  local system_server_private="$profile_mount/data/apps/android.system/private-data"
  local system_server_command=("$host" --window-seconds 0 "$@" "$support_dex")
  prepare_system_private_data "$system_server_private" || return
  darwin_art_system_service_environment \
  DARWIN_ART_SYSTEM_SERVER_MODE=1 \
  DARWIN_ART_RUNTIME_TARGET_SDK_VERSION=36 \
  DARWIN_ART_RUNTIME_JAVA_DEBUGGABLE=0 \
  DARWIN_ART_DAEMONIZED_LOG="${profile_mount%/mnt}/darwin-artd.log" \
  DARWIN_ART_ANDROID_SHARED_STORAGE_ROOT="$profile_mount/storage" \
  DARWIN_ART_FRAMEWORK_RES_APK="$framework_res" \
  DARWIN_ART_APK_APP_PACKAGE=android \
  DARWIN_ART_APK_APP_APPLICATION=android.app.Application \
  DARWIN_ART_APK_APP_LAUNCH_COMPONENT=none \
  DARWIN_ART_APK_APP_SCREEN_ORIENTATION=-1 \
  DARWIN_ART_APK_APP_ACTIVITIES=none \
  DARWIN_ART_APK_APP_ACTIVITY_ALIASES=none \
  DARWIN_ART_APK_APP_SERVICES=none \
  DARWIN_ART_APK_APP_RECEIVERS=none \
  DARWIN_ART_APK_APP_PROVIDERS=none \
  DARWIN_ART_APK_APP_SERVICE_METADATA=none \
  DARWIN_ART_APK_APP_METADATA=none \
  DARWIN_ART_APK_APP_VERSION_CODE=1 \
  DARWIN_ART_APK_APP_VERSION_NAME=1 \
  DARWIN_ART_APK_APP_THEME=0 \
  DARWIN_ART_APK_APP_TARGET_SDK=36 \
  DARWIN_ART_APK_APP_LABEL=Android \
  DARWIN_ART_APK_APP_LABEL_RES=0 \
  DARWIN_ART_APK_APP_ICON_RES=0 \
  DARWIN_ART_APK_APP_RESOURCE_APK="$framework_res" \
  DARWIN_ART_APK_APP_SUPPORT_DEX="$support_dex" \
  DARWIN_ART_ANDROID_PRIVATE_DATA_ROOT="$system_server_private" \
  DARWIN_ART_APK_APP_DATA_DIR="${system_server_private%/private-data}" \
  DARWIN_ART_APK_APP_DATA_GUEST_DIR=/data/user/0/android \
  DARWIN_ART_APK_APP_EXTERNAL_DIR=/storage/emulated/0 \
  DARWIN_ART_ANDROID_FILESYSTEM_ROOT="$system_server_root" \
  DARWIN_ART_ANDROID_SYSTEM_ROOT="$system_server_root/system" \
  DARWIN_ART_ANDROID_SYSTEM_NATIVE_DIR="$system_server_root/system/lib64" \
  DARWIN_ART_RUNTIME_HOST_FILES="$DARWIN_ART_BOOT_CLASSPATH:$support_dex:$system_server_root/system/framework/services.jar" \
  DARWIN_ART_DEBUG_SURFACECONTROL_CAPTURE_PATH="${DARWIN_ART_DEBUG_SURFACECONTROL_CAPTURE_PATH:-}" \
  DARWIN_ART_DEBUG_SURFACECONTROL_CAPTURE_PIXELS="${DARWIN_ART_DEBUG_SURFACECONTROL_CAPTURE_PIXELS:-}" \
  DARWIN_ART_DEBUG_SURFACECONTROL_PIXELS="${DARWIN_ART_DEBUG_SURFACECONTROL_PIXELS:-}" \
  DARWIN_ART_DEBUG_SURFACE_TRANSACTIONS="${DARWIN_ART_DEBUG_SURFACE_TRANSACTIONS:-}" \
  DARWIN_ART_DEBUG_GRAPHICS_DSO="${DARWIN_ART_DEBUG_GRAPHICS_DSO:-}" \
  DARWIN_ART_VM_FAILURE_TRACE="${DARWIN_ART_VM_FAILURE_TRACE:-}" \
  "$host" --start-system-service "$archive" "$store" "${system_server_command[@]}"
)

# Consume only the typed CLI reply, never evaluate it as shell code. Publish
# addresses atomically after validating the complete response; no PID probing
# or socket-existence test can substitute for daemon readiness.
darwin_art_apply_runtime_endpoints() {
  local line pid="" binder="" compositor=""
  while IFS= read -r line; do
    case "$line" in
      pid=*) [[ -z "$pid" && -n "${line#pid=}" ]] || return 65; pid="${line#pid=}" ;;
      binder=*) [[ -z "$binder" && -n "${line#binder=}" ]] || return 65; binder="${line#binder=}" ;;
      compositor=*) [[ -z "$compositor" && -n "${line#compositor=}" ]] || return 65; compositor="${line#compositor=}" ;;
      *) return 65 ;;
    esac
  done <<< "$1"
  [[ "$pid" =~ ^[1-9][0-9]*$ && "$binder" == /* && "$compositor" == /* ]] || return 65
  export DARWIN_ART_SYSTEM_SERVER_SOCKET="$binder"
  export DARWIN_ART_SURFACEFLINGER_SOCKET="$compositor"
}
