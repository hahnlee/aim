#!/bin/bash
set -euo pipefail
export LC_ALL=C

dir="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$dir/../.." && pwd)"
fail() { echo "bionic-socket-broker-adapter: $*" >&2; exit 3; }
missing() { echo "bionic-socket-broker-adapter: missing $*" >&2; exit 2; }

git -C "$root" diff --check -- tools/bionic-socket-broker-adapter || fail 'diff check'
[[ ! -e "$dir/target" ]] || fail 'source-local target directory'

ndk="${ANDROID_NDK_ROOT:-${ANDROID_HOME:-$HOME/Library/Android/sdk}/ndk/28.2.13676358}"
tc="$ndk/toolchains/llvm/prebuilt/darwin-x86_64/bin"
android_cc="$tc/aarch64-linux-android35-clang"
readelf="$tc/llvm-readelf"
for input in "$android_cc" "$readelf"; do [[ -x "$input" ]] || missing "$input"; done
tmp="$(mktemp -d "${TMPDIR:-/tmp}/bionic-socket-broker.XXXXXX")"
trap 'find "$tmp" -depth -delete' EXIT

fixture="$tmp/libnetwork_stack_acceptance.so"
"$android_cc" -std=c17 -O2 -fno-builtin -fPIC -fno-stack-protector \
  -U_FORTIFY_SOURCE -D_FORTIFY_SOURCE=0 -Wall -Wextra -Werror -shared -nostdlib \
  -fuse-ld=lld -Wl,--build-id=none -Wl,--hash-style=sysv -Wl,-z,now \
  -Wl,-z,norelro -Wl,-z,max-page-size=16384 \
  -Wl,-soname,libnetwork_stack_acceptance.so \
  -Wl,--version-script,"$dir/probes/exports.map" \
  "$root/tools/bionic-network-stack-acceptance/probes/fixture.c" \
  "$dir/probes/pipe_fixture.c" -lc -o "$fixture"
"$readelf" --dyn-syms --wide "$fixture" |
  awk '$7=="UND"&&$8!=""{name=$8;sub(/@.*/,"",name);print name}' | sort -u \
  > "$tmp/imports"
[[ "$(wc -l < "$tmp/imports" | tr -d ' ')" == 13 ]] || fail 'fixture import count drift'
while IFS= read -r symbol; do
  case "$symbol" in
    __errno|getaddrinfo|freeaddrinfo|socket|connect|send|sendto|recv|close|pipe|poll|read|write) ;;
    *) fail "unexpected fixture import: $symbol" ;;
  esac
done < "$tmp/imports"

loader_target="$tmp/loader-target"
CARGO_TARGET_DIR="$loader_target" cargo build --quiet --release \
  --manifest-path "$root/crates/darwin-art-elf-loader/Cargo.toml"
loader="$loader_target/release/libdarwin_art_elf_loader.a"
CARGO_TARGET_DIR="$loader_target" cargo build --quiet --release \
  --manifest-path "$root/tools/bionic-fdsan-owner/Cargo.toml"
fdsan="$loader_target/release/libbionic_fdsan_owner.a"
[[ -f "$loader" ]] || missing "$loader"
sdk="$(xcrun --sdk macosx --show-sdk-path)"
host_cc="$(xcrun --find clang)"
host_cxx="$(xcrun --find clang++)"
"$host_cxx" -arch arm64 -isysroot "$sdk" -std=c++17 -Wall -Wextra -Werror \
  -I"$dir/src" "$dir/probes/unix_address.cc" -o "$tmp/unix-address"
"$tmp/unix-address"
includes=(-I"$dir/include"
          -I"$dir/src"
          -iquote "$root/compat"
          -I"$root/tools/bionic-central-fd-broker/include"
          -I"$root/tools/bionic-dns-facade/include"
          -I"$root/tools/bionic-errno-tls/include"
          -I"$root/tools/bionic-fs-facade/include"
          -I"$root/tools/bionic-ioctl-facade/include")
