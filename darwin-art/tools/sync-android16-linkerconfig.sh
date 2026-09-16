#!/bin/bash
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
source "$root/upstream/android16-linkerconfig-source.lock"
destination="$root/_aosp/android16-linkerconfig"
if [[ ! -e "$destination" ]]; then
  git init -q "$destination"
  git -C "$destination" remote add origin https://android.googlesource.com/platform/system/linkerconfig
fi
[[ -d "$destination/.git" ]] || { echo 'Refusing non-git source destination' >&2; exit 1; }
if ! git -C "$destination" rev-parse --verify HEAD >/dev/null 2>&1; then
  [[ -z "$(git -C "$destination" status --porcelain)" ]] || {
    echo 'Refusing populated uninitialized source tree' >&2; exit 1;
  }
  git -C "$destination" fetch --depth=1 origin "$LINKERCONFIG_REVISION"
  git -C "$destination" -c advice.detachedHead=false checkout --detach "$LINKERCONFIG_REVISION"
fi
[[ "$(git -C "$destination" rev-parse HEAD)" == "$LINKERCONFIG_REVISION" ]] || {
  echo 'Source revision mismatch; existing checkout preserved' >&2; exit 1;
}
[[ -z "$(git -C "$destination" status --porcelain)" ]] || {
  echo 'Source modifications present; refusing to claim pristine AOSP input' >&2; exit 1;
}
echo "AOSP linkerconfig source verified: $LINKERCONFIG_REVISION ($destination)"
