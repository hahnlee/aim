#!/bin/bash
# Actual original-image reader regression; not framework or APK acceptance.
set -euo pipefail
root="$(cd "$(dirname "$0")/../.." && pwd)"
[[ $# == 1 ]] || { echo "usage: $0 PINNED_SYSTEM_IMAGE" >&2; exit 64; }
image="$1"
reader="$root/target/release/super-i18n-apex-extract"
source "$root/upstream/android16-linker-config.lock"
actual="$(shasum -a 256 "$image")"
[[ "${actual%% *}" == "$SYSTEM_IMAGE_PS16K_SHA256" ]] || exit 65
stage="$(mktemp -d /tmp/darwin-art-compat-partitions.XXXXXX)"
trap 'rm -rf -- "$stage"' EXIT
"$reader" "$image" - --path /system/etc/compatconfig > "$stage/default"
"$reader" "$image" - --partition system --path /system/etc/compatconfig > "$stage/explicit"
cmp "$stage/default" "$stage/explicit"
[[ "$(sed '/^\.$/d; /^\.\.$/d' "$stage/default" | wc -l | tr -d ' ')" == 10 ]]
"$reader" "$image" - --partition system_ext --path /etc/compatconfig > "$stage/extension"
[[ "$(sed '/^\.$/d; /^\.\.$/d' "$stage/extension")" == settings-platform-compat-config.xml ]]
"$reader" "$image" "$stage/framework.xml" --path /system/etc/compatconfig/framework-platform-compat-config.xml
actual="$(shasum -a 256 "$stage/framework.xml")"
[[ "${actual%% *}" == 93c493c341478e6903b541dedc7b43650d320728607e4a8671ed17ff799a3eef ]]
"$reader" "$image" "$stage/settings.xml" --partition system_ext --path /etc/compatconfig/settings-platform-compat-config.xml
actual="$(shasum -a 256 "$stage/settings.xml")"
[[ "${actual%% *}" == e695ef17003b332aae1095de2aaf9d18ebbb6b7d432023ab95b70574bb4de214 ]]
reject() {
  if "$reader" "$image" "$stage/rejected" "$@"; then
    echo 'invalid partition selector accepted' >&2; exit 1
  fi
  [[ ! -e "$stage/rejected" && ! -L "$stage/rejected" ]]
}
reject --partition product --path /etc/compatconfig
reject --partition system_ext
reject --partition
reject --partition system_ext --path /etc/../etc/compatconfig
# A directory reached through the system partition's real symlink is not an
# absent catalog and must not silently become an empty successful listing.
if "$reader" "$image" - --path /system/system_ext/etc/compatconfig; then exit 1; fi
echo 'system compat partitions: original bytes, default compatibility, strict selectors PASS'
