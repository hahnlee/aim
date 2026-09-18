#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
source "$root/tools/lib/aosp-core-apps-graphics-validation.sh"
output="${DARWIN_ART_ACCEPTANCE_OUTPUT:-$(mktemp -d "${TMPDIR:-/tmp}/aosp-core-apps-acceptance.XXXXXX")}"
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
observer="$root/tools/macos-window-observe.swift"
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

package_pid() {
  local package="$1"
  "$ctl" ps | awk -F '\t' -v package="$package" '$2 == package { print $1; exit }'
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

launch_app() {
  local apk="$1" package="$2" metadata_log="$3"
  if [[ -n "$(package_pid "$package")" ]]; then
    echo "$package is already running; close it explicitly before fresh-product acceptance" >&2
    return 1
  fi
  launch_offset="$(wc -c <"$daemon_log" | tr -d ' ')"
  env \
    DARWIN_ART_ASYNC_LAUNCH=1 \
    DARWIN_ART_WINDOW_SCALE=2 \
    DARWIN_ART_DEBUG_INPUT_LATENCY=1 \
    DARWIN_ART_DEBUG_POINTER=1 \
    DARWIN_ART_DEBUG_SURFACE_TRANSACTIONS=1 \
  "$root/tools/run-android-apk-app.sh" "$apk" 0 >"$metadata_log" 2>&1
  wait_for_package "$package"
}

calculator_metadata="$output/calculator-launch.log"
launch_app "$calculator" com.android.calculator2 "$calculator_metadata"
calculator_offset="$launch_offset"
calculator_pid="$active_pid"
calculator_ready=false
for _ in {1..60}; do
  if swift "$observer" "$active_pid" "$output/calculator-ready.png" \
      >"$output/calculator-ready.txt" 2>/dev/null &&
      grep -E '^(DEL|CLR)$' "$output/calculator-ready.txt" >/dev/null; then
    calculator_ready=true
    break
  fi
  sleep 0.5
done
[[ "$calculator_ready" == true ]] || {
  echo 'Calculator did not display its keypad' >&2
  exit 1
}
swift "$input" click-content "$active_pid" 640 \
  125,475,300 315,575,300 205,475,500
swift "$observer" "$active_pid" "$output/calculator-formula.png" top \
  >"$output/calculator-formula.txt"
grep -E '2[[:space:]]*\+[[:space:]]*3' "$output/calculator-formula.txt" >/dev/null || {
  echo 'Calculator did not visibly display formula 2+3' >&2
  exit 1
}
swift "$input" click-content-no-focus "$active_pid" 640 205,575,500
swift "$observer" "$active_pid" "$output/calculator-result.png" top \
  >"$output/calculator-result.txt"
grep -E '^5$' "$output/calculator-result.txt" >/dev/null || {
  echo 'Calculator did not visibly display result 5' >&2
  exit 1
}
capture_log_since "$calculator_offset" "$calculator_log"

[[ "$(grep -a -c 'AppKit pointer action=' "$calculator_log")" -ge 8 ]] || {
  echo 'Calculator did not receive four physical AppKit click pairs' >&2
  exit 1
}
for sequence in {1..8}; do
  grep -a -E "Input dispatch-return pid=$calculator_pid sequence=$sequence .*invoked=1 delivered=1 " "$calculator_log" >/dev/null || {
    echo "Calculator physical pointer sequence $sequence did not reach framework dispatch" >&2
    exit 1
  }
done
darwin_art_validate_hwui_blast_visible "$calculator_pid" "$calculator_log" || {
  echo 'Calculator did not publish a visible HWUI buffer transaction' >&2
  exit 1
}

clock_metadata="$output/deskclock-launch.log"
launch_app "$clock" com.android.deskclock "$clock_metadata"
clock_offset="$launch_offset"
clock_pid="$active_pid"
# Retired WMS diagnostic text is not a readiness contract. Verify actual
# visible content, then require physical dispatch acknowledgments and HWUI
# buffer publication below; a window alone is insufficient acceptance.
clock_visible=false
for _ in {1..60}; do
  swift "$observer" "$active_pid" "$output/deskclock-ready.png" \
    >"$output/deskclock-ready.txt"
  if grep -i -E 'alarm|timer|stopwatch' "$output/deskclock-ready.txt" >/dev/null; then
    clock_visible=true
    break
  fi
  sleep 0.5
done
[[ "$clock_visible" == true ]] || { echo 'DeskClock did not display its tabs' >&2; exit 1; }
# Click the actual Timer icon center, rather than the tab-label/gap region.
swift "$input" click-content "$active_pid" 640 200,40,500
timer_visible=false
for _ in {1..60}; do
  swift "$observer" "$active_pid" "$output/deskclock-timer.png" \
    >"$output/deskclock-timer.txt"
  # Vision may read the small italic hour suffix h as n; require the complete
  # zero-hour/minute/second timer string, not a clock-time or tab-label match.
  if grep -E '00[hHn].*00[mM].*00[sS]|00:00:00' "$output/deskclock-timer.txt" >/dev/null; then
    timer_visible=true
    break
  fi
  sleep 0.5
done
[[ "$timer_visible" == true ]] || {
  echo 'DeskClock did not transition to the Timer page' >&2
  exit 1
}
capture_log_since "$clock_offset" "$clock_log"

[[ "$(grep -a -c 'AppKit pointer action=' "$clock_log")" -ge 2 ]] || {
  echo 'DeskClock did not receive the physical Timer-tab click pair' >&2
  exit 1
}
for sequence in 1 2; do
  grep -a -E "Input dispatch-return pid=$clock_pid sequence=$sequence .*invoked=1 delivered=1 " "$clock_log" >/dev/null || {
    echo "DeskClock physical pointer sequence $sequence did not reach framework dispatch" >&2
    exit 1
  }
done
darwin_art_validate_hwui_blast_visible "$clock_pid" "$clock_log" || {
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
