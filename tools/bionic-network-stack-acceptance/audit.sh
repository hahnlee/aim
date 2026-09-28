#!/bin/bash
set -euo pipefail
export LC_ALL=C

dir="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$dir/../.." && pwd)"
# shellcheck disable=SC1091
source "$dir/sources.lock"

fail() { echo "bionic-network-stack-acceptance: $*" >&2; exit 3; }
missing() { echo "bionic-network-stack-acceptance: missing $*" >&2; exit 2; }
sha() { shasum -a 256 "$1" | awk '{print $1}'; }
check() { [[ "$(sha "$1")" == "$2" ]] || fail "hash: $1"; }
clean() {
  git -C "$root" diff --check -- tools/bionic-network-stack-acceptance ||
    fail 'tracked diff check'
  git -C "$root" diff --cached --check -- tools/bionic-network-stack-acceptance ||
    fail 'staged diff check'
  while IFS= read -r -d '' file; do
    set +e
    whitespace="$(git -C "$root" diff --no-index --check /dev/null "$file" 2>&1)"
    status=$?
    set -e
    [[ -z "$whitespace" ]] || fail "untracked whitespace: $file: $whitespace"
    [[ $status -le 1 ]] || fail "could not diff-check: $file"
  done < <(git -C "$root" ls-files --others --exclude-standard -z -- \
    tools/bionic-network-stack-acceptance)
  [[ ! -e "$dir/target" ]] || fail 'source-local target directory'
}

clean
tmp="$(mktemp -d "${TMPDIR:-/tmp}/bionic-network-stack.XXXXXX")"
trap 'find "$tmp" -depth -delete' EXIT

check "$dir/README.md" "$README_SHA256"
check "$dir/audit.sh" "$AUDIT_SHA256"
check "$dir/probes/fixture.c" "$FIXTURE_SHA256"
check "$dir/probes/elf_runner.cc" "$RUNNER_SHA256"
check "$dir/probes/exports.map" "$EXPORTS_SHA256"
check "$dir/manifests/imports.tsv" "$IMPORTS_SHA256"
check "$dir/manifests/boundary.tsv" "$BOUNDARY_SHA256"

socket_tree="$(git -C "$root" rev-parse HEAD:tools/bionic-socket-facade)"
dns_tree="$(git -C "$root" rev-parse HEAD:tools/bionic-dns-facade)"
errno_tree="$(git -C "$root" rev-parse HEAD:tools/bionic-errno-tls)"
[[ "$socket_tree" == "$SOCKET_TREE" ]] || fail 'socket provider tree drift'
[[ "$dns_tree" == "$DNS_TREE" ]] || fail 'DNS provider tree drift'
[[ "$errno_tree" == "$ERRNO_TREE" ]] || fail 'errno provider tree drift'
check "$root/tools/bionic-socket-facade/src/socket.cc" "$SOCKET_SOURCE_SHA256"
check "$root/tools/bionic-dns-facade/src/dns.cc" "$DNS_SOURCE_SHA256"
check "$root/tools/bionic-errno-tls/src/errno_tls.c" "$ERRNO_SOURCE_SHA256"

ndk="${ANDROID_NDK_ROOT:-${ANDROID_HOME:-$HOME/Library/Android/sdk}/ndk/$NDK_REVISION}"
tc="$ndk/toolchains/llvm/prebuilt/darwin-x86_64/bin"
android_cc="$tc/aarch64-linux-android${ANDROID_API}-clang"
readelf="$tc/llvm-readelf"
for input in "$android_cc" "$readelf"; do
  [[ -x "$input" ]] || missing "$input"
done
check "$ndk/source.properties" "$NDK_SOURCE_PROPERTIES_SHA256"

fixture="$tmp/libnetwork_stack_acceptance.so"
"$android_cc" -std=c17 -O2 -fno-builtin -fPIC -fno-stack-protector \
  -U_FORTIFY_SOURCE -D_FORTIFY_SOURCE=0 -Wall -Wextra -Werror -shared -nostdlib \
  -fuse-ld=lld -Wl,--build-id=none -Wl,--hash-style=sysv -Wl,-z,now \
  -Wl,-z,norelro -Wl,-z,max-page-size=16384 \
  -Wl,-soname,libnetwork_stack_acceptance.so \
  -Wl,--version-script,"$dir/probes/exports.map" \
  "$dir/probes/fixture.c" -lc -o "$fixture"
