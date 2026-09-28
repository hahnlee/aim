#!/bin/bash
set -euo pipefail
module_dir="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$module_dir/../.." && pwd)"
process_dir="$module_dir/../bionic-process-state-facade"
probe_dir="$(mktemp -d "${TMPDIR:-/tmp}/darwin-signal-fault.XXXXXX")"
xcrun clang -std=c17 -O1 -g -I"$process_dir/include" \
  -c "$process_dir/src/shims.c" -o "$probe_dir/shims.o"
xcrun clang++ -std=c++17 -pthread -O1 -g -Wl,-dead_strip \
  -I"$module_dir/include" -I"$root/_aosp/art/sigchainlib" \
  "$module_dir/signal_fault_boundary_probe.cc" "$module_dir/src/provider.cc" \
  "$root/compat/darwin_sigchain.cc" "$probe_dir/shims.o" \
  -o "$probe_dir/fault-boundary"
"$probe_dir/fault-boundary"
printf 'Diagnostic-only executable: %s\n' "$probe_dir/fault-boundary"
