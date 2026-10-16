#!/bin/bash
# Derives the app host image (docs/app-host.md) from the derived image:
# an APFS clone of it in which zygote starts without system_server, the
# app host agent is installed, and ActivityManager and PackageManager are
# native. The derived image itself is left unchanged.
#
# Usage: tools/app-host-image.sh [DERIVED_ROOT [OUT]]
#   DERIVED_ROOT  default: target/aim/derived/root
#   OUT           default: target/aim/app-host/root (replaced)
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
derived="${1:-$root/target/aim/derived/root}"
out="${2:-$root/target/aim/app-host/root}"
agent="$root/target/aim/daemons/bin/aim-app-host-agent"
[[ -d "$derived" && -x "$agent" ]] || {
  echo "app-host-image needs the derived image and daemon/aim-app-host-agent (cargo aim build)" >&2; exit 2;
}
rm -rf "$out"
mkdir -p "$(dirname "$out")"
cp -c -R "$derived" "$out"
zygote="$out/system/etc/init/hw/init.zygote64.rc"
grep -q -- ' --start-system-server' "$zygote" || { echo "$zygote: no --start-system-server" >&2; exit 1; }
sed -i '' 's/ --start-system-server//' "$zygote"
cp "$agent" "$out/system_ext/bin/aim-app-host-agent"
cp "$root/image/app-host/aim-app-host-agent.rc" "$out/system_ext/etc/init/aim-app-host-agent.rc"
printf 'activity         com.android.server.am.ActivityManagerService$Lifecycle\n' >> "$out/system/etc/aim/native-services"
echo "$out"
