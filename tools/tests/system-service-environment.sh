#!/bin/bash
set -euo pipefail
root="$(cd "$(dirname "$0")/../.." && pwd)"
source "$root/tools/lib/system-service-environment.sh"
export AIM_APK_APP_NATIVE_PATH=/apk/unrelated.so
export AIM_APK_DARWIN_DIRECTORY=/apk/darwin
export AIM_APK_APP_PROVIDERS=unrelated.provider
export AIM_APK_APP_SPLIT_SOURCE_DIRS=/apk/split.apk
export AIM_APK_FUTURE_FIELD=must_not_leak
export AIM_ACCEPTANCE_OUTPUT=/tmp/acceptance-output
export AIM_DEBUG_INPUT_LATENCY=1
export AIM_ANGLE_DIRECTORY=/runtime/angle
export ANDROID_ROOT=/system
aim_system_service_environment \
  AIM_APK_APP_PACKAGE=android \
  AIM_APK_APP_SUPPORT_DEX=/runtime/support.dex \
  /bin/bash -eu -c '
    [[ -z ${AIM_APK_APP_NATIVE_PATH+x} ]]
    [[ -z ${AIM_APK_DARWIN_DIRECTORY+x} ]]
    [[ -z ${AIM_APK_APP_PROVIDERS+x} ]]
    [[ -z ${AIM_APK_APP_SPLIT_SOURCE_DIRS+x} ]]
    [[ -z ${AIM_APK_FUTURE_FIELD+x} ]]
    [[ -z ${AIM_ACCEPTANCE_OUTPUT+x} ]]
    [[ $AIM_DEBUG_INPUT_LATENCY == 1 ]]
    [[ $AIM_APK_APP_PACKAGE == android ]]
    [[ $AIM_APK_APP_SUPPORT_DEX == /runtime/support.dex ]]
    [[ $AIM_ANGLE_DIRECTORY == /runtime/angle ]]
    [[ $ANDROID_ROOT == /system ]]
  '
[[ $AIM_APK_APP_NATIVE_PATH == /apk/unrelated.so ]]
[[ $AIM_APK_APP_PROVIDERS == unrelated.provider ]]
[[ $AIM_ACCEPTANCE_OUTPUT == /tmp/acceptance-output ]]
baseline_environment="$(aim_system_service_environment /usr/bin/env | LC_ALL=C sort)"
for artifact_destination in '' /tmp/different-artifact-destination; do
  export AIM_ACCEPTANCE_OUTPUT="$artifact_destination"
  child_environment="$(aim_system_service_environment /usr/bin/env | LC_ALL=C sort)"
  [[ "$child_environment" == "$baseline_environment" ]]
  [[ "$AIM_ACCEPTANCE_OUTPUT" == "$artifact_destination" ]]
  aim_system_service_environment /bin/bash -eu -c \
    '[[ -z ${AIM_ACCEPTANCE_OUTPUT+x} ]]'
done
echo 'system service environment: APK isolation, explicit system inputs, parent preservation PASS'
