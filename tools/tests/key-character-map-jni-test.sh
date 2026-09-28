#!/bin/bash
set -euo pipefail

test_root="$(cd "$(dirname "$0")/../.." && pwd)"
jni_include="${JAVA_HOME:-/opt/homebrew/opt/openjdk@17}/include"
[[ -f "$jni_include/jni.h" ]] || exit 2
stage="$(mktemp -d "${TMPDIR:-/tmp}/key-character-map-jni.XXXXXX")"
trap 'rm -rf -- "$stage"' EXIT

clang++ -std=c++20 -O1 -g -Wall -Wextra -Werror \
  -fsanitize=address,undefined -fno-omit-frame-pointer -c \
  -I"$test_root" -I"$jni_include" -I"$jni_include/darwin" \
  "$test_root/runtime/framework/input/key_character_map_jni.cc" \
  -o "$stage/key_character_map_jni.o"
clang++ -std=c++20 -O1 -g -Wall -Wextra -Werror \
  -fsanitize=address,undefined -fno-omit-frame-pointer \
  -I"$test_root" -I"$jni_include" -I"$jni_include/darwin" \
  "$test_root/tools/tests/key-character-map-jni-test.cc" \
  "$stage/key_character_map_jni.o" -o "$stage/key-character-map-jni-test"
"$stage/key-character-map-jni-test"
