#!/bin/bash
# Runs the image's shell in a running guest as adbd's shell would run it
# (docs/boot-status.md, "Debugging"): with init's global environment
# (<data>.run/environ: PATH, BOOTCLASSPATH, ANDROID_*, ...), the boot's
# binder and its pid namespace, as root. `aimctl shell` does the same for
# an aimctl guest.
#
# Usage: tools/guest-shell.sh [--linux-run PATH] DATA [COMMAND]
#   DATA     the boot's data directory (guest-init's --data)
#   COMMAND  run by `sh -c`; default: an interactive shell
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
linux_run="$root/target/release/linux-run"
if [[ "${1:-}" == "--linux-run" ]]; then
  linux_run="$2"
  shift 2
fi
[[ $# -ge 1 ]] || { sed -n '8,10p' "$0" >&2; exit 2; }
data="$1"
shift
runtime="$data.run"
# guest-init holds the data directory's lock while it runs.
guest_init="$(lsof -t -- "$data.lock" 2>/dev/null | head -1)"
[[ -n "$guest_init" && -f "$runtime/path-map" ]] || { echo "$data: no running guest" >&2; exit 1; }
# Before init's first `export`, linux-run's default environment.
env=()
inherit=()
if [[ -f "$runtime/environ" ]]; then
  while IFS= read -r line; do env+=("$line"); done < "$runtime/environ"
  # An interactive shell gets the terminal's type, as adbd's does.
  [[ $# -eq 0 && -n "${TERM:-}" ]] && env+=("TERM=$TERM")
  inherit=(--inherit-env)
fi
[[ $# -gt 0 ]] && set -- -c "$*"
exec env -i ${env[@]+"${env[@]}"} "$linux_run" ${inherit[@]+"${inherit[@]}"} --root "$root/target/aim/derived/root" \
  --path-map "$runtime/path-map" --binder "dev.aim.guest-init.$guest_init.binder" \
  --by-pid "$runtime/identity/by-pid" /system/bin/sh "$@"
