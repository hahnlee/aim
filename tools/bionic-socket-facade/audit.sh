#!/bin/bash
set -euo pipefail
export LC_ALL=C

dir="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$dir/../.." && pwd)"
# shellcheck disable=SC1091
source "$dir/sources.lock"

fail() { echo "bionic-socket-facade: $*" >&2; exit 3; }
missing() { echo "bionic-socket-facade: missing $*" >&2; exit 2; }
sha() { shasum -a 256 "$1" | awk '{print $1}'; }
check() { [[ "$(sha "$1")" == "$2" ]] || fail "hash: $1"; }
clean() {
  git -C "$root" diff --check -- tools/bionic-socket-facade ||
    fail 'tracked diff check'
  git -C "$root" diff --cached --check -- tools/bionic-socket-facade ||
    fail 'staged diff check'
  while IFS= read -r -d '' file; do
    set +e
    whitespace="$(git -C "$root" diff --no-index --check /dev/null "$file" 2>&1)"
    status=$?
    set -e
    [[ -z "$whitespace" ]] || fail "untracked whitespace: $file: $whitespace"
    [[ $status -le 1 ]] || fail "could not diff-check: $file"
  done < <(git -C "$root" ls-files --others --exclude-standard -z -- \
    tools/bionic-socket-facade)
  [[ ! -e "$dir/target" ]] || fail 'source-local target directory'
}

clean
tmp="$(mktemp -d "${TMPDIR:-/tmp}/bionic-socket.XXXXXX")"
trap 'find "$tmp" -depth -delete' EXIT

check "$dir/README.md" "$README_SHA256"
check "$dir/audit.sh" "$AUDIT_SHA256"
check "$dir/include/aim_bionic_socket.h" "$HEADER_SHA256"
check "$dir/src/socket.cc" "$PROVIDER_SHA256"
check "$dir/probes/fixture.c" "$FIXTURE_SHA256"
check "$dir/probes/elf_runner.cc" "$RUNNER_SHA256"
check "$dir/probes/exports.map" "$EXPORTS_SHA256"
check "$dir/manifests/imports.tsv" "$IMPORTS_SHA256"
check "$dir/manifests/unsupported.tsv" "$UNSUPPORTED_SHA256"
check "$dir/upstream-sources.tsv" "$UPSTREAM_SOURCES_SHA256"

source_root="$root/_aosp/bionic-socket-facade"
while IFS=$'\t' read -r project revision relative size expected; do
  [[ "$project" != project ]] || continue
  destination="$source_root/$project/$relative"
  if [[ ! -f "$destination" ]]; then
    mkdir -p "$(dirname "$destination")"
    staged="$(mktemp "${TMPDIR:-/tmp}/bionic-socket-source.XXXXXX")"
    curl -fsSL "https://android.googlesource.com/$project/+/$revision/$relative?format=TEXT" |
      base64 -D > "$staged"
    [[ "$(stat -f %z "$staged")" == "$size" &&
       "$(sha "$staged")" == "$expected" ]] ||
      fail "download provenance: $project/$relative"
    mv "$staged" "$destination"
  fi
  [[ "$(stat -f %z "$destination")" == "$size" &&
     "$(sha "$destination")" == "$expected" ]] ||
    fail "source drift: $project/$relative"
done < "$dir/upstream-sources.tsv"

python3 - "$source_root" <<'PY'
from pathlib import Path
import sys
root = Path(sys.argv[1]) / 'platform/bionic'
syscalls = (root / 'libc/SYSCALLS.TXT').read_text()
recv = (root / 'libc/bionic/recv.cpp').read_text()
send = (root / 'libc/bionic/send.cpp').read_text()
poll = (root / 'libc/bionic/poll.cpp').read_text()
recvmsg = (root / 'libc/bionic/recvmsg.cpp').read_text()
for fragment in ('__socket:socket(int, int, int)',
                 '__socketpair:socketpair(int, int, int, int*)',
                 '__accept4:accept4(int, struct sockaddr*, socklen_t*, int)',
                 '__sendto:sendto(int, const void*, size_t, int, const struct sockaddr*, socklen_t)',
                 'recvfrom(int, void*, size_t, unsigned int, struct sockaddr*, socklen_t*)'):
    assert fragment in syscalls, fragment
