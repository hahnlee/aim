#!/usr/bin/env bash
set -euo pipefail
root="$(cd "$(dirname "$0")/../.." && pwd)"
ninja="$root/_aosp/external/skia/third_party/ninja/ninja"
if [[ $# == 0 ]]; then
  graph="$root/_build/native-graph/build.ninja"
  set -- \
    "$root/_build/runtime-graphics-link-probe/libdarwin_art_runtime_graphics.dylib" \
    "$root/_build/runtime-link-probe/libdarwin_art_runtime.dylib"
elif [[ $# -ge 2 ]]; then
  graph="$1"
  shift
else
  echo 'usage: production-runtime-graph-boundary.sh [graph target ...]' >&2
  exit 2
fi
for target in "$@"; do
  # Inspect reachable dependencies, not the whole graph: genuine fixture
  # targets may coexist without becoming prerequisites of installed APK code.
  inputs="$("$ninja" -f "$graph" -t inputs "$target")"
  if forbidden="$(printf '%s\n' "$inputs" | rg '(^|/)probes/')"; then
    echo "production-runtime-graph-boundary: fixture dependency of $target:" >&2
    printf '%s\n' "$forbidden" >&2
    exit 1
  else
    search_status=$?
    [[ "$search_status" == 1 ]] || exit "$search_status"
  fi
done
echo 'production runtime graph: reachable probes/ inputs=0 PASS'
