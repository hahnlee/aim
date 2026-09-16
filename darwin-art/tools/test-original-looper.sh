#!/bin/bash
set -euo pipefail
project_root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$project_root"
bash tools/build-original-looper.sh
cargo build -p darwin-art-looper-transport
out="$project_root/_build/looper-transport"
xcrun clang++ -std=c++23 -Wall -Wextra -Werror -Wno-unused-parameter \
  -I"$out/source/include" -I"$project_root/compat/looper" \
  -I"$project_root/_aosp/system/core/libutils/include" \
  -I"$project_root/_aosp/system/core/libutils/binder/include" \
  -I"$project_root/_aosp/system/libbase/include" \
  -I"$project_root/_aosp/system/logging/liblog/include" \
  -I"$project_root/_aosp/system/core/libsystem/include" \
  "$project_root/tools/tests/original-looper-test.cc" \
  "$project_root/compat/looper/readiness_queue.cc" "$out/Looper.o" \
  "$project_root/_build/graphics-foundations/libutils-darwin.a" \
  "$project_root/_build/libbase-foundation/libandroid-base-darwin.a" \
  "$project_root/_build/graphics-foundations/liblog-darwin.a" \
  "$project_root/target/debug/libdarwin_art_looper_transport.a" \
  -framework Security -framework CoreFoundation -liconv -lresolv \
  -o "$out/original-looper-test"
"$out/original-looper-test"
