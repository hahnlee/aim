#!/bin/bash
set -euo pipefail
module_dir="$(cd "$(dirname "$0")" && pwd)"
process_dir="$module_dir/../bionic-process-state-facade"
probe_dir="$(mktemp -d "${TMPDIR:-/tmp}/darwin-signal-cycles.XXXXXX")"
xcrun clang -std=c17 -O1 -g -I"$process_dir/include" \
  -c "$process_dir/src/shims.c" -o "$probe_dir/shims.o"
xcrun clang++ -std=c++17 -pthread -O1 -g -Wl,-dead_strip \
  -I"$module_dir/include" "$module_dir/signal_cycle_stress.cc" \
  "$module_dir/src/provider.cc" "$probe_dir/shims.o" \
  -o "$probe_dir/signal-cycles"
"$probe_dir/signal-cycles"
printf 'Signal-cycle probe: %s\n' "$probe_dir"