"$readelf" --dyn-syms --wide "$fixture" |
  awk '$7=="UND"&&$8!=""{name=$8;sub(/@.*/,"",name);print name}' | sort -u \
  > "$tmp/fixture-imports"
tail -n +2 "$dir/manifests/imports.tsv" | cut -f1 | sort -u \
  > "$tmp/expected-imports"
diff -u "$tmp/expected-imports" "$tmp/fixture-imports" ||
  fail 'fixture import drift'
"$readelf" --dyn-syms --wide "$fixture" |
  awk '$7=="UND"&&$8!=""&&$8!~ /@LIBC$/{bad=1}END{exit bad}' ||
  fail 'fixture version drift'
strings "$fixture" | grep -F 'GET /acceptance HTTP/1.0' >/dev/null ||
  fail 'HTTP request missing'
if grep -E 'https?://|example\.(com|org)|8\.8\.8\.8' \
    "$dir/probes/fixture.c" "$dir/probes/elf_runner.cc" >/dev/null; then
  fail 'external network target escaped fixture'
fi

sdk="$(xcrun --sdk macosx --show-sdk-path)"
host_cc="$(xcrun --find clang)"
host_cxx="$(xcrun --find clang++)"
includes=(-I"$root/tools/bionic-socket-facade/include"
          -I"$root/tools/bionic-dns-facade/include"
          -I"$root/tools/bionic-errno-tls/include")
loader_target="$tmp/loader-target"
CARGO_TARGET_DIR="$loader_target" cargo build --quiet --release \
  --manifest-path "$root/crates/aim-elf-loader/Cargo.toml"
build_runner() {
  local sanitizer="$1"
  local output="$2"
  local san=(-fsanitize="$sanitizer" -fno-omit-frame-pointer)
  "$host_cxx" -arch arm64 -isysroot "$sdk" -std=c++20 -O1 -g \
    -Wall -Wextra -Werror -Wpedantic "${san[@]}" "${includes[@]}" \
    -c "$root/tools/bionic-socket-facade/src/socket.cc" \
    -o "$tmp/socket-$sanitizer.o"
  "$host_cxx" -arch arm64 -isysroot "$sdk" -std=c++20 -O1 -g \
    -Wall -Wextra -Werror -Wpedantic "${san[@]}" "${includes[@]}" \
    -c "$root/tools/bionic-dns-facade/src/dns.cc" \
    -o "$tmp/dns-$sanitizer.o"
  "$host_cc" -arch arm64 -isysroot "$sdk" -std=c17 -O1 -g \
    -Wall -Wextra -Werror "${san[@]}" \
    -I"$root/tools/bionic-errno-tls/include" \
    -I"$root/tools/bionic-errno-tls/generated" \
    -c "$root/tools/bionic-errno-tls/src/errno_tls.c" \
    -o "$tmp/errno-$sanitizer.o"
  "$host_cxx" -arch arm64 -isysroot "$sdk" -std=c++20 -O1 -g \
    -Wall -Wextra -Werror -Wpedantic "${san[@]}" "${includes[@]}" \
    -I"$root/crates/aim-elf-loader/include" \
    "$dir/probes/elf_runner.cc" "$tmp/socket-$sanitizer.o" \
    "$tmp/dns-$sanitizer.o" "$tmp/errno-$sanitizer.o" \
    "$loader_target/release/libaim_elf_loader.a" \
    -framework Security -o "$output"
}
build_runner address,undefined "$tmp/runner-asan"
ASAN_OPTIONS=detect_leaks=0:halt_on_error=1 UBSAN_OPTIONS=halt_on_error=1 \
  "$tmp/runner-asan" "$fixture"
build_runner thread "$tmp/runner-tsan"
TSAN_OPTIONS=halt_on_error=1 "$tmp/runner-tsan" "$fixture"

mkdir -p "$root/_build/bionic-network-stack-acceptance"
cp "$fixture" "$root/_build/bionic-network-stack-acceptance/"
clean
echo 'bionic-network-stack-acceptance: PASS imports=9 Android-ELF=yes HTTP/1.0=IPv4+IPv6+localhost concurrent-clients=4 short-IO=3 EINTR=actual DNS-free=yes server-teardown=yes resolver=closed host-errno=preserved Internet=no ASan+UBSan+TSan'