build_runner() {
  local sanitizer="$1"
  local output="$2"
  local san=(-fsanitize="$sanitizer" -fno-omit-frame-pointer)
  "$host_cxx" -arch arm64 -isysroot "$sdk" -std=c++20 -O1 -g \
    -Wall -Wextra -Werror -Wpedantic "${san[@]}" "${includes[@]}" \
    -c "$dir/src/adapter.cc" -o "$tmp/adapter-$sanitizer.o"
  "$host_cxx" -arch arm64 -isysroot "$sdk" -std=c++20 -O1 -g \
    -Wall -Wextra -Werror -Wpedantic "${san[@]}" "${includes[@]}" \
    -c "$dir/src/android_scm_exports.cc" -o "$tmp/scm-exports-$sanitizer.o"
  "$host_cxx" -arch arm64 -isysroot "$sdk" -std=c++20 -O1 -g \
    -Wall -Wextra -Werror -Wpedantic "${san[@]}" "${includes[@]}" \
    -c "$dir/src/fd_inheritance.cc" -o "$tmp/fd-inheritance-$sanitizer.o"
  "$host_cxx" -arch arm64 -isysroot "$sdk" -std=c++20 -O1 -g \
    -Wall -Wextra -Werror -Wpedantic "${san[@]}" "${includes[@]}" \
    -c "$dir/src/retained_scm_export.cc" -o "$tmp/retained-scm-export-$sanitizer.o"
  "$host_cxx" -arch arm64 -isysroot "$sdk" -std=c++20 -O1 -g \
    -Wall -Wextra -Werror -Wpedantic "${san[@]}" "${includes[@]}" \
    -c "$dir/src/eventfd_owner.cc" -o "$tmp/eventfd-owner-$sanitizer.o"
  "$host_cxx" -arch arm64 -isysroot "$sdk" -std=c++20 -O1 -g \
    -Wall -Wextra -Werror "${san[@]}" "${includes[@]}" \
    -c "$dir/src/sync_fence_merge.cc" -o "$tmp/merge-$sanitizer.o"
  "$host_cxx" -arch arm64 -isysroot "$sdk" -std=c++20 -O1 -g \
    -Wall -Wextra -Werror "${san[@]}" "${includes[@]}" \
    -c "$dir/src/sync_fence_broker.cc" -o "$tmp/merge-broker-$sanitizer.o"
  "$host_cxx" -arch arm64 -isysroot "$sdk" -std=c++20 -O1 -g \
    -Wall -Wextra -Werror "${san[@]}" "${includes[@]}" \
    -c "$dir/src/fdsan.cc" -o "$tmp/fdsan-$sanitizer.o"
  "$host_cxx" -arch arm64 -isysroot "$sdk" -std=c++20 -O1 -g \
    -Wall -Wextra -Werror "${san[@]}" "${includes[@]}" \
    -c "$dir/src/fdsan_symbols.cc" -o "$tmp/fdsan-symbols-$sanitizer.o"
  "$host_cxx" -arch arm64 -isysroot "$sdk" -std=c++20 -O1 -g \
    -Wall -Wextra -Werror -Wpedantic "${san[@]}" "${includes[@]}" \
    -c "$root/tools/bionic-central-fd-broker/src/fd_broker.cc" \
    -o "$tmp/broker-$sanitizer.o"
  "$host_cxx" -arch arm64 -isysroot "$sdk" -std=c++20 -O1 -g \
    -Wall -Wextra -Werror -Wpedantic "${san[@]}" "${includes[@]}" \
    -c "$root/tools/bionic-dns-facade/src/dns.cc" \
    -o "$tmp/dns-$sanitizer.o"
  "$host_cc" -arch arm64 -isysroot "$sdk" -std=c17 -O1 -g \
    -Wall -Wextra -Werror "${san[@]}" "${includes[@]}" \
    -I"$root/tools/bionic-errno-tls/generated" \
    -c "$root/tools/bionic-errno-tls/src/errno_tls.c" \
    -o "$tmp/errno-$sanitizer.o"
  "$host_cxx" -arch arm64 -isysroot "$sdk" -std=c++20 -O1 -g \
    -Wall -Wextra -Werror -Wpedantic "${san[@]}" "${includes[@]}" \
    -I"$root/crates/darwin-art-elf-loader/include" \
    "$dir/probes/runner.cc" "$tmp/adapter-$sanitizer.o" \
    "$root/tools/tests/binder-retained-provider-fixture.cc" \
    "$root/compat/binder/fd_transport.cc" \
    "$root/compat/binder/retained_export_lease.cc" \
    "$tmp/scm-exports-$sanitizer.o" \
    "$tmp/fd-inheritance-$sanitizer.o" \
    "$tmp/retained-scm-export-$sanitizer.o" \
    "$tmp/eventfd-owner-$sanitizer.o" \
    "$tmp/merge-$sanitizer.o" "$tmp/merge-broker-$sanitizer.o" \
    "$tmp/broker-$sanitizer.o" "$tmp/dns-$sanitizer.o" \
    "$tmp/errno-$sanitizer.o" "$tmp/fdsan-$sanitizer.o" \
    "$tmp/fdsan-symbols-$sanitizer.o" "$fdsan" "$loader" -framework Security -lresolv \
    -o "$output"
}
build_runner address,undefined "$tmp/runner-asan"
ASAN_OPTIONS=detect_leaks=0:halt_on_error=1 UBSAN_OPTIONS=halt_on_error=1 \
  "$tmp/runner-asan" "$fixture"
build_runner thread "$tmp/runner-tsan"
TSAN_OPTIONS=halt_on_error=1 "$tmp/runner-tsan" "$fixture"

rg -q 'darwin_art_fd_broker_socket_operation' "$dir/src/adapter.cc" ||
  fail 'adapter bypassed broker v3 socket lease'
if rg -n 'kTokenMarker|g_slots|F_DUPFD_CLOEXEC' "$dir/src/adapter.cc" >/dev/null; then
  fail 'adapter reintroduced private descriptor namespace'
fi
mkdir -p "$root/_build/bionic-socket-broker-adapter"
cp "$fixture" "$root/_build/bionic-socket-broker-adapter/"
echo 'bionic-socket-broker-adapter: PASS AndroidELF=HTTP+pipe-poll broker=v6 token=central socket+pipe+read+write+poll+close+fcntl-status DNS=async lifecycle=quiescent deactivate-race=100 Internet=no ASan+UBSan+TSan'
