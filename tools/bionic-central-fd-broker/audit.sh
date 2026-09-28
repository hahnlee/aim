#!/bin/bash
set -euo pipefail
export LC_ALL=C

dir="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$dir/../.." && pwd)"
fail() { echo "bionic-central-fd-broker: FAIL $*" >&2; exit 2; }
# shellcheck disable=SC1090
source "$dir/sources.lock"
sha() { shasum -a 256 "$1" | awk '{print $1}'; }
header_sha="$(sha "$dir/include/aim_bionic_fd_broker.h")"
[[ "$header_sha" == "$FD_BROKER_HEADER_SHA256" ]] ||
  fail 'public C header drift'

tmp="$(mktemp -d "${TMPDIR:-/tmp}/central-fd-broker.XXXXXX")"
cleanup() {
  [[ "$tmp" == "${TMPDIR:-/tmp}"/central-fd-broker.* ]] && find "$tmp" -depth -delete
}
trap cleanup EXIT

cc="$(xcrun --find clang)"
cxx="$(xcrun --find clang++)"
sdk="$(xcrun --sdk macosx --show-sdk-path)"
"$cc" -std=c17 -arch arm64 -isysroot "$sdk" -Wall -Wextra -Werror \
  -Wpedantic -I"$dir/include" -fsyntax-only "$dir/probes/abi.c"

ndk="${ANDROID_NDK_ROOT:-${ANDROID_HOME:-$HOME/Library/Android/sdk}/ndk/$NDK_REVISION}"
tc="$ndk/toolchains/llvm/prebuilt/darwin-x86_64/bin"
android_cc="$tc/aarch64-linux-android${ANDROID_API}-clang"
readelf="$tc/llvm-readelf"
[[ -x "$android_cc" && -x "$readelf" ]] || fail 'pinned NDK toolchain missing'
[[ "$(sha "$ndk/source.properties")" == "$NDK_SOURCE_PROPERTIES_SHA256" ]] ||
  fail 'NDK source.properties drift'
cat > "$tmp/android-abi.c" <<'EOF'
#define _GNU_SOURCE 1
#include <fcntl.h>
#include <stddef.h>
#include <sys/epoll.h>
#include <sys/socket.h>
_Static_assert(O_CLOEXEC == 02000000, "Android O_CLOEXEC");
_Static_assert(F_DUPFD_CLOEXEC == 1030, "Android F_DUPFD_CLOEXEC");
_Static_assert(EPOLL_CLOEXEC == O_CLOEXEC, "Android EPOLL_CLOEXEC");
_Static_assert(EPOLL_CTL_ADD == 1 && EPOLL_CTL_DEL == 2 && EPOLL_CTL_MOD == 3,
               "Android epoll operations");
_Static_assert(EPOLLIN == 1 && sizeof(struct epoll_event) == 16,
               "Android epoll event ABI");
_Static_assert(sizeof(socklen_t) == 4 && sizeof(struct sockaddr_storage) == 128,
               "Android socket address ABI");
_Static_assert(SOCK_NONBLOCK == 04000 && SOCK_CLOEXEC == O_CLOEXEC,
               "Android socket descriptor flags");
int main(void) { return 0; }
EOF
"$android_cc" -std=c17 -Wall -Wextra -Werror -c "$tmp/android-abi.c" \
  -o "$tmp/android-abi.o"
fixture="$tmp/libcentral_fd_dup_epoll_fixture.so"
"$android_cc" -std=c17 -O2 -fno-builtin -fPIC -fno-stack-protector \
  -U_FORTIFY_SOURCE -D_FORTIFY_SOURCE=0 -Wall -Wextra -Werror -shared \
  -nostdlib -fuse-ld=lld -Wl,--build-id=none -Wl,--hash-style=sysv \
  -Wl,-z,now -Wl,-z,norelro -Wl,-z,max-page-size=16384 \
  -Wl,-soname,libcentral_fd_dup_epoll_fixture.so \
  -Wl,--version-script,"$dir/probes/exports.map" \
  "$dir/probes/android_fixture.c" -lc -o "$fixture"