assert 'return recvfrom(socket, buf, len, flags, nullptr, nullptr);' in recv
assert 'return sendto(socket, buf, len, flags, nullptr, 0);' in send
assert 'return __ppoll(fds, fd_count, ts_ptr, nullptr, 0);' in poll
assert 'return ppoll64(fds, fd_count, ts, set.ptr);' in poll
assert 'for (struct cmsghdr* cmsg = CMSG_FIRSTHDR(msg)' in recvmsg
assert 'if (cmsg->cmsg_type != SCM_RIGHTS)' in recvmsg
print('bionic-socket-facade: AOSP semantics PASS LP64-syscalls+poll+send-recv+SCM_RIGHTS-tracking')
PY

ndk="${ANDROID_NDK_ROOT:-${ANDROID_HOME:-$HOME/Library/Android/sdk}/ndk/$NDK_REVISION}"
tc="$ndk/toolchains/llvm/prebuilt/darwin-x86_64/bin"
android_cc="$tc/aarch64-linux-android${ANDROID_API}-clang"
readelf="$tc/llvm-readelf"
ndk_include="$tc/../sysroot/usr/include"
for input in "$android_cc" "$readelf"; do
  [[ -x "$input" ]] || missing "$input"
done
check "$ndk/source.properties" "$NDK_SOURCE_PROPERTIES_SHA256"
check "$ndk_include/sys/socket.h" "$NDK_SYS_SOCKET_SHA256"
check "$ndk_include/linux/socket.h" "$NDK_LINUX_SOCKET_SHA256"
check "$ndk_include/aarch64-linux-android/asm/socket.h" "$NDK_ASM_SOCKET_SHA256"
check "$ndk_include/netinet/in.h" "$NDK_NETINET_IN_SHA256"
check "$ndk_include/netinet/tcp.h" "$NDK_NETINET_TCP_SHA256"
check "$ndk_include/poll.h" "$NDK_POLL_SHA256"
check "$ndk_include/sys/uio.h" "$NDK_SYS_UIO_SHA256"

cat > "$tmp/android-abi.c" <<'EOF'
#include <netinet/in.h>
#include <poll.h>
#include <stddef.h>
#include <sys/socket.h>
#include <sys/uio.h>
_Static_assert(AF_INET == 2 && AF_INET6 == 10, "Android families");
_Static_assert(SOCK_STREAM == 1 && SOCK_DGRAM == 2, "Android types");
_Static_assert(SOCK_NONBLOCK == 00004000 && SOCK_CLOEXEC == 02000000,
               "Android socket flags");
_Static_assert(SOL_SOCKET == 1, "Android SOL_SOCKET");
_Static_assert(SO_REUSEADDR == 2 && SO_TYPE == 3 && SO_ERROR == 4 &&
               SO_SNDBUF == 7 && SO_RCVBUF == 8 && SO_KEEPALIVE == 9,
               "Android socket options");
_Static_assert(MSG_DONTWAIT == 0x40 && MSG_NOSIGNAL == 0x4000,
               "Android message flags");
_Static_assert(POLLIN == 0x001 && POLLOUT == 0x004 && POLLNVAL == 0x020 &&
               POLLWRNORM == 0x100 && POLLWRBAND == 0x200,
               "Android poll flags");
_Static_assert(sizeof(struct sockaddr_in) == 16 &&
               sizeof(struct sockaddr_in6) == 28 && sizeof(socklen_t) == 4,
               "Android address ABI");
_Static_assert(sizeof(nfds_t) == 4 && sizeof(struct pollfd) == 8,
               "Android poll ABI");
_Static_assert(sizeof(struct iovec) == 16 && sizeof(struct msghdr) == 56 &&
               offsetof(struct msghdr, msg_iovlen) == 24 &&
               offsetof(struct msghdr, msg_controllen) == 40 &&
               sizeof(struct cmsghdr) == 16 && sizeof(sigset_t) == 8,
               "Android message and signal ABI");
int main(void) { return 0; }
EOF
"$android_cc" -std=c17 -Wall -Wextra -Werror -c "$tmp/android-abi.c" \
  -o "$tmp/android-abi.o"

sdk="$(xcrun --sdk macosx --show-sdk-path)"
host_cc="$(xcrun --find clang)"
host_cxx="$(xcrun --find clang++)"
cat > "$tmp/darwin-abi.c" <<'EOF'
#include <netinet/in.h>
#include <poll.h>
#include <stddef.h>
#include <sys/socket.h>
#include <sys/uio.h>
_Static_assert(AF_INET6 == 30, "Darwin AF_INET6 differs");
_Static_assert(SOL_SOCKET == 0xffff, "Darwin SOL_SOCKET differs");
_Static_assert(MSG_DONTWAIT == 0x80, "Darwin MSG_DONTWAIT differs");
_Static_assert(POLLWRNORM == POLLOUT && POLLWRBAND == 0x100,
               "Darwin poll write bits differ");
