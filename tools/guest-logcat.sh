#!/bin/bash
# Reads the logs of a running guest-init boot with the image's own logcat
# (docs/boot-status.md, "Debugging"): the original logd holds every
# service's log, and logcat reaches it through the boot's path map.
#
# Usage: tools/guest-logcat.sh [--linux-run PATH] RUNTIME_DIR [logcat args]
#   RUNTIME_DIR  the boot's runtime directory (guest-init's --runtime,
#                by default <data>/run)
#   logcat args  default: -d -b all -v threadtime
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
linux_run="$root/target/release/linux-run"
if [[ "${1:-}" == "--linux-run" ]]; then
  linux_run="$2"
  shift 2
fi
[[ $# -ge 1 ]] || { sed -n '6,9p' "$0" >&2; exit 2; }
runtime="$1"
shift
[[ -f "$runtime/path-map" ]] || { echo "$runtime/path-map: not a guest-init runtime" >&2; exit 1; }
[[ $# -gt 0 ]] || set -- -d -b all -v threadtime
exec "$linux_run" --path-map "$runtime/path-map" /system/bin/logcat "$@"
