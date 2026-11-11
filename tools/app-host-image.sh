#!/bin/bash
# Derives the app host image (docs/app-host.md) from the derived image:
# an APFS clone of it in which zygote starts without system_server, the
# app host agent is installed, and ActivityManager and PackageManager are
# native. The derived image itself is left unchanged.
#
# Usage: tools/app-host-image.sh [DERIVED_ROOT [OUT]]
#   DERIVED_ROOT  default: target/aim/derived/root
#   OUT           default: target/aim/app-host/root (replaced)
# The image's translation cache (docs/storage.md) goes beside OUT, as the
# derived image's is beside its root: the derived image's entries, kept
# across runs (an entry is named by its content and never changes), with
# an index for OUT's files and the agent translated. The package parser's
# cache beside OUT (package-cache) is kept too: its entries are keyed by
# their APKs' contents and times, which the clone keeps, and the app host
# removes those of another image or parser.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
derived="${1:-$root/target/aim/derived/root}"
out="${2:-$root/target/aim/app-host/root}"
agent="$root/target/aim/daemons/bin/aim-app-host-agent"
translate="$root/target/release/linux-translate"
translated="$(dirname "$derived")/translated"
[[ -d "$derived" && -d "$translated" && -x "$agent" && -x "$translate" ]] || {
  echo "app-host-image needs the derived image with its translation cache, daemon/aim-app-host-agent and host/linux-translate (cargo aim build)" >&2; exit 2;
}
rm -rf "$out"
mkdir -p "$(dirname "$out")"
cp -c -p -R "$derived" "$out"
# Entries are read-only: removing one makes it writable first. A copy is
# staged under a dot name, as linux-translate stages its own (.tmp-*), so
# an interrupted run leaves no partial entry, only a stage removed here.
cache="$(dirname "$out")/translated"
mkdir -p "$cache"
for entry in "$cache"/.copy-* "$cache"/.tmp-* "$cache"/*; do
  name="${entry##*/}"
  [[ -e "$entry" && "$name" != paths && ( "$name" == .copy-* || "$name" == .tmp-* || ! -e "$translated/$name" ) ]] || continue
  chmod -R u+w "$entry"
  rm -rf "$entry"
done
for entry in "$translated"/*; do
  name="${entry##*/}"
  [[ "$name" != paths && ! -e "$cache/$name" ]] || continue
  cp -c -R "$entry" "$cache/.copy-$name"
  mv "$cache/.copy-$name" "$cache/$name"
done
zygote="$out/system/etc/init/hw/init.zygote64.rc"
grep -q -- ' --start-system-server' "$zygote" || { echo "$zygote: no --start-system-server" >&2; exit 1; }
sed -i '' 's/ --start-system-server//' "$zygote"
cp "$agent" "$out/system_ext/bin/aim-app-host-agent"
cp "$root/image/app-host/aim-app-host-agent.rc" "$out/system_ext/etc/init/aim-app-host-agent.rc"
cp "$root/image/app-host/aim-system" "$out/system_ext/bin/aim-system"
chmod 755 "$out/system_ext/bin/aim-system"
cp "$root/image/app-host/aim-system.rc" "$out/system_ext/etc/init/aim-system.rc"
cp "$root/target/aim/device-services/aim-nearby.jar" "$out/system/framework/aim-nearby.jar"
# The services init starts for it (`aimctl` and `guest-init --only`).
cp "$root/image/app-host/services" "$out/system/etc/aim/app-host-services"
printf 'activity         com.android.server.am.ActivityManagerService$Lifecycle\n' >> "$out/system/etc/aim/native-services"
# The native package boot the app host runs (#2022).
printf 'package          com.android.server.pm.PackageManagerService\n' >> "$out/system/etc/aim/native-services"
"$translate" --image "$(dirname "$out")" > /dev/null
echo "$out"
