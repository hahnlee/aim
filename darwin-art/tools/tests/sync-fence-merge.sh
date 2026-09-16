#!/bin/bash
set -euo pipefail
export LC_ALL=C

dir="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$dir/../.." && pwd)"
adapter="$root/tools/bionic-socket-broker-adapter"
tmp="$(mktemp -d "${TMPDIR:-/tmp}/sync-fence-merge.XXXXXX")"
trap 'find "$tmp" -depth -delete' EXIT

sdk="$(xcrun --sdk macosx --show-sdk-path)"
host_cc="$(xcrun --find clang)"
host_cxx="$(xcrun --find clang++)"
loader="$root/target/release/libdarwin_art_elf_loader.a"
fdsan="$root/target/release/libbionic_fdsan_owner.a"
[[ -f "$loader" ]] || {
  echo "sync-fence-merge: missing existing ELF loader library: $loader" >&2
  exit 2
}
[[ -f "$fdsan" ]] || {
  echo "sync-fence-merge: missing existing fdsan library: $fdsan" >&2
  exit 2
}

includes=(-I"$adapter/include"
          -I"$root/tools/bionic-central-fd-broker/include"
          -I"$root/tools/bionic-dns-facade/include"
          -I"$root/tools/bionic-errno-tls/include")
san=(-fsanitize="${SANITIZER:-address,undefined}" -fno-omit-frame-pointer)
common=(-arch arm64 -isysroot "$sdk" -std=c++20 -O1 -g
        -Wall -Wextra -Werror -Wpedantic "${san[@]}" "${includes[@]}")

"$host_cxx" "${common[@]}" -c "$adapter/src/adapter.cc" \
  -o "$tmp/adapter.o"
"$host_cxx" "${common[@]}" -c "$adapter/src/sync_fence_merge.cc" \
  -o "$tmp/sync-fence-merge.o"
"$host_cxx" "${common[@]}" -c "$adapter/src/sync_fence_broker.cc" \
  -o "$tmp/sync-fence-broker.o"
"$host_cxx" "${common[@]}" -c \
  "$root/tools/bionic-central-fd-broker/src/fd_broker.cc" \
  -o "$tmp/broker.o"
"$host_cxx" "${common[@]}" -c "$root/tools/bionic-dns-facade/src/dns.cc" \
  -o "$tmp/dns.o"
"$host_cxx" "${common[@]}" -c "$root/compat/darwin_android_sync.cc" \
  -o "$tmp/sync.o"
"$host_cxx" "${common[@]}" -c "$adapter/src/fdsan.cc" \
  -o "$tmp/fdsan.o"
"$host_cc" -arch arm64 -isysroot "$sdk" -std=c17 -O1 -g \
  -Wall -Wextra -Werror "${san[@]}" "${includes[@]}" \
  -I"$root/tools/bionic-errno-tls/generated" \
  -c "$root/tools/bionic-errno-tls/src/errno_tls.c" -o "$tmp/errno.o"
"$host_cxx" "${common[@]}" \
  "$adapter/probes/sync_fence_merge_test.cc" \
  "$tmp/adapter.o" "$tmp/sync-fence-merge.o" "$tmp/sync-fence-broker.o" \
  "$tmp/broker.o" "$tmp/dns.o" "$tmp/sync.o" "$tmp/fdsan.o" \
  "$tmp/errno.o" "$fdsan" "$loader" \
  -framework Security -lresolv -o "$tmp/sync-fence-merge"

ASAN_OPTIONS=detect_leaks=0:halt_on_error=1 \
UBSAN_OPTIONS=halt_on_error=1 TSAN_OPTIONS=halt_on_error=1 "$tmp/sync-fence-merge"
