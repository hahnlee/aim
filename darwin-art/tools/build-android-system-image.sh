#!/bin/bash
# Produce the immutable development shared-system-image archive.
#
# This is deliberately a packaging edge, not an AOSP/native build edge: the
# native baseline, font image, framework resource and original service code are verified by
# their owning stages. The package helper owns the bounded archive inventory.
set -euo pipefail
export LC_ALL=C
umask 022

script_dir="$(cd "$(dirname "$0")" && pwd)"
project_root="$(cd "$script_dir/.." && pwd)"

# shellcheck disable=SC1091
source "$project_root/tools/lib/package-system-root.sh"
source "$project_root/tools/lib/system-services-artifact.sh"

if [[ "$#" -ne 0 && "$#" -ne 4 && "$#" -ne 5 ]]; then
  echo "usage: $0 [NATIVE_BASELINE ARCHIVE_DESTINATION FONTS_ROOT FRAMEWORK_RES_APK [SERVICES_JAR]]" >&2
  exit 64
fi

if [[ $# == 0 ]]; then
  native_baseline="$(bash "$project_root/tools/prepare-runtime-system-root.sh")"
  services_jar="$(darwin_art_prepare_system_services_artifact)"
else
  native_baseline="$1"
  services_jar="${5:-$project_root/_build/android16-system-services/services.jar}"
fi
archive_destination="${2:-$project_root/_build/android-system-image/system-root.tar}"
fonts_root="${3:-$project_root/_build/android16-system-fonts}"
framework_res_apk="${4:-$project_root/_prebuilt/android-16/resources/framework-res.apk}"

[[ "$archive_destination" = /* && ! -L "$archive_destination" ]] || {
  echo "archive destination must be an absolute, non-symlink path: $archive_destination" >&2
  exit 64
}
[[ ! -d "$archive_destination" ]] || {
  echo "archive destination is a directory: $archive_destination" >&2
  exit 64
}

archive_parent="$(dirname "$archive_destination")"
mkdir -p "$archive_parent"

# Build in a private sibling directory. package-system-root.sh requires a
# fresh destination and leaves all artifact inputs untouched; publishing the
# completed file with rename gives readers either the old archive or the new
# archive, never a partially written one. The destination is the one stable
# product owned by this stage, so an existing regular archive may be replaced.
stage_dir="$(mktemp -d "$archive_parent/.system-root-stage.XXXXXX")"
staged_archive="$stage_dir/system-root.tar"
cleanup() {
  rm -rf -- "$stage_dir"
}
trap cleanup EXIT

darwin_art_package_system_root \
  "$native_baseline" "$staged_archive" "$fonts_root" "$framework_res_apk" "$services_jar"

[[ -f "$staged_archive" && ! -L "$staged_archive" ]] || {
  echo "system image producer did not create a regular archive" >&2
  exit 70
}
if [[ -e "$archive_destination" || -L "$archive_destination" ]]; then
  [[ -f "$archive_destination" && ! -L "$archive_destination" ]] || {
    echo "refusing to replace non-regular archive destination: $archive_destination" >&2
    exit 65
  }
  # Avoid needless publication when the producer's bytes are unchanged. This
  # keeps the stable inode usable by readers and makes warm graph invocations
  # true no-ops at the archive boundary.
  if cmp -s "$staged_archive" "$archive_destination"; then
    echo "android system image: archive unchanged=$archive_destination"
    exit 0
  fi
fi
mv -f "$staged_archive" "$archive_destination"
[[ -f "$archive_destination" && ! -L "$archive_destination" ]] || {
  echo "system image archive publication failed: $archive_destination" >&2
  exit 70
}

echo "android system image: archive=$archive_destination"
