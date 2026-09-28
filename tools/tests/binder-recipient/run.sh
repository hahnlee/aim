#!/bin/bash
# Explicit boot inputs, isolated test image, no installed APK/profile mutation.
set -euo pipefail
[[ $# == 5 ]] || {
  echo 'usage: run.sh core-oj.jar core-libart.jar framework.jar extra-boot-classpath runtime-support.dex' >&2
  exit 64
}
root="$(cd "$(dirname "$0")/../../.." && pwd)"
out="$root/_build/native-fixtures/binder-recipient"
for input in "$1" "$2" "$3" "$5"; do
  [[ "$input" = /* && -f "$input" ]] || { echo "missing absolute boot input: $input" >&2; exit 65; }
done
IFS=: read -r -a extra <<< "$4"
for input in "${extra[@]}"; do
  [[ "$input" = /* && -f "$input" ]] || { echo "missing absolute extra boot input: $input" >&2; exit 65; }
done
[[ -x "$out/driver" && -f "$out/libaim_binder_recipient_test.dylib" \
  && -f "$out/dex/classes.dex" ]] || {
  echo 'run art-bootstrap build-binder-recipient-test first' >&2; exit 66;
}
# A child process owns ART until OS exit. No product dylib is loaded alongside
# the fixture image, and this runner never attempts runtime unload/reuse.
export AIM_BOOT_CLASSPATH="$1:$2:$3:$4"
export AIM_BOOT_CLASSPATH_LOCATIONS="$AIM_BOOT_CLASSPATH"
export AIM_RUNTIME_JAVA_DEBUGGABLE=0
export AIM_JIT=1
exec "$out/driver" "$out/libaim_binder_recipient_test.dylib" \
  "$1" "$2" "$3" "$4" "$5:$out/dex/classes.dex"
