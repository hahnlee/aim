#!/bin/bash
# Compare real Bionic consumer imports against a concrete native provider.
# Definition coverage is necessary, not proof of ABI or lifecycle correctness.
set -euo pipefail
[[ $# == 2 ]] || { echo "usage: $0 GUEST_ROOT PROVIDER_ARCHIVE_OR_IMAGE" >&2; exit 2; }
guest_root="$1"
provider="$2"
[[ -d "$guest_root" && -f "$provider" ]]
readelf="${ANDROID_SDK_ROOT:-$HOME/Library/Android/sdk}/ndk/28.2.13676358/toolchains/llvm/prebuilt/darwin-x86_64/bin/llvm-readelf"
[[ -x "$readelf" ]]
stage="$(mktemp -d /tmp/linker-startup-abi.XXXXXX)"
trap 'rm -f "$stage/imports" "$stage/definitions"; rmdir "$stage"' EXIT
for name in libc.so libdl.so libdl_android.so; do
  "$readelf" --dyn-syms "$guest_root/apex/com.android.runtime/lib64/bionic/$name" |
    awk -v consumer="$name" '$7 == "UND" && $8 ~ /^__loader_/ { print consumer, $5, $8 }'
done > "$stage/imports"
[[ -s "$stage/imports" ]]
# Strip the one Mach-O ABI underscore, not the Android symbol's underscores.
xcrun nm -gU "$provider" | awk 'NF >= 3 && $NF ~ /^___loader_/ {name=$NF; sub(/^_/, "", name); print name}' \
  > "$stage/definitions"
missing=0
while read -r consumer binding symbol; do
  if grep -Fxq "$symbol" "$stage/definitions"; then
    echo "DEFINED $consumer $binding $symbol"
  else
    echo "MISSING $consumer $binding $symbol"
    missing=$((missing + 1))
  fi
done < "$stage/imports"
if (( missing )); then
  echo "$missing loader imports lack definitions; startup ABI is incomplete" >&2
  exit 1
fi
echo 'Definition coverage only: validate ABI, state ownership and calls before publication'
