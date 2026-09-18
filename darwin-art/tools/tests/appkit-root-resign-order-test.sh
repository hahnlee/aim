#!/usr/bin/env bash
set -euo pipefail
root="$(cd "$(dirname "$0")/../.." && pwd)"
stage="$(mktemp -d "${TMPDIR:-/tmp}/appkit-root-resign.XXXXXX")"
trap 'rm -rf -- "$stage"' EXIT
"${CXX:-clang++}" -std=c++20 -fobjc-arc -O1 -g -Wall -Wextra -Werror \
  -fsanitize=address,undefined -fno-omit-frame-pointer -pthread -I"$root" \
  "$root/compat/window/desktop_root_events.mm" \
  "$root/compat/window/desktop_root_target.mm" \
  "$root/compat/window/desktop_root_surface.mm" \
  "$root/compat/window/appkit_window_delegate.mm" \
  "$root/compat/darwin_surface_bridge.mm" \
  "$root/compat/window/display_output.mm" \
  "$root/compat/window/composition_fence_monitor.cc" \
  "$root/compat/surfaceflinger/output_owner.cc" \
  "$root/tools/tests/appkit-root-resign-order-test.mm" \
  -framework AppKit -framework CoreFoundation -framework Metal -framework QuartzCore \
  -framework IOSurface -Wl,-dead_strip -o "$stage/test"
"$stage/test"
