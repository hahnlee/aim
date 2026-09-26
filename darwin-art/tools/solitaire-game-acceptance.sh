#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
apk="${1:-$root/_build/aosp-apks/SolitaireCG-fdroid.apk}"
output="$root/_build/solitaire-game-acceptance"

[[ -f "$apk" ]] || {
  echo "missing SolitaireCG APK: $apk" >&2
  exit 66
}
mkdir -p "$output"

log="$output/solitaire.log"
env \
  DARWIN_ART_WINDOW_SCALE=2 \
  DARWIN_ART_DEBUG_POINTER=1 \
  DARWIN_ART_DEBUG_VIEW_TREE=1 \
  DARWIN_ART_DEBUG_INPUT_LATENCY=1 \
  DARWIN_ART_TEST_POINTER_CLICK='180,300' \
  DARWIN_ART_TEST_POINTER_AFTER_SEQUENCE_DRAG='180,300;200,330;220,360;240,390' \
  DARWIN_ART_TEST_POINTER_AFTER_DRAG_SEQUENCE='240,390,1200' \
  "$root/tools/run-android-apk-app.sh" "$apk" 8 >"$log" 2>&1

grep -a -F 'darwin-art: launch package=net.sourceforge.solitaire_cg ' "$log" >/dev/null
grep -a -F 'net.sourceforge.solitaire_cg.SolitaireView' "$log" >/dev/null
grep -a -F 'MotionEvent ABI2 action=0 consumed=1 path=input-channel' "$log" >/dev/null
grep -a -E 'MotionEvent ABI2 action=2 .*path=input-channel' "$log" >/dev/null
if grep -a -E \
    'FATAL EXCEPTION|Fatal signal|SIG(SEGV|BUS|ABRT|TRAP)|runtime abort|Activity.onCreate\(\) threw' \
    "$log" >/dev/null; then
  echo 'SolitaireCG game acceptance observed a runtime crash' >&2
  exit 1
fi

echo 'solitaire-game-acceptance: PASS native=0 custom-view=SolitaireView drag=consumed crash=0'
echo "logs=$output"
