#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
output="$root/_build/android-window-menu-acceptance"
calculator="$root/_build/aosp-apks/ExactCalculator-api28.apk"
calendar="$root/_build/aosp-apks/Calendar-api29.apk"
chrome="$(find "$root/_build/installed-apps/org.chromium.chrome" -name base.apk -type f 2>/dev/null | head -1)"

[[ -f "$calculator" ]] || {
  echo "missing unchanged AOSP Calculator APK: $calculator" >&2
  exit 66
}
[[ -f "$chrome" ]] || {
  echo "missing installed Chrome APK" >&2
  exit 66
}
[[ -f "$calendar" ]] || {
  echo "missing unchanged AOSP Calendar APK: $calendar" >&2
  exit 66
}
mkdir -p "$output"
run_output="$(mktemp -d "$output/run.XXXXXX")"
manifest="$run_output/manifest.log"
printf 'run_start=%s pid=%s\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "$$" >"$manifest"
trap 'printf "run_end=%s\n" "$(date -u +%Y-%m-%dT%H:%M:%SZ)" >>"$manifest"' EXIT

# Never validate product APKs with stale compatibility classes. This command
# verifies source inventory, nested closure and original framework ownership.
cargo run -q --manifest-path "$root/Cargo.toml" -p art-bootstrap -- build-runtime-support-dex \
  >"$run_output/support-dex.log" 2>&1
cargo build -q --manifest-path "$root/Cargo.toml" -p darwin-art-host --bin darwin-art-host

common_env=(
  DARWIN_ART_WINDOW_SCALE=2
  DARWIN_ART_TEST_POINTER_HOLD_MS=50
  DARWIN_ART_DEBUG_INPUT_LATENCY=1
  DARWIN_ART_DEBUG_POINTER=1
  DARWIN_ART_DEBUG_WINDOW_LAYERS=1
  DARWIN_ART_DEBUG_VIEW_TEXT=1
)

calculator_log="$run_output/calculator.log"
env "${common_env[@]}" \
  DARWIN_ART_TEST_POINTER_SEQUENCE='0,0,0;340,45,2500;340,45,300;250,35,300' \
  "$root/tools/run-android-apk-app.sh" "$calculator" 8 \
  >"$calculator_log" 2>&1

grep -a -F 'view=android.widget.ActionMenuPresenter$OverflowMenuButton' \
  "$calculator_log" >/dev/null
grep -a -F 'window frame request=392x192 layout=392x192 output=392x192 at=320,8 type=1002' \
  "$calculator_log" >/dev/null
grep -a -F 'input window index=1 type=1002' "$calculator_log" | \
  grep -a -F 'bounds=320,8-712,200' >/dev/null
grep -a -E 'View text .* text=History$' "$calculator_log" >/dev/null
grep -a -F 'window remove argc=1 session=true' "$calculator_log" >/dev/null

calculator_outside_log="$run_output/calculator-outside.log"
env "${common_env[@]}" \
  DARWIN_ART_TEST_POINTER_SEQUENCE='0,0,0;340,45,2500;50,250,300' \
  "$root/tools/run-android-apk-app.sh" "$calculator" 8 \
  >"$calculator_outside_log" 2>&1

grep -a -F 'outside=1 hit=0' "$calculator_outside_log" >/dev/null
grep -a -F 'window remove argc=1 session=true' "$calculator_outside_log" >/dev/null
if grep -a -F 'app:id/digit_7' "$calculator_outside_log" >/dev/null; then
  echo 'Calculator popup leaked its outside DOWN into the Activity' >&2
  exit 1
fi

calculator_resize_log="$run_output/calculator-resize.log"
# Leave the popup ViewRoot alive after opening it. A third tap at 340,45
# selects History and removes the popup before the delayed resize begins.
env "${common_env[@]}" \
  DARWIN_ART_DEBUG_RESIZE=1 \
  DARWIN_ART_TEST_POINTER_SEQUENCE='0,0,0;340,45,2500' \
  DARWIN_ART_TEST_POINTER_SEQUENCE_POST_DELAY_MS=500 \
  DARWIN_ART_TEST_WINDOW_RESIZE='600x1000' \
  DARWIN_ART_TEST_WINDOW_RESIZE_AFTER_MS=500 \
  "$root/tools/run-android-apk-app.sh" "$calculator" 7 \
  >"$calculator_resize_log" 2>&1

grep -a -F 'window frame request=392x192 layout=392x192 output=392x192 at=320,8 type=1002' \
  "$calculator_resize_log" >/dev/null
grep -a -F 'ART Android resize: 720x1280 -> 600x1000' \
  "$calculator_resize_log" >/dev/null
grep -a -F 'window frame request=392x192 layout=392x192 output=392x192 at=208,8 type=1002' \
  "$calculator_resize_log" >/dev/null

calendar_log="$run_output/calendar.log"
env "${common_env[@]}" \
  DARWIN_ART_TEST_POINTER_SEQUENCE='0,0,0;100,30,800;100,70,300' \
  "$root/tools/run-android-apk-app.sh" "$calendar" 5 \
  >"$calendar_log" 2>&1

grep -a -F 'view=android.widget.Spinner' "$calendar_log" >/dev/null
grep -a -F 'window frame request=419x336 layout=419x336 output=419x336 at=0,80 type=1002' \
  "$calendar_log" >/dev/null
for label in Day Week Month; do
  grep -a -E "View text .* text=${label}$" "$calendar_log" >/dev/null
done
grep -a -F 'window remove argc=1 session=true' "$calendar_log" >/dev/null

chrome_log="$run_output/chrome.log"
env "${common_env[@]}" \
  DARWIN_ART_TEST_POINTER_SEQUENCE='0,0,0;315,610,6000;180,100,500' \
  "$root/tools/run-android-apk-app.sh" "$chrome" 13 \
  >"$chrome_log" 2>&1

grep -a -F 'view=org.chromium.chrome.browser.ui.bottombar.BottomBarAppMenu' \
  "$chrome_log" >/dev/null
grep -a -F 'window frame request=560x1233 layout=560x1233 output=560x1233 at=80,47 type=1002' \
  "$chrome_log" >/dev/null
grep -a -F 'input window index=1 type=1002' "$chrome_log" | \
  grep -a -F 'bounds=80,47-640,1280' >/dev/null
grep -a -F 'app:id/new_tab_menu_id' "$chrome_log" >/dev/null
grep -a -F 'window remove argc=1 session=true' "$chrome_log" >/dev/null

if grep -a -E 'FATAL EXCEPTION|Fatal signal|SIG(SEGV|BUS|ABRT|TRAP)|runtime abort|HandleUnexpectedSignal|ART signal context unavailable .* signal=[0-9]+' \
    "$calculator_log" "$calculator_outside_log" "$calculator_resize_log" \
    "$calendar_log" "$chrome_log" >/dev/null; then
  echo 'Android window menu acceptance observed a runtime crash' >&2
  exit 1
fi

echo 'android-window-menu-acceptance: PASS Calculator=History+outside-dismiss+resize Calendar=Day/Week/Month Chrome=New-tab Android-popup=ViewRoot+InputChannel+SurfaceFlinger'
echo "logs=$run_output"
