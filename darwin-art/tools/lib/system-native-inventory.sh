#!/bin/bash
# Validate pinned native bytes before packaging. This does not prove that their
# dependency graph loads, or that the Android runtime is ready to start.
darwin_art_verify_system_native_inventory() (
  set -euo pipefail
  local input="$1" manifest="$2" expected guest actual
  [[ "$input" = /* && -d "$input" && ! -L "$input" && -f "$manifest" ]] || return 64
  while read -r expected guest; do
    [[ -n "$expected" && "$expected" != \#* ]] || continue
    [[ "$expected" =~ ^[0-9a-f]{64}$ && "$guest" == /system/lib64/* &&
       "$guest" != *..* && "$guest" != *$'\t'* && "$guest" != *' '* ]] || return 64
    [[ -f "$input$guest" && ! -L "$input$guest" ]] || {
      echo "missing pinned Android native file: $input$guest" >&2
      return 69
    }
    actual="$(shasum -a 256 "$input$guest")" || return
    [[ "${actual%% *}" == "$expected" ]] || {
      echo "Android native file hash mismatch: $input$guest" >&2
      return 65
    }
  done < "$manifest"
)
