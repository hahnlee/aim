#!/bin/bash
set -euo pipefail
root="$(cd "$(dirname "$0")/../.." && pwd)"
source "$root/tools/lib/system-service-environment.sh"
export DARWIN_ART_APK_APP_NATIVE_PATH=/apk/unrelated.so
export DARWIN_ART_APK_DARWIN_DIRECTORY=/apk/darwin
export DARWIN_ART_APK_APP_PROVIDERS=unrelated.provider
export DARWIN_ART_APK_APP_SPLIT_SOURCE_DIRS=/apk/split.apk
export DARWIN_ART_APK_FUTURE_FIELD=must_not_leak
export DARWIN_ART_ACCEPTANCE_OUTPUT=/tmp/acceptance-output
export DARWIN_ART_DEBUG_INPUT_LATENCY=1
export DARWIN_ART_ANGLE_DIRECTORY=/runtime/angle
export ANDROID_ROOT=/system
darwin_art_system_service_environment \
  DARWIN_ART_APK_APP_PACKAGE=android \
  DARWIN_ART_APK_APP_SUPPORT_DEX=/runtime/support.dex \
  /bin/bash -eu -c '
    [[ -z ${DARWIN_ART_APK_APP_NATIVE_PATH+x} ]]
    [[ -z ${DARWIN_ART_APK_DARWIN_DIRECTORY+x} ]]
    [[ -z ${DARWIN_ART_APK_APP_PROVIDERS+x} ]]
    [[ -z ${DARWIN_ART_APK_APP_SPLIT_SOURCE_DIRS+x} ]]
    [[ -z ${DARWIN_ART_APK_FUTURE_FIELD+x} ]]
    [[ -z ${DARWIN_ART_ACCEPTANCE_OUTPUT+x} ]]
    [[ $DARWIN_ART_DEBUG_INPUT_LATENCY == 1 ]]
    [[ $DARWIN_ART_APK_APP_PACKAGE == android ]]
    [[ $DARWIN_ART_APK_APP_SUPPORT_DEX == /runtime/support.dex ]]
    [[ $DARWIN_ART_ANGLE_DIRECTORY == /runtime/angle ]]
    [[ $ANDROID_ROOT == /system ]]
  '
[[ $DARWIN_ART_APK_APP_NATIVE_PATH == /apk/unrelated.so ]]
[[ $DARWIN_ART_APK_APP_PROVIDERS == unrelated.provider ]]
[[ $DARWIN_ART_ACCEPTANCE_OUTPUT == /tmp/acceptance-output ]]
baseline_environment="$(darwin_art_system_service_environment /usr/bin/env | LC_ALL=C sort)"
for artifact_destination in '' /tmp/different-artifact-destination; do
  export DARWIN_ART_ACCEPTANCE_OUTPUT="$artifact_destination"
  child_environment="$(darwin_art_system_service_environment /usr/bin/env | LC_ALL=C sort)"
  [[ "$child_environment" == "$baseline_environment" ]]
  [[ "$DARWIN_ART_ACCEPTANCE_OUTPUT" == "$artifact_destination" ]]
  darwin_art_system_service_environment /bin/bash -eu -c \
    '[[ -z ${DARWIN_ART_ACCEPTANCE_OUTPUT+x} ]]'
done
echo 'system service environment: APK isolation, explicit system inputs, parent preservation PASS'
