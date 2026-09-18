#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
output="$root/_build/chrome-process-lifecycle-acceptance"
chrome="$(find "$root/_build/installed-apps/org.chromium.chrome" \
  -name base.apk -type f 2>/dev/null | head -1)"

# Installed APKs are normally exposed through the profile package store. Keep
# the legacy build output as the first choice, but make lifecycle acceptance
# follow the same immutable package mount used by run-android-apk-app.sh.
if [[ ! -f "$chrome" ]]; then
  profile_root="${DARWIN_ART_PROFILE_ROOT:-$HOME/Library/Application Support/DarwinART/profiles}"
  chrome="$(find "$profile_root" -path '*/mnt/packages/org.chromium.chrome/*/*/base.apk' \
    -type f 2>/dev/null | head -1)"
fi

[[ -f "$chrome" ]] || {
  echo 'missing installed Chrome APK' >&2
  exit 66
}
mkdir -p "$output"

cargo run -q --manifest-path "$root/Cargo.toml" -p art-bootstrap -- \
  build-runtime-support-dex >"$output/support-dex.log" 2>&1
cargo run -q --manifest-path "$root/Cargo.toml" -p art-bootstrap -- \
  audit-runtime-graphics-link-incremental >"$output/graphics-link.log" 2>&1
cargo build -q --manifest-path "$root/Cargo.toml" -p darwin-art-host --bin darwin-art-host

for iteration in 1 2; do
  log="$output/chrome-$iteration.log"
  app_data="$output/app-data-$iteration"
  mkdir -p "$app_data"
  env \
    DARWIN_ART_APP_DATA_ROOT="$app_data" \
    DARWIN_ART_WINDOW_SCALE=2 \
    DARWIN_ART_TEST_POINTER_HOLD_MS=50 \
    DARWIN_ART_DEBUG_POINTER=1 \
    DARWIN_ART_DEBUG_WINDOW_LAYERS=1 \
    DARWIN_ART_DEBUG_VIEW_TEXT=1 \
    DARWIN_ART_TEST_POINTER_SEQUENCE='0,0,0;315,610,6000;180,100,500' \
    "$root/tools/run-android-apk-app.sh" "$chrome" 12 >"$log" 2>&1

  grep -a -F 'app:id/new_tab_menu_id' "$log" >/dev/null
  grep -a -F 'window remove argc=1 session=true' "$log" >/dev/null
  if grep -a -E \
      'Native thread exited without calling DetachCurrentThread|FATAL EXCEPTION|Fatal signal|SIG(SEGV|BUS|ABRT|TRAP)|runtime abort' \
      "$log" >/dev/null; then
    echo "Chrome lifecycle iteration $iteration observed a runtime crash" >&2
    exit 1
  fi

  child_count=0
  while IFS= read -r pid; do
    [[ -n "$pid" ]] || continue
    child_count=$((child_count + 1))
    if kill -0 "$pid" 2>/dev/null; then
      echo "Chrome service child $pid survived iteration $iteration" >&2
      exit 1
    fi
  done < <(grep -a -oE 'ChildProcessService pid=[0-9]+' "$log" | \
    sed 's/.*=//' | sort -nu)
  ((child_count > 0)) || {
    echo "Chrome lifecycle iteration $iteration spawned no service child" >&2
    exit 1
  }
done

echo 'chrome-process-lifecycle-acceptance: PASS iterations=2 new-tab=2 JNI-detach-crash=0 service-children=reaped'
echo "logs=$output"
