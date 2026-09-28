#!/bin/bash
set -euo pipefail
export LC_ALL=C

script_dir="$(cd "$(dirname "$0")" && pwd)"
project_root="$(cd "$script_dir/.." && pwd)"
lock_file="$project_root/upstream/freebsd-linuxulator.lock"
reference_root="$project_root/upstream/freebsd-linuxulator"
source_root="$reference_root/source"
file_manifest="$reference_root/source-files.tsv"
generated_root="$reference_root/manifests"
update_manifests=0

if [[ "${1:-}" == "--update-manifests" ]]; then
  update_manifests=1
elif [[ $# -ne 0 ]]; then
  echo "usage: $0 [--update-manifests]" >&2
  exit 64
fi

# shellcheck disable=SC1090
source "$lock_file"

sha256() { shasum -a 256 "$1" | awk '{print $1}'; }
fail() { echo "freebsd-linuxulator: $*" >&2; exit 3; }

seen=0
seen_bytes=0
while IFS=$'\t' read -r expected_sha expected_bytes relative; do
  [[ "$expected_sha" == "sha256" ]] && continue
  [[ -n "$expected_sha" && -n "$expected_bytes" && -n "$relative" ]] ||
    fail "malformed source manifest row"
  case "$relative" in
    /*|../*|*/../*|*/..) fail "unsafe source path: $relative" ;;
  esac
  destination="$source_root/$relative"
  if [[ ! -f "$destination" ]]; then
    mkdir -p "$(dirname "$destination")"
    staged="$(mktemp "${TMPDIR:-/tmp}/freebsd-linuxulator-download.XXXXXX")"
    curl -fsSL "$FREEBSD_RAW_BASE/$FREEBSD_REVISION/$relative" -o "$staged" ||
      fail "download failed: $relative"
    [[ "$(sha256 "$staged")" == "$expected_sha" ]] ||
      fail "download checksum mismatch: $relative"
    [[ "$(wc -c < "$staged" | tr -d ' ')" == "$expected_bytes" ]] ||
      fail "download size mismatch: $relative"
    mv "$staged" "$destination"
  fi
  [[ "$(sha256 "$destination")" == "$expected_sha" ]] ||
    fail "checksum mismatch: $relative"
  [[ "$(wc -c < "$destination" | tr -d ' ')" == "$expected_bytes" ]] ||
    fail "size mismatch: $relative"
  seen=$((seen + 1))
  seen_bytes=$((seen_bytes + expected_bytes))
done < "$file_manifest"

[[ "$seen" == "$SOURCE_FILE_COUNT" ]] || fail "source file count lock mismatch"
[[ "$seen_bytes" == "$SOURCE_BYTE_COUNT" ]] || fail "source byte count lock mismatch"
actual_count="$(find "$source_root" -type f | wc -l | tr -d ' ')"
[[ "$actual_count" == "$SOURCE_FILE_COUNT" ]] ||
  fail "unexpected file in source slice (expected $SOURCE_FILE_COUNT, got $actual_count)"

stage="$(mktemp -d "${TMPDIR:-/tmp}/freebsd-linuxulator-gate.XXXXXX")"
trap 'rm -rf "$stage"' EXIT
python3 "$script_dir/generate-freebsd-linuxulator-manifests.py" \
  --source-root "$source_root" --output-dir "$stage"

if [[ "$update_manifests" == 1 ]]; then
  mkdir -p "$generated_root"
  for generated in "$stage"/*.tsv; do
    install -m 0644 "$generated" "$generated_root/$(basename "$generated")"
  done
else
  [[ -d "$generated_root" ]] || fail "checked manifests missing; run with --update-manifests"
  for generated in "$stage"/*.tsv; do
    checked="$generated_root/$(basename "$generated")"
    [[ -f "$checked" ]] || fail "checked manifest missing: $(basename "$generated")"
    diff -u "$checked" "$generated" || fail "generated manifest drift"
  done
fi

python3 "$project_root/probes/freebsd-linuxulator-bionic-diff.py" \
  --linuxulator "$generated_root/constants.tsv" \
  --bionic "$project_root/upstream/android16-os-constants-values.tsv" \
  --expected "$project_root/probes/freebsd-linuxulator-bionic-expected-differences.tsv" \
  --expected-comparisons 200

echo "freebsd-linuxulator: PASS revision=$FREEBSD_REVISION files=$seen bytes=$seen_bytes"
