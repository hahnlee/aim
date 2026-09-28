#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
# shellcheck source=../sources.lock
source "$root/sources.lock"

archive="$root/_downloads/MoltenVK-macos-$MOLTENVK_VERSION.tar"
output="$root/_build/moltenvk"
dylib="$output/libMoltenVK.dylib"
license="$output/LICENSE"
url="https://github.com/KhronosGroup/MoltenVK/releases/download/v$MOLTENVK_VERSION/MoltenVK-macos.tar"

sha256() {
  shasum -a 256 "$1" | cut -d' ' -f1
}

verify_file() {
  local file="$1"
  local expected_size="$2"
  local expected_sha="$3"
  [[ -f "$file" && "$(stat -f %z "$file")" == "$expected_size" &&
      "$(sha256 "$file")" == "$expected_sha" ]]
}

if verify_file "$dylib" "$MOLTENVK_MACOS_DYLIB_SIZE" \
    "$MOLTENVK_MACOS_DYLIB_SHA256" &&
   [[ -f "$license" && "$(sha256 "$license")" == "$MOLTENVK_LICENSE_SHA256" ]]; then
  echo "$dylib"
  exit 0
fi

mkdir -p "$root/_downloads" "$root/_build" "$output"
if ! verify_file "$archive" "$MOLTENVK_MACOS_ARCHIVE_SIZE" \
    "$MOLTENVK_MACOS_ARCHIVE_SHA256"; then
  archive_download="$(mktemp "$root/_downloads/MoltenVK-macos.download.XXXXXX")"
  trap 'rm -f "$archive_download"' EXIT
  curl -fL --retry 3 "$url" -o "$archive_download"
  verify_file "$archive_download" "$MOLTENVK_MACOS_ARCHIVE_SIZE" \
    "$MOLTENVK_MACOS_ARCHIVE_SHA256" || {
    echo "MoltenVK archive failed its pinned size/checksum gate" >&2
    exit 65
  }
  mv "$archive_download" "$archive"
  trap - EXIT
fi

stage="$(mktemp -d "$root/_build/moltenvk.materialize.XXXXXX")"
trap 'rm -rf "$stage"' EXIT
tar -xf "$archive" -C "$stage" \
  MoltenVK/MoltenVK/dynamic/dylib/macOS/libMoltenVK.dylib \
  MoltenVK/LICENSE

source_dylib="$stage/MoltenVK/MoltenVK/dynamic/dylib/macOS/libMoltenVK.dylib"
source_license="$stage/MoltenVK/LICENSE"
verify_file "$source_dylib" "$MOLTENVK_MACOS_DYLIB_SIZE" \
  "$MOLTENVK_MACOS_DYLIB_SHA256" || {
  echo "MoltenVK dylib failed its pinned size/checksum gate" >&2
  exit 65
}
[[ "$(sha256 "$source_license")" == "$MOLTENVK_LICENSE_SHA256" ]] || {
  echo "MoltenVK license failed its pinned checksum gate" >&2
  exit 65
}

cp "$source_dylib" "$dylib"
cp "$source_license" "$license"
chmod 0444 "$dylib" "$license"
echo "$dylib"
