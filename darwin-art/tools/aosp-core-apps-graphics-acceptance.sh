#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
output="$root/_build/aosp-core-apps-graphics-acceptance"
calculator="$root/_build/aosp-apks/ExactCalculator-api28.apk"
clock="$root/_build/aosp-apks/DeskClock-api29.apk"

[[ -f "$calculator" ]] || {
  echo "missing unchanged AOSP Calculator APK: $calculator" >&2
  exit 66
}
[[ -f "$clock" ]] || {
  echo "missing unchanged AOSP DeskClock APK: $clock" >&2
  exit 66
}
mkdir -p "$output"

calculator_log="$output/calculator.log"
clock_log="$output/deskclock.log"
daemon_log="${HOME}/Library/Application Support/DarwinART/profiles/default/darwin-artd.log"
ctl="$root/target/debug/darwin-artctl"
input="$root/tools/macos-window-input.swift"
active_pid=""
launch_offset=""

[[ -x "$ctl" ]] || {
  echo "missing profile control CLI: $ctl" >&2
  exit 66
}
[[ -f "$daemon_log" ]] || {
  echo "missing profile daemon log: $daemon_log" >&2
  exit 66
}

stop_active() {
  [[ -z "$active_pid" ]] || kill -TERM "$active_pid" 2>/dev/null || true
  active_pid=""
}
trap stop_active EXIT INT TERM

package_pid() {
  local package="$1"
  "$ctl" ps | awk -F '\t' -v package="$package" '$2 == package { print $1; exit }'
}

stop_package() {
  local package="$1" pid
  pid="$(package_pid "$package")"
  [[ -z "$pid" ]] || kill -TERM "$pid" 2>/dev/null || true
  for _ in {1..100}; do
    [[ -z "$(package_pid "$package")" ]] && return 0
    sleep 0.05
  done
  echo "could not stop stale $package process" >&2
  return 1
}

wait_for_package() {
  local package="$1"
  for _ in {1..300}; do
    active_pid="$(package_pid "$package")"
    [[ -n "$active_pid" ]] && return 0
    sleep 0.05
  done
  echo "$package did not enter the profile process registry" >&2
  return 1
}

capture_log_since() {
  local offset="$1" destination="$2"
  tail -c "+$((offset + 1))" "$daemon_log" >"$destination"
}

wait_for_log() {
  local offset="$1" pattern="$2"
  for _ in {1..300}; do
    tail -c "+$((offset + 1))" "$daemon_log" | grep -a -E "$pattern" >/dev/null && return 0
    sleep 0.05
  done
  return 1
}

launch_app() {
  local apk="$1" package="$2" metadata_log="$3"
  stop_package "$package"
  launch_offset="$(wc -c <"$daemon_log" | tr -d ' ')"
  env \
    DARWIN_ART_ASYNC_LAUNCH=1 \
    DARWIN_ART_WINDOW_SCALE=2 \
    DARWIN_ART_DEBUG_INPUT_LATENCY=1 \
    DARWIN_ART_DEBUG_POINTER=1 \
    DARWIN_ART_DEBUG_VIEW_TEXT=1 \
    DARWIN_ART_DEBUG_SURFACE_TRANSACTIONS=1 \
  "$root/tools/run-android-apk-app.sh" "$apk" 0 >"$metadata_log" 2>&1
  wait_for_package "$package"
}

calculator_metadata="$output/calculator-launch.log"
launch_app "$calculator" com.android.calculator2 "$calculator_metadata"
calculator_offset="$launch_offset"
wait_for_log "$calculator_offset" \
  'View text id=0x7f06002c bounds=0,0-720,139 .* text=$' || {
  echo 'Calculator did not finish its first Android layout' >&2
  exit 1
}
swift "$input" click-content "$active_pid" 640 \
  125,475,300 315,575,300 205,475,300 205,575,300
wait_for_log "$calculator_offset" 'View text id=0x7f06006b .* text=5$' || {
  echo 'Calculator result did not become 5' >&2
  exit 1
}
capture_log_since "$calculator_offset" "$calculator_log"
stop_package com.android.calculator2
active_pid=""

[[ "$(grep -a -c 'AppKit pointer action=' "$calculator_log")" -ge 8 ]] || {
  echo 'Calculator did not receive four physical AppKit click pairs' >&2
  exit 1
}
for sequence in {1..8}; do
  grep -a -E "InputChannel ingress->dispatch kind=pointer sequence=$sequence " "$calculator_log" >/dev/null || {
    echo "Calculator physical pointer sequence $sequence did not reach Android InputChannel" >&2
    exit 1
  }
done
grep -a -E 'View text id=0x7f06002c .* text=2\+3$' "$calculator_log" >/dev/null || {
  echo 'Calculator formula did not become 2+3' >&2
  exit 1
}
grep -a -E 'View text id=0x7f06006b .* text=5$' "$calculator_log" >/dev/null || {
  echo 'Calculator result did not become 5' >&2
  exit 1
}
grep -a -E 'SurfaceTransaction: update .* visible=1 .*buffer=1' "$calculator_log" >/dev/null || {
  echo 'Calculator did not publish a visible HWUI buffer transaction' >&2
  exit 1
}

clock_metadata="$output/deskclock-launch.log"
launch_app "$clock" com.android.deskclock "$clock_metadata"
clock_offset="$launch_offset"
wait_for_log "$clock_offset" \
  'WMS InputWindow publish .* visible=1 .*frame=\[0,0,720,1280\]' || {
  echo 'DeskClock did not publish its Android input window' >&2
  exit 1
}
swift "$input" click-content "$active_pid" 640 180,60,500
wait_for_log "$clock_offset" 'View text id=0x7f0a015b .* text=00h 00m 00s$' || {
  echo 'DeskClock did not transition to the Timer page' >&2
  exit 1
}
capture_log_since "$clock_offset" "$clock_log"
stop_package com.android.deskclock
active_pid=""

[[ "$(grep -a -c 'AppKit pointer action=' "$clock_log")" -ge 2 ]] || {
  echo 'DeskClock did not receive the physical Timer-tab click pair' >&2
  exit 1
}
for sequence in 1 2; do
  grep -a -E "InputChannel ingress->dispatch kind=pointer sequence=$sequence " "$clock_log" >/dev/null || {
    echo "DeskClock physical pointer sequence $sequence did not reach Android InputChannel" >&2
    exit 1
  }
done
grep -a -E 'View text id=0x7f0a015b .* text=00h 00m 00s$' "$clock_log" >/dev/null || {
  echo 'DeskClock did not transition to the Timer page' >&2
  exit 1
}
grep -a -E 'SurfaceTransaction: update .* visible=1 .*buffer=1' "$clock_log" >/dev/null || {
  echo 'DeskClock did not publish a visible HWUI buffer transaction' >&2
  exit 1
}

if grep -a -E 'FATAL EXCEPTION|SIG(SEGV|BUS|ABRT)|runtime abort' \
    "$calculator_log" "$clock_log" >/dev/null; then
  echo 'AOSP core-app graphics acceptance observed a runtime crash' >&2
  exit 1
fi

echo "aosp-core-apps-graphics-acceptance: PASS Calculator=2+3=5 DeskClock=Timer input=physical-CGEvent common-path=HWUI+SurfaceFlinger+Metal"
echo "logs=$output"
