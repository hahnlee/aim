#!/usr/bin/env bash
set -euo pipefail

root_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
output_dir="${AIM_RIPPLE_OUTPUT_DIR:-$root_dir/_build/hwui-ripple-capture}"
pointer="${AIM_TEST_POINTER_CLICK:-180,320}"
hold_ms="${AIM_TEST_POINTER_HOLD_MS:-420}"

mkdir -p "$output_dir"
rm -f "$output_dir"/frame-*.png "$output_dir"/ripple.mp4 "$output_dir"/ripple.gif

window_id() {
  swift -e '
import CoreGraphics
let options: CGWindowListOption = [.optionOnScreenOnly, .excludeDesktopElements]
if let windows = CGWindowListCopyWindowInfo(options, kCGNullWindowID) as? [[String: Any]] {
  for window in windows {
    let owner = window[kCGWindowOwnerName as String] as? String ?? ""
    let name = window[kCGWindowName as String] as? String ?? ""
    let layer = window[kCGWindowLayer as String] as? Int ?? -1
    let number = window[kCGWindowNumber as String] as? Int ?? 0
    if layer == 0 && (owner.localizedCaseInsensitiveContains("aim") ||
                      name.localizedCaseInsensitiveContains("aim")) {
      print(number)
      break
    }
  }
}' 2>/dev/null
}

log_file="$output_dir/probe.log"
(
  cd "$root_dir"
  AIM_TEST_POINTER_CLICK="$pointer" \
  AIM_TEST_POINTER_HOLD_MS="$hold_ms" \
    cargo run -q -p art-bootstrap -- probe-runtime-button-window
) >"$log_file" 2>&1 &
probe_pid=$!

frame_index=0
deadline=$((SECONDS + 90))
while kill -0 "$probe_pid" 2>/dev/null && ((SECONDS < deadline)); do
  id="$(window_id || true)"
  if [[ -n "$id" ]]; then
    frame_path="$output_dir/frame-$(printf '%05d' "$frame_index").png"
    if screencapture -x -l "$id" "$frame_path" 2>/dev/null; then
      frame_index=$((frame_index + 1))
    fi
  fi
  sleep 0.04
done

set +e
wait "$probe_pid"
probe_status=$?
set -e

if (( frame_index == 0 )); then
  echo "hwui-ripple-capture: no aim window frames captured" >&2
  cat "$log_file" >&2
  exit 1
fi

ffmpeg -hide_banner -loglevel error -y \
  -framerate 24 -i "$output_dir/frame-%05d.png" \
  -vf "format=yuv420p" -c:v libx264 -movflags +faststart \
  "$output_dir/ripple.mp4"
ffmpeg -hide_banner -loglevel error -y \
  -framerate 12 -i "$output_dir/frame-%05d.png" \
  -vf "fps=12,scale=400:-1:flags=lanczos,split[s0][s1];[s0]palettegen=max_colors=128[p];[s1][p]paletteuse=dither=sierra2_4a" \
  -loop 0 "$output_dir/ripple.gif"

echo "hwui-ripple-capture: frames=$frame_index probe_status=$probe_status"
echo "hwui-ripple-capture: mp4=$output_dir/ripple.mp4"
echo "hwui-ripple-capture: gif=$output_dir/ripple.gif"
exit "$probe_status"
