#!/bin/bash
set -euo pipefail
export LC_ALL=C

project_root="$(cd "$(dirname "$0")/.." && pwd)"
stage="$(mktemp -d "${TMPDIR:-/tmp}/android-jni-trampoline.XXXXXX")"
trap 'rm -rf "$stage"' EXIT

xcrun clang++ -std=c++20 -O1 -g -Wall -Wextra -Werror \
  -fsanitize=address,undefined -fno-omit-frame-pointer \
  -I"$project_root/compat" \
  "$project_root/compat/darwin_android_jni_trampoline.cc" \
  "$project_root/tools/android-jni-trampoline-smoke.cc" \
  -o "$stage/android-jni-trampoline-smoke"

output="$("$stage/android-jni-trampoline-smoke")"
grep -F 'android-jni-trampoline: PASS' <<< "$output" >/dev/null
printf '%s\n' "$output"

# Execute distinct critical/lifecycle call shapes, not only inspect generated
# entry addresses. Each binary retains the same sanitizer instrumentation.
xcrun clang++ -std=c++20 -O1 -g -Wall -Wextra -Werror \
  -fsanitize=address,undefined -fno-omit-frame-pointer \
  -I"$project_root/compat" \
  "$project_root/compat/darwin_android_jni_trampoline.cc" \
  "$project_root/tools/android-jni-call-shape-smoke.cc" \
  -o "$stage/android-jni-call-shape-smoke"
output="$("$stage/android-jni-call-shape-smoke")"
grep -F 'android-jni-call-shape: PASS' <<< "$output" >/dev/null
printf '%s\n' "$output"

xcrun clang++ -std=c++20 -O1 -g -Wall -Wextra -Werror \
  -fsanitize=address,undefined -fno-omit-frame-pointer \
  -I"$project_root/compat" \
  -I"$project_root/_build/nativehelper-foundation/source/libnativehelper/include_jni" \
  "$project_root/compat/jni/android_varargs.cc" \
  "$project_root/tools/android-jni-varargs-test.cc" \
  -o "$stage/android-jni-varargs-test"
"$stage/android-jni-varargs-test"

xcrun clang++ -std=c++20 -O1 -g -Wall -Wextra -Werror \
  -fsanitize=address,undefined -fno-omit-frame-pointer \
  -I"$project_root/compat" \
  -I"$project_root/_build/nativehelper-foundation/source/libnativehelper/include_jni" \
  "$project_root/compat/jni/method_call.cc" \
  "$project_root/tools/android-jni-method-call-test.cc" \
  -o "$stage/android-jni-method-call-test"
"$stage/android-jni-method-call-test"

xcrun clang++ -std=c++20 -O1 -g -Wall -Wextra -Werror \
  -fsanitize=address,undefined -fno-omit-frame-pointer \
  -I"$project_root/compat" -I"$project_root/tools/android-jni-proxy/include" \
  -I"$project_root/_build/nativehelper-foundation/source/libnativehelper/include_jni" \
  "$project_root/compat/jni/vm_context.cc" \
  "$project_root/compat/jni/android_varargs.cc" \
  "$project_root/compat/jni/method_call.cc" \
  "$project_root/tools/android-jni-vm-context-test.cc" \
  -o "$stage/android-jni-vm-context-test"
"$stage/android-jni-vm-context-test"

xcrun clang++ -std=c++20 -O1 -g -Wall -Wextra -Werror \
  -fsanitize=address,undefined -fno-omit-frame-pointer \
  -I"$project_root/compat" -I"$project_root/tools/android-jni-proxy/include" \
  -I"$project_root/_build/nativehelper-foundation/source/libnativehelper/include_jni" \
  "$project_root/compat/jni/art_registration.cc" \
  "$project_root/tools/android-jni-art-registration-test.cc" \
  -o "$stage/android-jni-art-registration-test"
"$stage/android-jni-art-registration-test"
