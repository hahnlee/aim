#!/bin/bash
set -euo pipefail

module_dir="$(cd "$(dirname "$0")" && pwd)"
probe_dir="$(mktemp -d "${TMPDIR:-/tmp}/darwin-foreign-threads.XXXXXX")"
# Retain the small test binaries for inspection; never touch runtime artifacts.
for sanitizer in address undefined thread; do
  xcrun clang++ -std=c++17 -pthread -O1 -g -fno-omit-frame-pointer \
    -fsanitize="$sanitizer" -I"$module_dir/include" \
    "$module_dir/foreign_thread_stress.cc" "$module_dir/src/provider.cc" \
    -o "$probe_dir/foreign-$sanitizer"
  ASAN_OPTIONS=detect_leaks=0:halt_on_error=1 \
    UBSAN_OPTIONS=halt_on_error=1:print_stacktrace=1 \
    TSAN_OPTIONS=halt_on_error=1 "$probe_dir/foreign-$sanitizer"
done
printf 'Foreign-thread sanitizer probes: %s\n' "$probe_dir"
