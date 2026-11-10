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
# derived image's is beside its root: a clone of it, indexed for OUT's
# files and with the agent translated.
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
rm -rf "$out" "$(dirname "$out")/translated"
mkdir -p "$(dirname "$out")"
cp -c -R "$derived" "$out"
cp -c -R "$translated" "$(dirname "$out")/translated"
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
