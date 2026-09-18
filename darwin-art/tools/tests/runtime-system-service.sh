#!/bin/bash
# Real shell provisioning/argument test with an inert host fixture. This does
# not claim native startup, daemon readiness or APK interaction acceptance.
set -euo pipefail
if [[ "${1:-}" == --start-system-service ]]; then
  [[ $# == 12 && $2 == '/bundle/system image.tar' && $3 == /store ]]
  [[ $5 == --window-seconds && $6 == 0 && $7 == /bundle/runtime.dylib ]]
  [[ $8 == /boot/oj && $9 == /boot/libart && ${10} == /boot/framework ]]
  [[ ${11} == /boot/a:/boot/b && ${12} == '/data/support dex' ]]
  [[ $DARWIN_ART_SYSTEM_SERVER_MODE == 1 && $DARWIN_ART_APK_APP_PACKAGE == android ]]
  [[ $DARWIN_ART_RUNTIME_TARGET_SDK_VERSION == 36 && $DARWIN_ART_RUNTIME_JAVA_DEBUGGABLE == 0 ]]
  [[ $DARWIN_ART_APK_APP_SUPPORT_DEX == '/data/support dex' ]]
  [[ $DARWIN_ART_APK_APP_RESOURCE_APK == /image/system/framework/framework-res.apk ]]
  [[ $DARWIN_ART_FRAMEWORK_RES_APK == /image/system/framework/framework-res.apk ]]
  [[ -z ${DARWIN_ART_APK_APP_ACTIVITY+x} && -z ${DARWIN_ART_APK_APP_DESCRIPTOR+x} ]]
  [[ $DARWIN_ART_ANDROID_FILESYSTEM_ROOT == /image ]]
  [[ $DARWIN_ART_ANDROID_SYSTEM_NATIVE_DIR == /image/system/lib64 ]]
  [[ $DARWIN_ART_RUNTIME_HOST_FILES == '/boot/a:/boot/b:/data/support dex:/image/system/framework/services.jar' ]]
  [[ -z ${DARWIN_ART_APK_APP_NATIVE_PATH+x} && -z ${DARWIN_ART_APP_COMMAND_LINE+x} ]]
  [[ $DARWIN_ART_APK_APP_PROVIDERS == none && $DARWIN_ART_APK_APP_RECEIVERS == none ]]
  [[ -d $DARWIN_ART_ANDROID_PRIVATE_DATA_ROOT/system ]]
  [[ -d $DARWIN_ART_ANDROID_PRIVATE_DATA_ROOT/user/0/android ]]
  # Fixture failure must propagate; no socket probing can turn it into success.
  [[ ${DARWIN_ART_TEST_HOST_STATUS:-0} == 0 ]] || exit "$DARWIN_ART_TEST_HOST_STATUS"
  printf 'pid=123\nbinder=/tmp/ready.system.sock\ncompositor=/tmp/ready.sf.sock\n'
  exit 0
fi

root="$(cd "$(dirname "$0")/../.." && pwd)"
source "$root/tools/lib/system-private-data.sh"
source "$root/tools/lib/system-service-environment.sh"
source "$root/tools/lib/runtime-system-service.sh"
source "$root/tools/lib/runtime-system-image.sh"
darwin_art_apply_runtime_endpoints $'pid=123\nbinder=/tmp/instance.system.sock\ncompositor=/tmp/instance.sf.sock'
[[ "$DARWIN_ART_SYSTEM_SERVER_SOCKET" == /tmp/instance.system.sock ]]
[[ "$DARWIN_ART_SURFACEFLINGER_SOCKET" == /tmp/instance.sf.sock ]]
for bad_reply in \
  $'pid=1\nbinder=/tmp/new\ncompositor=relative' \
  $'pid=1\npid=2\nbinder=/tmp/new\ncompositor=/tmp/new2' \
  $'pid=\npid=1\nbinder=/tmp/new\ncompositor=/tmp/new2' \
  $'pid=1\nbinder=/tmp/new' \
  $'pid=1\nbinder=/tmp/new\ncompositor=/tmp/new2\nextra=value'; do
  if darwin_art_apply_runtime_endpoints "$bad_reply"; then
    echo 'invalid endpoint response accepted' >&2; exit 1
  fi
  [[ "$DARWIN_ART_SYSTEM_SERVER_SOCKET" == /tmp/instance.system.sock ]]
  [[ "$DARWIN_ART_SURFACEFLINGER_SOCKET" == /tmp/instance.sf.sock ]]
done
fixture="$(mktemp -d /tmp/darwin-system-service-test.XXXXXX)"
trap 'chmod -R u+w "$fixture"; rm -rf -- "$fixture"' EXIT
cp "$root/tools/tests/runtime-system-service.sh" "$fixture/host"
chmod 0700 "$fixture/host"
mkdir -p "$fixture/bundle/android" "$fixture/bundle/_build/android-system-image"
touch "$fixture/bundle/android/system-root.tar" "$fixture/bundle/_build/android-system-image/system-root.tar"
[[ $(darwin_art_runtime_system_archive "$fixture/bundle" packaged) == "$fixture/bundle/android/system-root.tar" ]]
[[ $(darwin_art_runtime_system_archive "$fixture/bundle" development) == "$fixture/bundle/_build/android-system-image/system-root.tar" ]]
# A failed selector must return before invoking even an always-successful host.
if darwin_art_prepare_runtime_system_image "$fixture/bundle" /usr/bin/true invalid /store 2>/dev/null; then
  echo 'invalid image mode reached the host' >&2; exit 1
fi
export DARWIN_ART_BOOT_CLASSPATH=/boot/a:/boot/b
export DARWIN_ART_APK_APP_NATIVE_PATH=/app/native.so
export DARWIN_ART_APP_COMMAND_LINE=app-only-command
export DARWIN_ART_APK_APP_ACTIVITY=dev.darwinart.probe.ProbeActivity
export DARWIN_ART_APK_APP_DESCRIPTOR='Ldev/darwinart/probe/ProbeActivity;'
export DARWIN_ART_FRAMEWORK_RES_APK=/inherited/framework-res.apk
export DARWIN_ART_RUNTIME_TARGET_SDK_VERSION=29
export DARWIN_ART_RUNTIME_JAVA_DEBUGGABLE=1
invoke() {
  darwin_art_start_runtime_system_service "$fixture/host" "$fixture/mnt" /image \
    '/bundle/system image.tar' /store /image/system/framework/framework-res.apk \
    '/data/support dex' /bundle/runtime.dylib /boot/oj /boot/libart /boot/framework /boot/a:/boot/b
}
reply="$(invoke)"
darwin_art_apply_runtime_endpoints "$reply"
[[ "$DARWIN_ART_SYSTEM_SERVER_SOCKET" == /tmp/ready.system.sock ]]
[[ "$DARWIN_ART_SURFACEFLINGER_SOCKET" == /tmp/ready.sf.sock ]]
# Reprovisioning preserves existing Android-owned state, including its inode.
state="$fixture/mnt/data/apps/android.system/private-data/system/preserved"
touch "$state"
inode="$(stat -f '%i' "$state")"
invoke
[[ $(stat -f '%i' "$state") == "$inode" ]]
export DARWIN_ART_TEST_HOST_STATUS=73
status=0
invoke || status=$?
[[ $status == 73 ]]
[[ $DARWIN_ART_APK_APP_NATIVE_PATH == /app/native.so ]]
[[ $DARWIN_ART_APP_COMMAND_LINE == app-only-command ]]
[[ $DARWIN_ART_RUNTIME_TARGET_SDK_VERSION == 29 ]]
# Provisioning failure must not be masked when callers use a conditional.
chmod u+w "$fixture/mnt/data/apps/android.system/private-data"
mv "$fixture/mnt/data/apps/android.system/private-data/system" "$fixture/preserved-system"
touch "$fixture/mnt/data/apps/android.system/private-data/system"
export DARWIN_ART_TEST_HOST_STATUS=0
if invoke 2>/dev/null; then echo 'invalid storage reached the host' >&2; exit 1; fi
echo 'system runtime launch: explicit command/environment, state preservation and failure propagation PASS'