"$readelf" -h "$fixture" | grep -F 'Machine:                           AArch64' >/dev/null ||
  fail 'fixture is not Android AArch64 ELF'
"$readelf" --dyn-syms --wide "$fixture" |
  awk '$7=="UND"&&$8!=""{name=$8;sub(/@.*/,"",name);print name}' | sort -u \
  > "$tmp/android-imports"
cat > "$tmp/android-imports.expected" <<'EOF'
accept4
bind
close
connect
dup
dup3
epoll_create1
epoll_ctl
epoll_wait
fcntl
getpeername
getsockname
getsockopt
listen
recv
recvfrom
send
sendto
setsockopt
shutdown
EOF
diff -u "$tmp/android-imports.expected" "$tmp/android-imports" ||
  fail 'Android dup/epoll import drift'
"$readelf" --dyn-syms --wide "$fixture" |
  awk '$7=="UND"&&$8!=""&&$8!~ /@LIBC$/{bad=1}END{exit bad}' ||
  fail 'Android fixture symbol-version drift'

common=(-std=c++20 -arch arm64 -isysroot "$sdk" -pthread -Wall -Wextra -Werror
        -Wpedantic -I"$dir/include" "$dir/src/fd_broker.cc"
        "$dir/probes/state_machine_test.cc")
for sanitizer in address undefined thread; do
  binary="$tmp/broker-$sanitizer"
  "$cxx" "${common[@]}" -O1 -g -fno-omit-frame-pointer \
    -fsanitize="$sanitizer" -o "$binary"
  case "$sanitizer" in
    address) ASAN_OPTIONS=detect_leaks=0:halt_on_error=1 "$binary" ;;
    undefined) UBSAN_OPTIONS=halt_on_error=1:print_stacktrace=1 "$binary" ;;
    thread) TSAN_OPTIONS=halt_on_error=1 "$binary" ;;
  esac
done

if nm -u "$tmp/broker-address" | awk '{print $NF}' |
    grep -E '^_(close|dup|fcntl|ioctl|poll|ppoll|read|recv|send|sendfile|socket|write|dlsym|dlopen)$' \
      >/dev/null; then
  fail 'host descriptor or dynamic-loader dependency escaped broker'
fi
if rg -n '(^|[^.>A-Za-z_])(close|dup|fcntl|ioctl|poll|ppoll|read|recv|send|sendfile|socket|write|dlsym|dlopen)[[:space:]]*\(' \
    "$dir/src" >/dev/null; then
  fail 'host descriptor API entered broker source'
fi
xcrun clang-format --dry-run --Werror "$dir/include/aim_bionic_fd_broker.h" \
  "$dir/src/fd_broker.cc" "$dir/probes/state_machine_test.cc" \
  "$dir/probes/abi.c" "$dir/probes/android_fixture.c"
bash -n "$dir/audit.sh"

git -C "$root" diff --check -- tools/bionic-central-fd-broker
git -C "$root" diff --cached --check -- tools/bionic-central-fd-broker
while IFS= read -r -d '' file; do
  set +e
  whitespace="$(git -C "$root" diff --no-index --check /dev/null "$file" 2>&1)"
  status=$?
  set -e
  [[ -z "$whitespace" ]] || fail "untracked whitespace: $file: $whitespace"
  [[ $status -le 1 ]] || fail "could not diff-check untracked file: $file"
done < <(git -C "$root" ls-files --others --exclude-standard -z -- \
         tools/bionic-central-fd-broker)

echo 'bionic-central-fd-broker: PASS C-ABI=v1+v2+v3+v4+v5+v6 AndroidELF=dup+fcntl+epoll+socket-control OFD=shared-offset+host-status refclose=last descriptor-flags=independent poll=owner-batched-wait epoll=interruptible-owner-wait socket=typed-leased+accept-atomic token=generation-tagged ASan+UBSan+TSan host-fd=0'
