#!/bin/bash
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
cargo build --quiet --release --manifest-path "$root/Cargo.toml" -p bionic-fdsan-owner
out="$root/_build/fdsan-close-contract"
mkdir -p "$out"
xcrun clang++ -std=c++20 -arch arm64 -Wall -Wextra -Werror \
  -I"$root/tools/bionic-socket-broker-adapter/include" \
  -I"$root/tools/bionic-errno-tls/include" \
  "$root/tools/bionic-socket-broker-adapter/src/fdsan.cc" \
  "$root/tools/bionic-socket-broker-adapter/src/fdsan_property.cc" \
  "$root/tools/bionic-socket-broker-adapter/probes/fdsan_close_contract.cc" \
  "$root/target/release/libbionic_fdsan_owner.a" -o "$out/test"
"$out/test"
