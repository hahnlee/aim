#!/bin/bash
set -euo pipefail

test_root="$(cd "$(dirname "$0")/../.." && pwd)"
patch_file="$test_root/patches/art/0141-darwin-arm64-original-fault-context.patch"
stage="$(mktemp -d "${TMPDIR:-/tmp}/fault-context.XXXXXX")"
trap 'rm -rf -- "$stage"' EXIT

[[ -s "$patch_file" ]]

# The AOSP target is an external patch artifact, so git apply cannot check it
# in this checkout. Count every unified-diff hunk directly instead.
awk '
function check(   oldgot, newgot) {
  if (!have) return
  oldgot = old; newgot = new
  if (oldgot != oldwant || newgot != newwant) {
    printf("hunk %d: declared -%d +%d, counted -%d +%d\n",
           hunk, oldwant, newwant, oldgot, newgot) > "/dev/stderr"
    bad = 1
  }
}
/^@@ / {
  check(); hunk++
  line = $0
  sub(/^@@ -/, "", line)
  split(line, parts, " \\\+")
  split(parts[1], old_part, ",")
  split(parts[2], new_part, ",")
  sub(/ @@.*$/, "", new_part[2])
  oldwant = old_part[2] == "" ? 1 : old_part[2]
  newwant = new_part[2] == "" ? 1 : new_part[2]
  old = 0; new = 0; have = 1
  next
}
/^diff --git / { check(); have = 0; next }
have {
  marker = substr($0, 1, 1)
  if (marker == "-") old++
  else if (marker == "+") new++
  else if (marker == " ") { old++; new++ }
  else {
    print "invalid unified-diff line: " $0 > "/dev/stderr"
    bad = 1
  }
}
END { check(); exit bad }
' "$patch_file"

# Extract the actual added function, retaining production preprocessor
# branches, and compile it for arm64 against the real macOS SDK. No TLS query
# seam, production object or shared native build is linked here.
awk '
/^\+void DumpDarwinOriginalFaultContext/ { found = 1 }
found && /^\+}$/ { print substr($0, 2); exit }
found && /^\+/ { print substr($0, 2) }
' "$patch_file" > "$stage/function.inc"
grep -q 'DumpDarwinOriginalFaultContext' "$stage/function.inc"
{
  cat <<'EOF'
#include <mach/arm/thread_status.h>
#include <mach/mach.h>
#include <mach/mach_vm.h>
#include <signal.h>
#include <sys/ucontext.h>
#include <unistd.h>
#include "compat/diagnostics/fault_log_buffer.h"
namespace art {
EOF
  cat "$stage/function.inc"
  cat <<'EOF'
}
EOF
} > "$stage/fault-context.cc"

cxx_path="$(xcrun --find clang++)"
sdk_path="$(xcrun --sdk macosx --show-sdk-path)"
"$cxx_path" -target arm64-apple-macos15 -isysroot "$sdk_path" \
  -std=c++17 -Wall -Wextra -Werror -Wpedantic -I"$test_root" \
  -c "$stage/fault-context.cc" -o "$stage/fault-context.o"

if grep -Eq 'signal-safety promise|not async[- ]signal[- ]safe' "$patch_file"; then
  echo "fault context: strict arm64 extraction/compile PASS; Mach provenance is explicitly best-effort, not async-signal-safe"
else
  echo "fault context: ERROR missing explicit non-async-safe provenance qualification" >&2
  exit 1
fi