_Static_assert(sizeof(struct sockaddr_in) == 16 &&
               sizeof(struct sockaddr_in6) == 28, "Darwin address sizes");
_Static_assert(sizeof(struct msghdr) == 48 &&
               offsetof(struct msghdr, msg_iovlen) == 24 &&
               offsetof(struct msghdr, msg_controllen) == 40 &&
               sizeof(struct cmsghdr) == 12,
               "Darwin message ABI differs");
int main(void) { return 0; }
EOF
"$host_cc" -arch arm64 -isysroot "$sdk" -std=c17 -Wall -Wextra -Werror \
  -c "$tmp/darwin-abi.c" -o "$tmp/darwin-abi.o"

includes=(-I"$dir/include" -I"$root/tools/bionic-errno-tls/include")
cxxflags=(-arch arm64 -isysroot "$sdk" -std=c++20 -O2 -Wall -Wextra -Werror -Wpedantic)
"$host_cxx" "${cxxflags[@]}" "${includes[@]}" \
  -c "$dir/src/socket.cc" -o "$tmp/socket.o"
if nm -u "$tmp/socket.o" | awk '{print $NF}' |
    grep -E '^_(dlsym|dlopen|NSLookupSymbolInImage)$' >/dev/null; then
  fail 'dynamic fallback escaped'
fi
nm -u "$tmp/socket.o" | grep -F '_aim_bionic_errno_store' >/dev/null ||
  fail 'Bionic errno route missing'
for symbol in socket socketpair close bind connect listen accept4 shutdown \
  getsockname getpeername getsockopt setsockopt send recv sendto recvfrom; do
  nm -gU "$tmp/socket.o" |
    grep -F " _aim_bionic_socket_$symbol" >/dev/null ||
    fail "definition: $symbol"
done
for symbol in poll ppoll sendmsg recvmsg; do
  nm -gU "$tmp/socket.o" |
    grep -F " _aim_bionic_socket_$symbol" >/dev/null ||
    fail "definition: $symbol"
done
ar rcs "$tmp/libaim-bionic-socket.a" "$tmp/socket.o"

fixture="$tmp/libbionic_socket_fixture.so"
"$android_cc" -std=c17 -O2 -fno-builtin -fPIC -fno-stack-protector \
  -U_FORTIFY_SOURCE -D_FORTIFY_SOURCE=0 -Wall -Wextra -Werror -shared -nostdlib \
  -fuse-ld=lld -Wl,--build-id=none -Wl,--hash-style=sysv -Wl,-z,now \
  -Wl,-z,norelro -Wl,-z,max-page-size=16384 \
  -Wl,-soname,libbionic_socket_fixture.so \
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

loader_target="$tmp/loader-target"
CARGO_TARGET_DIR="$loader_target" cargo build --quiet --release \
  --manifest-path "$root/crates/aim-elf-loader/Cargo.toml"
build_runner() {
  local sanitizer="$1"
  local output="$2"
  local san=(-fsanitize="$sanitizer" -fno-omit-frame-pointer)
  "$host_cxx" -arch arm64 -isysroot "$sdk" -std=c++20 -O1 -g \
    -Wall -Wextra -Werror -Wpedantic "${san[@]}" "${includes[@]}" \
    -c "$dir/src/socket.cc" -o "$tmp/socket-$sanitizer.o"
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
    "$tmp/errno-$sanitizer.o" \
    "$loader_target/release/libaim_elf_loader.a" \
    -framework Security -o "$output"
}
build_runner address,undefined "$tmp/runner-asan"
ASAN_OPTIONS=detect_leaks=0:halt_on_error=1 UBSAN_OPTIONS=halt_on_error=1 \
  "$tmp/runner-asan" "$fixture"
build_runner thread "$tmp/runner-tsan"
TSAN_OPTIONS=halt_on_error=1 "$tmp/runner-tsan" "$fixture"

mkdir -p "$root/_build/bionic-socket-facade"
cp "$tmp/libaim-bionic-socket.a" "$root/_build/bionic-socket-facade/"
cp "$fixture" "$root/_build/bionic-socket-facade/"
clean
echo 'bionic-socket-facade: PASS imports=20 Android-ELF=yes TCP+UDP4+UDP6+scatter-gather+poll+ppoll+socketpair=yes SCM_RIGHTS=closed fd=virtual+generation+borrow-close-safe constants+sockaddr+msghdr+errno=translated ASan+UBSan+TSan'
