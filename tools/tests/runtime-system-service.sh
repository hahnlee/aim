#!/bin/bash
# Real shell provisioning/argument test with an inert host fixture. This does
# not claim native startup, daemon readiness or APK interaction acceptance.
set -euo pipefail
if [[ "${1:-}" == --start-system-service ]]; then
  [[ $# == 12 && $2 == '/bundle/system image.tar' && $3 == /store ]]
  [[ $5 == --window-seconds && $6 == 0 && $7 == /bundle/runtime.dylib ]]
  [[ $8 == /boot/oj && $9 == /boot/libart && ${10} == /boot/framework ]]
  [[ ${11} == /boot/a:/boot/b && ${12} == '/data/support dex' ]]
  [[ $AIM_SYSTEM_SERVER_MODE == 1 && $AIM_APK_APP_PACKAGE == android ]]
  [[ $AIM_RUNTIME_TARGET_SDK_VERSION == 36 && $AIM_RUNTIME_JAVA_DEBUGGABLE == 0 ]]
  [[ $AIM_APK_APP_SUPPORT_DEX == '/data/support dex' ]]
  [[ $AIM_APK_APP_RESOURCE_APK == "$AIM_TEST_IMAGE/system/framework/framework-res.apk" ]]
  [[ $AIM_FRAMEWORK_RES_APK == "$AIM_TEST_IMAGE/system/framework/framework-res.apk" ]]
  [[ -z ${AIM_APK_APP_ACTIVITY+x} && -z ${AIM_APK_APP_DESCRIPTOR+x} ]]
  [[ $AIM_ANDROID_FILESYSTEM_ROOT == "$AIM_TEST_IMAGE" ]]
  [[ $AIM_ANDROID_SYSTEM_NATIVE_DIR == "$AIM_TEST_IMAGE/system/lib64" ]]
  # SYSTEMSERVERCLASSPATH from the image's derive_classpath environment file.
  [[ $AIM_RUNTIME_HOST_FILES == "/boot/a:/boot/b:/data/support dex:$AIM_TEST_IMAGE/system/framework/services.jar:$AIM_TEST_IMAGE/apex/com.android.art/javalib/service-art.jar" ]]
  # PackageManagerService owns /data/app in the system server.
  [[ $AIM_ANDROID_PACKAGE_ROOT_WRITABLE == 1 ]]
  [[ -z ${AIM_APK_APP_NATIVE_PATH+x} && -z ${AIM_APP_COMMAND_LINE+x} ]]
  [[ $AIM_APK_APP_PROVIDERS == none && $AIM_APK_APP_RECEIVERS == none ]]
  [[ -d $AIM_ANDROID_PRIVATE_DATA_ROOT/system ]]
  [[ -d $AIM_ANDROID_PRIVATE_DATA_ROOT/user/0/android ]]
  # Fixture failure must propagate; no socket probing can turn it into success.
  [[ ${AIM_TEST_HOST_STATUS:-0} == 0 ]] || exit "$AIM_TEST_HOST_STATUS"
  printf 'pid=123\nbinder=/tmp/ready.system.sock\ncompositor=/tmp/ready.sf.sock\n'
  exit 0
fi

root="$(cd "$(dirname "$0")/../.." && pwd)"
source "$root/tools/lib/system-private-data.sh"
source "$root/tools/lib/system-service-environment.sh"
source "$root/tools/lib/runtime-system-service.sh"
source "$root/tools/lib/runtime-system-image.sh"
aim_apply_runtime_endpoints $'pid=123\nbinder=/tmp/instance.system.sock\ncompositor=/tmp/instance.sf.sock'
[[ "$AIM_SYSTEM_SERVER_SOCKET" == /tmp/instance.system.sock ]]
[[ "$AIM_SURFACEFLINGER_SOCKET" == /tmp/instance.sf.sock ]]
for bad_reply in \
  $'pid=1\nbinder=/tmp/new\ncompositor=relative' \
  $'pid=1\npid=2\nbinder=/tmp/new\ncompositor=/tmp/new2' \
  $'pid=\npid=1\nbinder=/tmp/new\ncompositor=/tmp/new2' \
  $'pid=1\nbinder=/tmp/new' \
  $'pid=1\nbinder=/tmp/new\ncompositor=/tmp/new2\nextra=value'; do
  if aim_apply_runtime_endpoints "$bad_reply"; then
    echo 'invalid endpoint response accepted' >&2; exit 1
  fi
  [[ "$AIM_SYSTEM_SERVER_SOCKET" == /tmp/instance.system.sock ]]
  [[ "$AIM_SURFACEFLINGER_SOCKET" == /tmp/instance.sf.sock ]]
done
fixture="$(mktemp -d /tmp/darwin-system-service-test.XXXXXX)"
trap 'chmod -R u+w "$fixture"; rm -rf -- "$fixture"' EXIT
cp "$root/tools/tests/runtime-system-service.sh" "$fixture/host"
export AIM_TEST_IMAGE="$fixture/image"
mkdir -p "$AIM_TEST_IMAGE/system/etc"
printf '%s\n' \
  'export SYSTEMSERVERCLASSPATH /system/framework/services.jar:/apex/com.android.art/javalib/service-art.jar' \
  >"$AIM_TEST_IMAGE/system/etc/classpath"
chmod 0700 "$fixture/host"
mkdir -p "$fixture/bundle/android" "$fixture/bundle/_build/android-system-image"
touch "$fixture/bundle/android/system-root.tar" "$fixture/bundle/_build/android-system-image/system-root.tar"
[[ $(aim_runtime_system_archive "$fixture/bundle" packaged) == "$fixture/bundle/android/system-root.tar" ]]
[[ $(aim_runtime_system_archive "$fixture/bundle" development) == "$fixture/bundle/_build/android-system-image/system-root.tar" ]]
# A failed selector must return before invoking even an always-successful host.
if aim_prepare_runtime_system_image "$fixture/bundle" /usr/bin/true invalid /store 2>/dev/null; then
  echo 'invalid image mode reached the host' >&2; exit 1
fi
export AIM_BOOT_CLASSPATH=/boot/a:/boot/b
export AIM_APK_APP_NATIVE_PATH=/app/native.so
export AIM_APP_COMMAND_LINE=app-only-command
export AIM_APK_APP_ACTIVITY=dev.aim.probe.ProbeActivity
export AIM_APK_APP_DESCRIPTOR='Ldev/aim/probe/ProbeActivity;'
export AIM_FRAMEWORK_RES_APK=/inherited/framework-res.apk
export AIM_RUNTIME_TARGET_SDK_VERSION=29
export AIM_RUNTIME_JAVA_DEBUGGABLE=1
invoke() {
  aim_start_runtime_system_service "$fixture/host" "$fixture/mnt" "$AIM_TEST_IMAGE" \
    '/bundle/system image.tar' /store "$AIM_TEST_IMAGE/system/framework/framework-res.apk" \
    '/data/support dex' /bundle/runtime.dylib /boot/oj /boot/libart /boot/framework /boot/a:/boot/b
}
reply="$(invoke)"
aim_apply_runtime_endpoints "$reply"
[[ "$AIM_SYSTEM_SERVER_SOCKET" == /tmp/ready.system.sock ]]
[[ "$AIM_SURFACEFLINGER_SOCKET" == /tmp/ready.sf.sock ]]
# Reprovisioning preserves existing Android-owned state, including its inode.
state="$fixture/mnt/data/apps/android.system/private-data/system/preserved"
touch "$state"
inode="$(stat -f '%i' "$state")"
invoke
[[ $(stat -f '%i' "$state") == "$inode" ]]
export AIM_TEST_HOST_STATUS=73
status=0
invoke || status=$?
[[ $status == 73 ]]
[[ $AIM_APK_APP_NATIVE_PATH == /app/native.so ]]
[[ $AIM_APP_COMMAND_LINE == app-only-command ]]
[[ $AIM_RUNTIME_TARGET_SDK_VERSION == 29 ]]
# Provisioning failure must not be masked when callers use a conditional.
chmod u+w "$fixture/mnt/data/apps/android.system/private-data"
mv "$fixture/mnt/data/apps/android.system/private-data/system" "$fixture/preserved-system"
touch "$fixture/mnt/data/apps/android.system/private-data/system"
export AIM_TEST_HOST_STATUS=0
if invoke 2>/dev/null; then echo 'invalid storage reached the host' >&2; exit 1; fi
echo 'system runtime launch: explicit command/environment, state preservation and failure propagation PASS'
