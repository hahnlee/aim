#!/usr/bin/env bash
# Physical acceptance for Android-owned task geometry (ADR 0008): a real AppKit
# edge drag, real CGEvent clicks and captured frames, plus runtime evidence that
# only the resized task receives revisions and that Activities without
# configChanges go through the framework relaunch path.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
output="${DARWIN_ART_ACCEPTANCE_OUTPUT:-$(mktemp -d "${TMPDIR:-/tmp}/window-geometry-acceptance.XXXXXX")}"
calculator="$root/_build/aosp-apks/ExactCalculator-api28.apk"
clock="$root/_build/aosp-apks/DeskClock-api29.apk"
daemon_log="${HOME}/Library/Application Support/DarwinART/profiles/default/darwin-artd.log"
ctl="$root/target/release/darwin-artctl"
input="$root/tools/macos-window-input.swift"
observer="$root/tools/macos-window-observe.swift"
resize="$root/tools/macos-window-resize.swift"
move="$root/tools/macos-window-move.swift"
title_bar_points=28
mkdir -p "$output"

for required in "$calculator" "$clock" "$ctl" "$daemon_log"; do
  [[ -e "$required" ]] || { echo "missing acceptance input: $required" >&2; exit 66; }
done

fail() { echo "window-geometry-acceptance: FAIL $*" >&2; echo "logs=$output" >&2; exit 1; }
package_pid() { "$ctl" ps | awk -F '\t' -v package="$1" '$2 == package { print $1; exit }'; }
log_offset() { wc -c <"$daemon_log" | tr -d ' '; }
log_since() { tail -c "+$(( $1 + 1 ))" "$daemon_log"; }
count_since() { log_since "$1" | LC_ALL=C grep -a -c -E "$2" || true; }

launch() {
  local apk="$1" package="$2" pid=""
  [[ -z "$(package_pid "$package")" ]] || fail "$package is already running"
  env DARWIN_ART_ASYNC_LAUNCH=1 DARWIN_ART_WINDOW_SCALE=2 \
    DARWIN_ART_DEBUG_INPUT_LATENCY=1 DARWIN_ART_DEBUG_POINTER=1 \
    "$root/tools/run-android-apk-app.sh" "$apk" 0 >"$output/$package-launch.log" 2>&1
  for _ in {1..300}; do
    pid="$(package_pid "$package")"
    [[ -n "$pid" ]] && { echo "$pid"; return 0; }
    sleep 0.05
  done
  fail "$package did not start"
}

wait_text() {
  local pid="$1" name="$2" pattern="$3" mode="${4:-}"
  for _ in {1..80}; do
    if swift "$observer" "$pid" "$output/$name.png" $mode >"$output/$name.txt" 2>/dev/null &&
        grep -E "$pattern" "$output/$name.txt" >/dev/null; then
      return 0
    fi
    sleep 0.5
  done
  return 1
}

offset="$(log_offset)"
calc_pid="$(launch "$calculator" com.android.calculator2)"
wait_text "$calc_pid" calculator-ready '^(DEL|CLR)$' || fail "Calculator keypad not visible"
clock_pid="$(launch "$clock" com.android.deskclock)"
wait_text "$clock_pid" deskclock-ready 'ALARM|TIMER|STOPWATCH' || fail "DeskClock not visible"
# Separate the windows so the edge drag belongs to Calculator only.
swift "$move" "$clock_pid" 520 0 >/dev/null
# Return focus with a physical click on Calculator's (inert) display area.
swift "$input" click-content "$calc_pid" 640 20,20,300
count_since "$offset" "task prepared pid=$calc_pid .*r1 720x1280" | grep -q '^1$' ||
  fail "Calculator did not start from its own portrait task revision"

# 1. Real edge drag of Calculator to a landscape extent.
resize_offset="$(log_offset)"
swift "$resize" "$calc_pid" 700 560 >"$output/calculator-resize.txt" || true
grep -q "pid=$calc_pid" "$output/calculator-resize.txt" || fail "resize tool did not run"
sleep 3
wait_text "$calc_pid" calculator-resized 'sin|cos|tan' ||
  fail "Calculator did not re-render its landscape layout"
[[ "$(count_since "$resize_offset" "publish pid=$calc_pid .*reason=host resize")" -gt 0 ]] ||
  fail "no task revision for the Calculator resize"
[[ "$(count_since "$resize_offset" "activity relaunch pid=$calc_pid ")" -gt 0 ]] ||
  fail "Calculator (no configChanges) was not relaunched"
[[ "$(count_since "$resize_offset" "publish pid=$clock_pid ")" == 0 ]] ||
  fail "resizing Calculator published a revision for DeskClock"
[[ "$(count_since "$resize_offset" "Caught a RuntimeException|main Looper failed|FATAL")" == 0 ]] ||
  fail "runtime exception during resize"

# 2. Five-point click map at the new extent: corners and centre of the keypad.
height="$(sed -n 's/.* new={.*height=\([0-9.]*\)}.*/\1/p' "$output/calculator-resize.txt")"
width="$(sed -n 's/.* new={.*width=\([0-9.]*\) height.*/\1/p' "$output/calculator-resize.txt")"
content_height="$(awk -v h="$height" -v t="$title_bar_points" 'BEGIN { printf "%d", h - t }')"
column() { awk -v w="$width" -v f="$1" 'BEGIN { printf "%d", w * f }'; }
row() { awk -v h="$content_height" -v f="$1" 'BEGIN { printf "%d", h * f }'; }
# Landscape keypad: digits (left 40%), operators, advanced panel (right 40%).
swift "$input" click-content "$calc_pid" "$content_height" \
  "$(column 0.075),$(row 0.37),400" "$(column 0.925),$(row 0.36),400" \
  "$(column 0.45),$(row 0.55),400" "$(column 0.925),$(row 0.92),400" \
  "$(column 0.075),$(row 0.92),800"
wait_text "$calc_pid" calculator-map '7%[x×X]' top || fail "five-point click map missed (see calculator-map.txt)"

log_since "$offset" >"$output/geometry.log"
echo "window-geometry-acceptance: PASS calculator=$calc_pid deskclock=$clock_pid map=7%x√."
echo "logs=$output"
