#!/bin/bash
# Runs host-side CTS modules against a running guest with the pinned
# release's own harness (docs/cts.md): cts-tradefed on the repository's JDK
# and aapt2 and the Mac's adb, connected to the guest's adbd on the Mac's
# loopback (`--androidboot aim.adb.port=PORT`).
#
# Usage: tools/cts-tradefed.sh [--linux-run PATH] DATA PORT [cts-tradefed run option]...
#   DATA  the boot's data directory (guest-init's --data)
#   PORT  its adbd's port; this run's adb server listens on PORT + 1
# e.g. tools/cts-tradefed.sh target/aim/boot/data 5611 -m CtsPackageSettingHostTestCases
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
shell_args=()
if [[ "${1:-}" == "--linux-run" ]]; then
  [[ $# -ge 2 && -x "$2" ]] || { echo '--linux-run requires an executable path' >&2; exit 2; }
  shell_args=(--linux-run "$2")
  shift 2
fi
[[ $# -ge 2 ]] || { sed -n '7,10p' "$0" >&2; exit 2; }
data="$1"
port="$2"
shift 2
harness="$root/_build/cts-tradefed/android-cts"
[[ -f "$harness/tools/cts-tradefed" ]] || { echo "no harness: run tools/cts-module.py --tradefed" >&2; exit 1; }
command -v adb > /dev/null || { echo "adb (Android SDK Platform-Tools) must be on PATH" >&2; exit 1; }
build_tools=("$root"/_build/java/build-tools-*/android-*)
for jdk in \
  "/Applications/Android Studio.app/Contents/jbr/Contents/Home/bin" \
  /opt/homebrew/opt/openjdk@21/bin; do
  [[ -x "$jdk/java" ]] || continue
  [[ "$("$jdk/java" -version 2>&1 | head -1)" == *'21.'* ]] && break
done
[[ -x "$jdk/java" && "$("$jdk/java" -version 2>&1 | head -1)" == *'21.'* ]] || {
  echo "CTS host tests require JDK 21 (Android Studio JBR or Homebrew openjdk@21)" >&2
  exit 1
}
serial="127.0.0.1:$port"

# A second invocation must fail before touching this invocation's adb server.
# The directory is removed only by its exact owning wrapper process.
run_lock="/tmp/aim-cts-tradefed-$port.lock"
if ! mkdir "$run_lock" 2>/dev/null; then
  echo "$serial: another CTS wrapper owns $run_lock; inspect its owner before cleanup" >&2
  exit 1
fi
printf '%s\n' "$$" > "$run_lock/pid"
trap '[[ "$(cat "$run_lock/pid" 2>/dev/null)" != "$$" ]] || { rm "$run_lock/pid"; rmdir "$run_lock"; }' EXIT

# An adb server of this run's own, apart from the Mac's other devices.
export ANDROID_ADB_SERVER_PORT=$((port + 1))
adb start-server
tradefed=
# Tradefed's processes (a group of their own) and the adb server go with
# this script, however it ends.
cleanup() {
  if [[ -n "$tradefed" ]]; then kill -KILL -- "-$tradefed" 2> /dev/null || true; fi
  adb kill-server || true
  if [[ "$(cat "$run_lock/pid" 2>/dev/null)" == "$$" ]]; then
    rm "$run_lock/pid"
    rmdir "$run_lock"
  fi
}
trap cleanup EXIT
trap 'exit 1' HUP INT TERM ALRM

# The Mac's key (adb's, made by its first server) is one the device's user
# allowed ("Always allow from this computer"): adbd checks adb_keys before
# it asks, and the lightweight shell has no dialog to ask with.
key="$(cat "$HOME/.android/adbkey.pub")"
"$root/tools/guest-shell.sh" ${shell_args[@]+"${shell_args[@]}"} "$data" "grep -qxF '$key' /data/misc/adb/adb_keys 2>/dev/null ||
  { echo '$key' >> /data/misc/adb/adb_keys; };
  chown system:shell /data/misc/adb/adb_keys && chmod 0640 /data/misc/adb/adb_keys"

deadline=$((SECONDS + 120))
until adb connect "$serial" > /dev/null && [[ "$(adb -s "$serial" get-state 2> /dev/null)" == device ]]; do
  ((SECONDS < deadline)) || { echo "$serial: no authorized adbd within 120 s" >&2; adb devices -l >&2; exit 1; }
  sleep 2
done

# The classic console (the ATS console's jars are not fetched); no usage
# statistics leave the Mac.
set -m
PATH="$jdk:${build_tools[0]}:$PATH" USE_ATS=false DISABLE_CLEARCUT=1 \
  bash "$harness/tools/cts-tradefed" run commandAndExit cts -s "$serial" \
  --skip-device-info --skip-preconditions "$@" < /dev/null &
tradefed=$!
set +m
cts_exit_code=0
wait "$tradefed" || cts_exit_code=$?
echo "results: $(ls -td "$harness"/results/*/ 2> /dev/null | head -1)"
exit "$cts_exit_code"
