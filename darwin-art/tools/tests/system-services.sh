#!/bin/bash
# Real original-artifact producer test, not an APK/runtime acceptance test.
set -euo pipefail
root="$(cd "$(dirname "$0")/../.." && pwd)"
[[ $# == 1 ]] || { echo "usage: $0 ABSOLUTE_PINNED_SYSTEM_IMAGE" >&2; exit 64; }
stage="$(mktemp -d /tmp/darwin-art-system-services-test.XXXXXX)"
trap 'rm -rf -- "$stage"' EXIT
producer="$root/tools/prepare-android16-system-services.sh"
output="$stage/valid/services.jar"
bash "$producer" "$1" "$output"
source "$root/upstream/android16-system-services.lock"
actual="$(shasum -a 256 "$output")"
[[ "${actual%% *}" == "$SYSTEM_SERVICES_SHA256" ]]
[[ "$(stat -f %Lp "$output")" == 444 ]]
expect_status() {
  local wanted="$1" actual
  shift
  if "$@"; then actual=0; else actual=$?; fi
  [[ "$actual" == "$wanted" ]] || {
    echo "expected status $wanted, got $actual" >&2; exit 1;
  }
}
expect_status 64 bash "$producer" "$1" "$output"
expect_status 65 bash "$producer" "$root/upstream/android16-system-services.lock" "$stage/rejected/services.jar"
[[ ! -e "$stage/rejected" ]]
mkdir "$stage/link"
ln -s "$output" "$stage/link/services.jar"
expect_status 64 bash "$producer" "$1" "$stage/link/services.jar"
actual="$(shasum -a 256 "$output")"
[[ "${actual%% *}" == "$SYSTEM_SERVICES_SHA256" ]]
[[ -z "$(find "$stage" -type d -name '.system-services.*' -print -quit)" ]]
echo 'system services: original bytes, read-only publication, mismatch/overwrite/symlink rejection PASS'
