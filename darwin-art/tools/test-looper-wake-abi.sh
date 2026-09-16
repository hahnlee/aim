#!/bin/bash
set -euo pipefail
project_root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$project_root"
cargo build -p darwin-art-looper-transport
out="$project_root/_build/looper-transport"
mkdir -p "$out"
for test in wake readiness; do
xcrun clang++ -std=c++20 -Wall -Wextra -Werror -I"$project_root/compat/looper" \
  "$project_root/tools/tests/looper-$test-abi-test.cc" \
  "$project_root/target/debug/libdarwin_art_looper_transport.a" \
  -framework Security -framework CoreFoundation -liconv -lresolv \
  -o "$out/$test-abi-test"
"$out/$test-abi-test"
done
xcrun clang++ -std=c++20 -Wall -Wextra -Werror -I"$project_root/compat/looper" \
  "$project_root/tools/tests/looper-queue-owner-test.cc" \
  "$project_root/compat/looper/readiness_queue.cc" \
  "$project_root/target/debug/libdarwin_art_looper_transport.a" \
  -framework Security -framework CoreFoundation -liconv -lresolv \
  -o "$out/queue-owner-test"
"$out/queue-owner-test"
