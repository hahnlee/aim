#!/bin/bash
set -euo pipefail

script_dir="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$script_dir/.." && pwd)"

# This is the narrow Android-side gate.  It compiles the real, checksum-
# verified Layoutlib registrar with the Darwin GPU patch and proves that the
# CPU-forcing hook is no longer active in GPU mode.  It does not claim that a
# Metal drawable was presented; that is covered by the HWUI Metal replay gate.
output="$(
  "$root/tools/build-android16-android-graphics-jni.sh" --registrar-only
)"
[[ "$output" == *"gpu-mode=1"* ]] || {
  echo "android16-hwui-gpu-layoutlib: registrar was not built in GPU mode" >&2
  echo "$output" >&2
  exit 2
}
fallback_output="$(
  "$root/tools/build-android16-android-graphics-jni.sh" --registrar-only-cpu
)"
[[ "$fallback_output" == *"gpu-mode=0"* ]] || {
  echo "android16-hwui-gpu-layoutlib: CPU fallback registrar changed unexpectedly" >&2
  echo "$fallback_output" >&2
  exit 2
}

echo "android16-hwui-gpu-layoutlib: cpu-layoutlib-override=disabled gpu-mode=1"
echo "$output"
echo "android16-hwui-gpu-layoutlib: cpu-fallback=preserved gpu-mode=0"
echo "$fallback_output"
