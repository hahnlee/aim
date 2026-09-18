#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
output="$root/_build/android-window-keyboard-acceptance"
calculator="$root/_build/aosp-apks/ExactCalculator-api28.apk"
calendar="$root/_build/aosp-apks/Calendar-api29.apk"
chrome="$(find "$root/_build/installed-apps/org.chromium.chrome" -name base.apk -type f 2>/dev/null | head -1)"

for apk in "$calculator" "$calendar" "$chrome"; do
  [[ -f "$apk" ]] || {
    echo "missing keyboard acceptance APK: $apk" >&2
    exit 66
  }
done
mkdir -p "$output"

cargo run -q --manifest-path "$root/Cargo.toml" -p art-bootstrap -- build-runtime-support-dex \
  >"$output/support-dex.log" 2>&1
cargo run -q --manifest-path "$root/Cargo.toml" -p art-bootstrap -- \
  audit-runtime-graphics-link-incremental >"$output/graphics-link.log" 2>&1
cargo build -q --manifest-path "$root/Cargo.toml" -p darwin-art-host --bin darwin-art-host

common_env=(
  DARWIN_ART_WINDOW_SCALE=2
  DARWIN_ART_TEST_POINTER_HOLD_MS=50
  DARWIN_ART_TEST_KEY_AFTER_POINTER_DELAY_MS=600
  DARWIN_ART_TEST_KEY_INTERVAL_MS=120
  DARWIN_ART_DEBUG_INPUT_LATENCY=1
  DARWIN_ART_DEBUG_POINTER=1
  DARWIN_ART_DEBUG_WINDOW_LAYERS=1
  DARWIN_ART_DEBUG_VIEW_TEXT=1
)

calculator_select_log="$output/calculator-select.log"
env "${common_env[@]}" \
  DARWIN_ART_TEST_POINTER_SEQUENCE='0,0,0;340,45,2500' \
  DARWIN_ART_TEST_KEY_AFTER_POINTER_SEQUENCE='20,23' \
  "$root/tools/run-android-apk-app.sh" "$calculator" 7 \
  >"$calculator_select_log" 2>&1

grep -a -F 'ART Android window focus=0 changed=1' "$calculator_select_log" >/dev/null
grep -a -F 'ART Android window focus=1 changed=1' "$calculator_select_log" >/dev/null
grep -a -F 'KeyEvent action=0 key=20 device=1 window=subwindow' \
  "$calculator_select_log" >/dev/null
grep -a -F 'KeyEvent action=0 key=23 device=1 window=subwindow' \
  "$calculator_select_log" | grep -a -F 'handled=1' >/dev/null
grep -a -F 'window remove argc=1 session=true' "$calculator_select_log" >/dev/null

calculator_restore_log="$output/calculator-restore.log"
env "${common_env[@]}" \
  DARWIN_ART_TEST_KEY_INTERVAL_MS=150 \
  DARWIN_ART_TEST_POINTER_SEQUENCE='0,0,0;340,45,2500' \
  DARWIN_ART_TEST_KEY_AFTER_POINTER_SEQUENCE='111,61' \
  "$root/tools/run-android-apk-app.sh" "$calculator" 7 \
  >"$calculator_restore_log" 2>&1

grep -a -F 'KeyEvent action=0 key=111 device=1 window=subwindow' \
  "$calculator_restore_log" | grep -a -F 'handled=1' >/dev/null
grep -a -F 'window remove argc=1 session=true' "$calculator_restore_log" >/dev/null
grep -a -F 'KeyEvent action=0 key=61 device=1 window=subwindow' \
  "$calculator_restore_log" >/dev/null

calendar_log="$output/calendar.log"
env "${common_env[@]}" \
  DARWIN_ART_TEST_POINTER_SEQUENCE='0,0,0;100,30,2500' \
  DARWIN_ART_TEST_KEY_AFTER_POINTER_SEQUENCE='20,23' \
  "$root/tools/run-android-apk-app.sh" "$calendar" 6 \
  >"$calendar_log" 2>&1

grep -a -F 'window frame request=419x336 layout=419x336 output=419x336 at=0,80 type=1002' \
  "$calendar_log" >/dev/null
grep -a -F 'KeyEvent action=0 key=20 device=1 window=subwindow' \
  "$calendar_log" >/dev/null
grep -a -F 'KeyEvent action=0 key=23 device=1 window=subwindow' \
  "$calendar_log" | grep -a -F 'handled=1' >/dev/null
grep -a -F 'window remove argc=1 session=true' "$calendar_log" >/dev/null

chrome_log="$output/chrome.log"
env "${common_env[@]}" \
  DARWIN_ART_TEST_KEY_AFTER_POINTER_DELAY_MS=900 \
  DARWIN_ART_TEST_KEY_INTERVAL_MS=150 \
  DARWIN_ART_TEST_POINTER_SEQUENCE='0,0,0;315,610,6000' \
  DARWIN_ART_TEST_KEY_AFTER_POINTER_SEQUENCE='20,111' \
  "$root/tools/run-android-apk-app.sh" "$chrome" 8 \
  >"$chrome_log" 2>&1

grep -a -F 'view=org.chromium.chrome.browser.ui.bottombar.BottomBarAppMenu' \
  "$chrome_log" >/dev/null
grep -a -F 'KeyEvent action=0 key=20 device=1 window=subwindow' \
  "$chrome_log" >/dev/null
grep -a -F 'KeyEvent action=0 key=111 device=1 window=subwindow' \
  "$chrome_log" | grep -a -F 'handled=1' >/dev/null
grep -a -F 'window remove argc=1 session=true' "$chrome_log" >/dev/null

if grep -a -E 'FATAL EXCEPTION|Fatal signal|SIG(SEGV|BUS|ABRT|TRAP)|runtime abort' \
    "$calculator_select_log" "$calculator_restore_log" "$calendar_log" \
    "$chrome_log" >/dev/null; then
  echo 'Android focused-window keyboard acceptance observed a runtime crash' >&2
  exit 1
fi

echo 'android-window-keyboard-acceptance: PASS Calculator=select+escape+restore Calendar=DPAD-select Chrome=DPAD+escape focused-window=single-owner'
echo "logs=$output"
