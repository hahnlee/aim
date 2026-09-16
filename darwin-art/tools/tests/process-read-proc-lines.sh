#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "$0")/../.." && pwd)"
source "$root/upstream/android16-process-read-proc-lines.lock"
framework="$root/$FRAMEWORK_PROCESS_PATH"
test "$(shasum -a 256 "$framework" | awk '{print $1}')" = "$FRAMEWORK_PROCESS_SHA256"
process_class_path="$(printf '%s' "$PROCESS_CLASS_DESCRIPTOR" | tr '.' '/')"
grep -Fq "$process_class_path" "$root/compat/darwin_framework_natives.cc"
grep -Fq "${READ_PROC_LINES_JNI_SIGNATURE}" "$root/compat/darwin_framework_natives.cc"
grep -Fq "${READ_PROC_FILE_JNI_SIGNATURE}" "$root/compat/darwin_framework_natives.cc"
grep -Fq "${PARSE_PROC_LINE_JNI_SIGNATURE}" "$root/compat/darwin_framework_natives.cc"
apkanalyzer="${APKANALYZER:-$HOME/Library/Android/sdk/cmdline-tools/latest/bin/apkanalyzer}"
test -x "$apkanalyzer"
method_dump="$root/_build/process-read-proc-lines-method.smali"
mkdir -p "$(dirname "$method_dump")"
"$apkanalyzer" dex code --class "$PROCESS_CLASS_DESCRIPTOR" \
  --method "readProcLines${READ_PROC_LINES_JNI_SIGNATURE}" "$framework" > "$method_dump"
grep -Fq ".method public static final native greylist readProcLines${READ_PROC_LINES_JNI_SIGNATURE}" \
  "$method_dump"
"$apkanalyzer" dex code --class "$PROCESS_CLASS_DESCRIPTOR" \
  --method "readProcFile${READ_PROC_FILE_JNI_SIGNATURE}" "$framework" > "$method_dump"
grep -Fq ".method public static final native greylist readProcFile${READ_PROC_FILE_JNI_SIGNATURE}" \
  "$method_dump"
"$apkanalyzer" dex code --class "$PROCESS_CLASS_DESCRIPTOR" \
  --method "parseProcLine${PARSE_PROC_LINE_JNI_SIGNATURE}" "$framework" > "$method_dump"
grep -Fq ".method public static final native greylist parseProcLine${PARSE_PROC_LINE_JNI_SIGNATURE}" \
  "$method_dump"

stage="$(mktemp -d "${TMPDIR:-/tmp}/process-read-proc-lines.XXXXXX")"
trap 'rm -rf -- "$stage"' EXIT
xcrun clang++ -std=c++20 -Wall -Wextra -Werror -fsanitize=address,undefined \
  -I"$root" -I"$root/_aosp/libnativehelper/include_jni" \
  -I"$root/tools/bionic-fs-facade/include" \
  -I"$root/tools/bionic-ioctl-facade/include" \
  "$root/tools/tests/process-read-proc-lines-test.cc" \
  "$root/compat/process/procfs_jni.cc" -o "$stage/test"
"$stage/test"
xcrun clang++ -std=c++20 -Wall -Wextra -Werror -fsanitize=address,undefined \
  -I"$root" -I"$root/_aosp/libnativehelper/include_jni" \
  -I"$root/tools/bionic-fs-facade/include" \
  -I"$root/tools/bionic-ioctl-facade/include" \
  "$root/tools/tests/process-read-proc-file-test.cc" \
  "$root/compat/process/procfs_jni.cc" -o "$stage/read-proc-file-test"
"$stage/read-proc-file-test"
echo "process-procfs-jni: AOSP signatures/provider boundary PASS"
