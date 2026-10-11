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
guest_shell=
host_tool_helper=
harness_override=
shell_args=()
host_tools=
selected_java=
selected_adb=
selected_aapt2=
owner_admission_fd=
selected_owner=
while [[ "${1:-}" == --* ]]; do
  case "$1" in
    --repository-root|--guest-shell|--host-tool-helper|--harness)
      [[ $# -ge 2 ]] || { echo "$1 requires a path" >&2; exit 2; }
      case "$1" in
        --repository-root) root="$2" ;;
        --guest-shell) guest_shell="$2" ;;
        --host-tool-helper) host_tool_helper="$2" ;;
        --harness) harness_override="$2" ;;
      esac
      shift 2 ;;
    --owner-admission-fd|--cts-owner)
      [[ $# -ge 2 ]] || { echo "$1 requires a value" >&2; exit 2; }
      case "$1" in
        --owner-admission-fd) owner_admission_fd="$2" ;;
        --cts-owner) selected_owner="$2" ;;
      esac
      shift 2 ;;
    --cts-java|--cts-adb|--cts-aapt2)
      [[ $# -ge 2 && -x "$2" ]] || { echo "$1 requires an executable" >&2; exit 2; }
      case "$1" in
        --cts-java) selected_java="$2" ;;
        --cts-adb) selected_adb="$2" ;;
        --cts-aapt2) selected_aapt2="$2" ;;
      esac
      shift 2 ;;
    --linux-run)
      [[ $# -ge 2 && -x "$2" ]] || { echo '--linux-run requires an executable path' >&2; exit 2; }
      shell_args=(--linux-run "$2"); shift 2 ;;
    --cts-host-toolsdir)
      [[ $# -ge 2 && -d "$2" && -z "$host_tools" ]] || { echo '--cts-host-toolsdir requires one directory' >&2; exit 2; }
      host_tools="$2"; shift 2 ;;
    *) echo "unknown wrapper option: $1" >&2; exit 2 ;;
  esac
done
[[ $# -ge 2 ]] || { sed -n '7,10p' "$0" >&2; exit 2; }
data="$1"
port="$2"
shift 2
harness="${harness_override:-$root/_build/cts-tradefed/android-cts}"
guest_shell="${guest_shell:-$root/tools/guest-shell.sh}"
host_tool_helper="${host_tool_helper:-$root/tools/lib/cts_host_tools.py}"
[[ -f "$harness/tools/cts-tradefed" ]] || { echo "no harness: run tools/cts-module.py --tradefed" >&2; exit 1; }
if [[ -n "$host_tools" ]]; then
  selected_module=
  previous=
  for argument in "$@"; do
    [[ "$argument" != *module-dir-path* ]] || { echo 'conflicting module-dir-path selection' >&2; exit 2; }
    if [[ "$previous" == -m || "$previous" == --module ]]; then selected_module="$argument"; fi
    previous="$argument"
  done
  [[ "$selected_module" == CtsStagedInstallHostTestCases ]] || {
    echo '--cts-host-toolsdir requires explicit CtsStagedInstallHostTestCases selection' >&2; exit 2;
  }
  selection="$(python3 "$host_tool_helper" "$host_tools" "$root" "$harness")"
  module_arg="$(python3 -c 'import json,sys; print(json.load(sys.stdin)["module_arg"])' <<< "$selection")"
  echo "cts host-tool selection: $selection"
  set -- "$@" --module-arg "$module_arg"
fi
[[ "$owner_admission_fd" == 198 && -x "$selected_owner" && -n "$selected_java" && -n "$selected_adb" && -n "$selected_aapt2" ]] || {
  echo 'CTS wrapper requires the owned supervisor and explicit selected tools' >&2; exit 2;
}
jdk="$(dirname "$selected_java")"
build_tools=("$(dirname "$selected_aapt2")")
export PATH="$jdk:${build_tools[0]}:$(dirname "$selected_adb"):$PATH"
[[ "$(command -v java)" == "$selected_java" && "$(command -v adb)" == "$selected_adb" && "$(command -v aapt2)" == "$selected_aapt2" ]] || {
  echo 'selected CTS PATH executable differs' >&2; exit 2;
}
[[ "$(java -version 2>&1 | head -1)" == *'21.'* ]] || { echo 'selected JDK21 required' >&2; exit 2; }
serial="127.0.0.1:$port"

# The direct Rust parent admits our actual PID and birth on its private socket.
"$selected_owner" --admit "$owner_admission_fd"
exec 198>&-
export ANDROID_ADB_SERVER_PORT=$((port + 1))

# The Mac's key (adb's, made by its first server) is one the device's user
# allowed ("Always allow from this computer"): adbd checks adb_keys before
# it asks, and the lightweight shell has no dialog to ask with.
key="$(cat "$HOME/.android/adbkey.pub")"
"$guest_shell" --repository-root "$root" ${shell_args[@]+"${shell_args[@]}"} "$data" "grep -qxF '$key' /data/misc/adb/adb_keys 2>/dev/null ||
  { echo '$key' >> /data/misc/adb/adb_keys; };
  chown system:shell /data/misc/adb/adb_keys && chmod 0640 /data/misc/adb/adb_keys"

deadline=$((SECONDS + 120))
until adb connect "$serial" > /dev/null && [[ "$(adb -s "$serial" get-state 2> /dev/null)" == device ]]; do
  ((SECONDS < deadline)) || { echo "$serial: no authorized adbd within 120 s" >&2; adb devices -l >&2; exit 1; }
  sleep 2
done

# The classic console (the ATS console's jars are not fetched); no usage
# statistics leave the Mac.
# The pinned Darwin launcher chooses JAVA_BINARY=java and calls it once.
# Exec preserves our admitted PID through the unchanged official launcher to JVM.
printf -v java_definition 'java() { exec %q "$@"; }' "$selected_java"
eval "$java_definition"
export -f java
export USE_ATS=false DISABLE_CLEARCUT=1
exec bash "$harness/tools/cts-tradefed" run commandAndExit cts -s "$serial" \
  --skip-device-info --skip-preconditions "$@" < /dev/null
