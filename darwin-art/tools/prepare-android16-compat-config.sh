#!/bin/bash
# Materialize original Android compatibility XMLs from the locked image.
# This creates a build artifact only; it does not activate compat policy.
set -euo pipefail
export LC_ALL=C TZ=UTC
root="$(cd "$(dirname "$0")/.." && pwd)"
[[ $# == 2 && "$1" = /* && "$2" = /* ]] || {
  echo "usage: $0 ABSOLUTE_SYSTEM_IMAGE NEW_ABSOLUTE_OUTPUT_DIRECTORY" >&2
  exit 64
}
image="$1"
out="$2"
[[ -f "$image" && ! -L "$image" ]] || { echo "expected regular system image" >&2; exit 64; }
[[ ! -e "$out" && ! -L "$out" && "$out" != / ]] || {
  echo "destination exists or is invalid: $out" >&2
  exit 64
}
source "$root/upstream/android16-linker-config.lock"
source "$root/tools/lib/system-compat-artifact.sh"
actual_image="$(shasum -a 256 "$image")"
[[ "${actual_image%% *}" == "$SYSTEM_IMAGE_PS16K_SHA256" ]] || {
  echo "system compatibility source image hash mismatch" >&2
  exit 65
}
manifest="$root/upstream/android16-system-compat-files.lock"
reader="$root/target/release/super-i18n-apex-extract"
publisher="$root/tools/lib/publish-new-directory.py"
[[ -f "$manifest" && ! -L "$manifest" && -x "$reader" && ! -L "$reader" &&
   -f "$publisher" && ! -L "$publisher" ]] || {
  echo "missing release extractor or compatibility manifest" >&2
  exit 69
}
parent="${out%/*}"
[[ -d "$parent" && ! -L "$parent" ]] || {
  echo "output parent must be an existing directory" >&2
  exit 64
}
stage="$(mktemp -d "$parent/.android16-compat-config.XXXXXX")"
system_expected=""
system_actual=""
ext_expected=""
ext_actual=""
system_sorted=""
ext_sorted=""
cleanup() {
  if [[ -d "$stage" ]]; then
    chmod -R u+w "$stage" 2>/dev/null || true
    rm -rf -- "$stage"
  fi
  rm -f -- "$system_expected" "$system_actual" "$ext_expected" \
    "$system_sorted" "$ext_sorted"
}
trap cleanup EXIT
system_expected="$(mktemp "${TMPDIR:-/tmp}/compat-system-expected.XXXXXX")"
system_actual="$(mktemp "${TMPDIR:-/tmp}/compat-system-actual.XXXXXX")"
ext_expected="$(mktemp "${TMPDIR:-/tmp}/compat-ext-expected.XXXXXX")"
ext_actual="$(mktemp "${TMPDIR:-/tmp}/compat-ext-actual.XXXXXX")"
system_sorted="$(mktemp "${TMPDIR:-/tmp}/compat-system-sorted.XXXXXX")"
ext_sorted="$(mktemp "${TMPDIR:-/tmp}/compat-ext-sorted.XXXXXX")"
while read -r expected partition reader_path archive_path extra; do
  [[ -z "${expected:-}" || "$expected" == \#* ]] && continue
  [[ -z "${extra:-}" && "$expected" =~ ^[0-9a-f]{64}$ ]] || {
    echo "malformed compatibility manifest" >&2
    exit 64
  }
  case "$partition" in
    system)
      [[ "$reader_path" == /system/etc/compatconfig/*.xml &&
         "$archive_path" == /system/etc/compatconfig/*.xml ]] || exit 64
      printf '%s\n' "${reader_path##*/}" >> "$system_expected"
      ;;
    system_ext)
      [[ "$reader_path" == /etc/compatconfig/*.xml &&
         "$archive_path" == /system/system_ext/etc/compatconfig/*.xml ]] || exit 64
      printf '%s\n' "${reader_path##*/}" >> "$ext_expected"
      ;;
    *) exit 64 ;;
  esac
done < "$manifest"
sort -u "$system_expected" -o "$system_expected"
sort -u "$ext_expected" -o "$ext_expected"
"$reader" "$image" - --path /system/etc/compatconfig > "$system_actual"
"$reader" "$image" - --partition system_ext --path /etc/compatconfig > "$ext_actual"
sed '/^\.$/d; /^\.\.$/d' "$system_actual" | sort -u > "$system_sorted"
sed '/^\.$/d; /^\.\.$/d' "$ext_actual" | sort -u > "$ext_sorted"
cmp "$system_expected" "$system_sorted"
cmp "$ext_expected" "$ext_sorted"
while read -r expected partition reader_path archive_path extra; do
  [[ -z "${expected:-}" || "$expected" == \#* ]] && continue
  destination="$stage$archive_path"
  mkdir -p "${destination%/*}"
  if [[ "$partition" == system_ext ]]; then
    "$reader" "$image" "$destination" --partition system_ext --path "$reader_path"
  else
    "$reader" "$image" "$destination" --path "$reader_path"
  fi
  [[ "$(shasum -a 256 "$destination")" == "$expected  $destination" ]] || {
    echo "compatibility XML hash mismatch: $archive_path" >&2
    exit 65
  }
done < "$manifest"
find "$stage" -exec touch -t 200801010000 {} +
find "$stage" -type f -exec chmod 0444 {} +
find "$stage" -type d -exec chmod 0555 {} +
darwin_art_verify_system_compat_config_inventory "$stage" "$manifest"
python3 "$publisher" "$stage" "$out"
stage=""
trap - EXIT
rm -f -- "$system_expected" "$system_actual" "$ext_expected" "$ext_actual" \
  "$system_sorted" "$ext_sorted"
echo "Original Android compatibility config verified: $out (not activated)"
