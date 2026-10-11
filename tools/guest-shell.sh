#!/bin/bash
# Runs the image's shell in a running guest as adbd's shell would run it
# (docs/boot-status.md, "Debugging"): with init's global environment
# (<data>.run/environ: PATH, BOOTCLASSPATH, ANDROID_*, ...), the boot's
# binder and its pid namespace, as root. `aimctl shell` does the same for
# an aimctl guest.
#
# Usage: tools/guest-shell.sh [--linux-run PATH] [--image ROOT] [--cts-owner PATH] DATA [COMMAND]
#   DATA     the boot's data directory (guest-init's --data)
#   COMMAND  run by `sh -c`; default: an interactive shell
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
linux_run=
image=
cts_owner=
while [[ "${1:-}" == --* ]]; do
  [[ $# -ge 2 && -n "$2" ]] || { echo "$1 requires a value" >&2; exit 2; }
  case "$1" in
    --repository-root) root="$2" ;;
    --linux-run) linux_run="$2" ;;
    --image) image="$2" ;;
    --cts-owner) cts_owner="$2" ;;
    *) echo "unknown guest-shell option: $1" >&2; exit 2 ;;
  esac
  shift 2
done
linux_run="${linux_run:-$root/target/release/linux-run}"
image="${image:-$root/target/aim/derived/root}"
cts_owner="${cts_owner:-$root/target/release/aim-cts-owner}"
[[ -x "$linux_run" && -x "$cts_owner" && -d "$image" ]] || {
  echo 'guest-shell requires executable runtime/identity owner and selected image' >&2; exit 2;
}
[[ $# -ge 1 ]] || { sed -n '8,10p' "$0" >&2; exit 2; }
data="$1"
shift
runtime="$data.run"
# The DATA controller may also hold data.lock. Select the registered init,
# authenticated by its exact kernel birth and actual init identity record.
[[ -f "$runtime/path-map" ]] || { echo "$data: no runtime path map" >&2; exit 1; }
guest_init="$("$cts_owner" --init-pid "$runtime/identity/by-pid")"
[[ "$guest_init" =~ ^[1-9][0-9]*$ ]] || { echo "$data: invalid authenticated init PID" >&2; exit 1; }
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
exec env -i ${env[@]+"${env[@]}"} "$linux_run" ${inherit[@]+"${inherit[@]}"} --root "$image" \
  --mount-namespace-from-init --path-map "$runtime/path-map" --binder "dev.aim.guest-init.$guest_init.binder" \
  --by-pid "$runtime/identity/by-pid" /system/bin/sh "$@"
