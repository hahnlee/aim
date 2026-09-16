#!/bin/bash
# Launch-time selection only. The embedded Rust host owns image validation,
# installation, locking and lifetime; build tools own archive production.
darwin_art_runtime_system_archive() (
  set -euo pipefail
  local runtime_root="$1" mode="$2" archive
  case "$mode" in
    packaged) archive="$runtime_root/android/system-root.tar" ;;
    development) archive="$runtime_root/_build/android-system-image/system-root.tar" ;;
    *) echo "unknown runtime image mode: $mode" >&2; return 64 ;;
  esac
  [[ -f "$archive" && ! -L "$archive" ]] || {
    echo "runtime system image missing: $archive" >&2
    echo "build the system image before launching; launch does not synthesize a replacement" >&2
    return 69
  }
  printf '%s\n' "$archive"
)

darwin_art_prepare_runtime_system_image() (
  set -euo pipefail
  local archive
  archive="$(darwin_art_runtime_system_archive "$1" "$3")" || return
  "$2" --prepare-system-image "$archive" "$4"
)
