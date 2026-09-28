#!/bin/bash
# Materialized build snapshot for original linkerconfig, not a running profile.
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
[[ $# -ge 1 && $# -le 2 && "$1" = /* ]] || { echo "usage: $0 NEW_ABSOLUTE_BUILD_DIRECTORY [--strict-audit]" >&2; exit 2; }
strict_audit=0
if [[ $# == 2 ]]; then
  [[ "$2" == --strict-audit ]] || { echo "unknown option: $2" >&2; exit 2; }
  strict_audit=1
fi
out="$1"
[[ ! -e "$out" && ! -L "$out" ]] || { echo "destination exists: $out" >&2; exit 1; }
inputs="$root/_build/android16-apex-payloads"
cmp "$inputs/metadata/payloads.lock" "$root/upstream/android16-apex-payloads.lock"
[[ "$(cat "$inputs/metadata/complete")" == 'offline-payloads-v1; not activated' ]]
[[ -x "$root/_build/linkerconfig-apex/apex-inventory" ]]
[[ -x "$root/_build/linkerconfig-host/linkerconfig" ]]
bash "$root/tools/extract-android16-linker-config.sh"
mkdir -m 700 "$out"
mkdir -p "$out/system/apex" "$out/system/etc" "$out/linkerconfig"
bash "$root/tools/prepare-android16-system-native-files.sh" "$out/native-inputs"
mv "$out/native-inputs/system/lib64" "$out/system/lib64"
rmdir "$out/native-inputs/system" "$out/native-inputs"
# APFS copy-on-write retains real directories for AOSP GetActivePackages and
# avoids copying 80MiB of data. Preserve Android symlinks without following them.
cp -cR "$inputs/payloads" "$out/apex"
placements=()
while IFS=$'\t' read -r module expected archive; do
  if [[ "$archive" != /* ]]; then
    archive="$inputs/$archive"
  fi
  locked="$(awk -v module="$module" '$1 == module {print $2}' "$root/upstream/android16-apex-payloads.lock")"
  [[ -n "$locked" && "$locked" == "$expected" ]]
  [[ "$(shasum -a 256 "$archive" | awk '{print $1}')" == "$expected" ]]
  name="${archive##*/}"
  ln "$archive" "$out/system/apex/$name"
  # Prove this snapshot's manifest matches its locked source archive; never
  # publish placement inferred solely from a directory name.
  unzip -p "$archive" apex_manifest.pb | cmp - "$out/apex/$module/apex_manifest.pb"
  placements+=("$module=/system/apex/$name")
done < "$inputs/metadata/sources.tsv"
cp "$root/_build/android16-linker-config/system/etc/"* "$out/system/etc/"
"$root/_build/linkerconfig-apex/apex-inventory" "$out" "${placements[@]}" \
  > "$out/apex/apex-info-list.xml"
# AOSP init invokes --target without --strict (pinned init/builtins.cpp).
# Strict is a separate diagnostic, not a replacement for boot-time semantics.
generator_args=(--root "$out" --target "$out/linkerconfig")
if (( strict_audit )); then generator_args+=(--strict); fi
"$root/_build/linkerconfig-host/linkerconfig" "${generator_args[@]}"
echo "Original linkerconfig generated for build snapshot: $out (runtime not connected)"
