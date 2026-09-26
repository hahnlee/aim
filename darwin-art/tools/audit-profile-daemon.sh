#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
temporary="$(mktemp -d "/tmp/dart-profile.XXXXXX")"
export DARWIN_ART_PROFILE_ROOT="$temporary/profiles"
export DARWIN_ART_PROFILE_IMAGE_SIZE="1g"
export DARWIN_ARTD_IDLE_SECONDS="30"
ctl="$root/target/release/darwin-artctl"
lease_pid=""

cleanup() {
  if [[ -n "$lease_pid" ]]; then
    kill "$lease_pid" 2>/dev/null || true
    wait "$lease_pid" 2>/dev/null || true
  fi
  "$ctl" shutdown >/dev/null 2>&1 || true
  for _ in {1..100}; do
    [[ ! -S "$DARWIN_ART_PROFILE_ROOT/default/control.sock" ]] && break
    sleep 0.05
  done
  [[ "$temporary" == /tmp/dart-profile.* ]] && rm -rf "$temporary"
}
trap cleanup EXIT

cargo build -q --release -p darwin-art-profile --bins
mount="$($ctl ensure)"
[[ -d "$mount/data/apps" && -d "$mount/storage/emulated/0" ]]
if "$root/target/release/darwin-artd" \
  --root "$DARWIN_ART_PROFILE_ROOT" --profile default --idle-seconds 1 \
  >/dev/null 2>&1; then
  echo "profile audit: a second daemon acquired the same profile" >&2
  exit 1
fi
touch "$mount/run/AuditCase" "$mount/run/auditcase"
[[ "$(find "$mount/run" -maxdepth 1 \( -name AuditCase -o -name auditcase \) | wc -l | tr -d ' ')" == "2" ]]

# Installed packages are PackageManagerService's packages.list (ADR 0009);
# write the line PMS writes for a shell-installed package.
package_list="$mount/data/apps/android.system/private-data/system/packages.list"
mkdir -p "$(dirname "$package_list")"
printf '%s\n' \
  'com.android.shell 2000 0 /data/user_de/0/com.android.shell platform:privapp:targetSdkVersion=36 none 0 36 1 @system' \
  'org.example.audit 10077 0 /data/user/0/org.example.audit default:targetSdkVersion=36 none 0 1 1 @null' \
  >"$package_list"
[[ "$("$ctl" list)" == "org.example.audit" ]]

"$ctl" exec org.example.audit /bin/sleep 2 &
lease_pid=$!
for _ in {1..100}; do
  "$ctl" ps 2>/dev/null | rg -q $'^[0-9]+\torg\.example\.audit$' && break
  sleep 0.02
done
"$ctl" status | rg -q 'mounted=true leases=1'
"$ctl" ps | rg -q $'^[0-9]+\torg\.example\.audit$'
if "$ctl" shutdown >/dev/null 2>&1; then
  echo "profile audit: shutdown incorrectly accepted an active lease" >&2
  exit 1
fi
wait "$lease_pid"
lease_pid=""
"$ctl" shutdown
for _ in {1..100}; do
  [[ ! -S "$DARWIN_ART_PROFILE_ROOT/default/control.sock" ]] && break
  sleep 0.05
done
[[ ! -S "$DARWIN_ART_PROFILE_ROOT/default/control.sock" ]]
mount="$($ctl ensure)"
[[ "$("$ctl" list)" == "org.example.audit" ]]
echo "profile-daemon-audit: PASS case-sensitive=true package-list-persistent=true process-exec-leased=true lease-protected=true"
