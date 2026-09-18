#!/usr/bin/env bash
set -euo pipefail
root="$(cd "$(dirname "$0")/../.." && pwd)"
log_object="$root/_build/android16-property-client/async_safe_log.o"
[[ -f "$log_object" ]] || {
  echo 'property-client logging test: build the production client first' >&2
  exit 2
}
stage="$(mktemp -d "${TMPDIR:-/tmp}/property-client-logging.XXXXXX")"
trap 'rm -rf -- "$stage"' EXIT
xcrun clang -I"$root/tools/bionic-errno-tls/include" \
  -I"$root/tools/bionic-errno-tls/generated" \
  -c "$root/tools/bionic-errno-tls/src/errno_tls.c" -o "$stage/errno.o"
xcrun clang -I"$root/tools/bionic-strerror-facade/include" \
  -I"$root/tools/bionic-strerror-facade/generated" \
  -c "$root/tools/bionic-strerror-facade/src/strerror.c" -o "$stage/strerror.o"
xcrun clang -I"$root/tools/bionic-errno-tls/include" \
  "$root/tools/android16-property-client/logging_test.c" \
  "$stage/errno.o" "$stage/strerror.o" "$log_object" \
  -Wl,-dead_strip -o "$stage/test"
"$stage/test"
