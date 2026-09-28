#!/bin/bash
set -euo pipefail
export LC_ALL=C

dir="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$dir/../.." && pwd)"
# shellcheck disable=SC1091
source "$dir/sources.lock"

fail() { echo "bionic-dns-facade: $*" >&2; exit 3; }
missing() { echo "bionic-dns-facade: missing $*" >&2; exit 2; }
sha() { shasum -a 256 "$1" | awk '{print $1}'; }
check() { [[ "$(sha "$1")" == "$2" ]] || fail "hash: $1"; }
clean() {
  git -C "$root" diff --check -- tools/bionic-dns-facade ||
    fail 'tracked diff check'
  git -C "$root" diff --cached --check -- tools/bionic-dns-facade ||
    fail 'staged diff check'
  while IFS= read -r -d '' file; do
    set +e
    whitespace="$(git -C "$root" diff --no-index --check /dev/null "$file" 2>&1)"
    status=$?
    set -e
    [[ -z "$whitespace" ]] || fail "untracked whitespace: $file: $whitespace"
    [[ $status -le 1 ]] || fail "could not diff-check: $file"
  done < <(git -C "$root" ls-files --others --exclude-standard -z -- \
    tools/bionic-dns-facade)
  [[ ! -e "$dir/target" ]] || fail 'source-local target directory'
}

clean
tmp="$(mktemp -d "${TMPDIR:-/tmp}/bionic-dns.XXXXXX")"
trap 'find "$tmp" -depth -delete' EXIT

check "$dir/README.md" "$README_SHA256"
check "$dir/audit.sh" "$AUDIT_SHA256"
check "$dir/include/aim_bionic_dns.h" "$HEADER_SHA256"
check "$dir/src/dns.cc" "$PROVIDER_SHA256"
check "$dir/probes/fixture.c" "$FIXTURE_SHA256"
check "$dir/probes/elf_runner.cc" "$RUNNER_SHA256"
check "$dir/probes/exports.map" "$EXPORTS_SHA256"
check "$dir/manifests/imports.tsv" "$IMPORTS_SHA256"
check "$dir/manifests/unsupported.tsv" "$UNSUPPORTED_SHA256"
check "$dir/upstream-sources.tsv" "$UPSTREAM_SOURCES_SHA256"

source_root="$root/_aosp/bionic-dns-facade"
while IFS=$'\t' read -r project revision relative size expected; do
  [[ "$project" != project ]] || continue
  destination="$source_root/$project/$relative"
  if [[ ! -f "$destination" ]]; then
    mkdir -p "$(dirname "$destination")"
    staged="$(mktemp "${TMPDIR:-/tmp}/bionic-dns-source.XXXXXX")"
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
gai = (root / 'libc/dns/net/getaddrinfo.c').read_text()
gni = (root / 'libc/dns/net/getnameinfo.c').read_text()
header = (root / 'libc/include/netdb.h').read_text()
for fragment in ('getaddrinfo(const char *hostname, const char *servname,',
                 'freeaddrinfo(struct addrinfo *ai)'):
    assert fragment in gai, fragment
for fragment in ('getnameinfo_inet(const struct sockaddr* sa, socklen_t salen,',
                 'if (flags & NI_NUMERICSERV)',
                 'flags |= NI_NUMERICHOST;'):
    assert fragment in gni, fragment
for fragment in ('struct addrinfo {', '#define\tAI_NUMERICSERV\t0x00000008',
                 '#define\tNI_NUMERICHOST\t0x00000002',
                 '#define\tEAI_OVERFLOW\t14'):
    assert fragment in header, fragment
print('bionic-dns-facade: AOSP semantics PASS addrinfo+numeric-nameinfo+ABI')
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
check "$ndk_include/netdb.h" "$NDK_NETDB_SHA256"
check "$ndk_include/sys/socket.h" "$NDK_SYS_SOCKET_SHA256"
check "$ndk_include/netinet/in.h" "$NDK_NETINET_IN_SHA256"

cat > "$tmp/android-abi.c" <<'EOF'
#include <netdb.h>
#include <netinet/in.h>
#include <stddef.h>
_Static_assert(AF_INET == 2 && AF_INET6 == 10, "Android families");
_Static_assert(AI_PASSIVE == 1 && AI_CANONNAME == 2 && AI_NUMERICHOST == 4 &&
               AI_NUMERICSERV == 8 && AI_ADDRCONFIG == 0x400,
               "Android AI constants");
_Static_assert(NI_NOFQDN == 1 && NI_NUMERICHOST == 2 && NI_NAMEREQD == 4 &&
               NI_NUMERICSERV == 8 && NI_DGRAM == 0x10,
               "Android NI constants");
_Static_assert(EAI_BADFLAGS == 3 && EAI_NONAME == 8 && EAI_SYSTEM == 11 &&
               EAI_OVERFLOW == 14, "Android EAI constants");
_Static_assert(sizeof(struct addrinfo) == 48 &&
               offsetof(struct addrinfo, ai_addrlen) == 16 &&
               offsetof(struct addrinfo, ai_canonname) == 24 &&
               offsetof(struct addrinfo, ai_addr) == 32 &&
               offsetof(struct addrinfo, ai_next) == 40,
               "Android addrinfo ABI");
_Static_assert(sizeof(struct sockaddr_in) == 16 &&
               sizeof(struct sockaddr_in6) == 28,
               "Android sockaddr ABI");
int main(void) { return 0; }
EOF
"$android_cc" -std=c17 -Wall -Wextra -Werror -c "$tmp/android-abi.c" \
  -o "$tmp/android-abi.o"

sdk="$(xcrun --sdk macosx --show-sdk-path)"
host_cc="$(xcrun --find clang)"
host_cxx="$(xcrun --find clang++)"
cat > "$tmp/darwin-abi.c" <<'EOF'
#include <netdb.h>
#include <netinet/in.h>
#include <stddef.h>
_Static_assert(AF_INET6 == 30, "Darwin AF_INET6 differs");
_Static_assert(AI_NUMERICSERV == 0x1000, "Darwin AI_NUMERICSERV differs");
_Static_assert(NI_NUMERICHOST == 2 && NI_NUMERICSERV == 8,
               "Darwin NI numeric values pinned");
_Static_assert(EAI_BADFLAGS == 3 && EAI_NONAME == 8 && EAI_OVERFLOW == 14,
               "Darwin BSD-derived EAI values pinned");
_Static_assert(sizeof(struct addrinfo) == 48 &&
               offsetof(struct addrinfo, ai_canonname) == 24 &&
               offsetof(struct addrinfo, ai_addr) == 32,
               "Darwin top-level layout pinned, still copied");
int main(void) { return 0; }
EOF
"$host_cc" -arch arm64 -isysroot "$sdk" -std=c17 -Wall -Wextra -Werror \
  -c "$tmp/darwin-abi.c" -o "$tmp/darwin-abi.o"

includes=(-I"$dir/include" -I"$root/tools/bionic-errno-tls/include")
cxxflags=(-arch arm64 -isysroot "$sdk" -std=c++20 -O2 -Wall -Wextra -Werror -Wpedantic)
"$host_cxx" "${cxxflags[@]}" "${includes[@]}" \
  -c "$dir/src/dns.cc" -o "$tmp/dns.o"
if nm -u "$tmp/dns.o" | awk '{print $NF}' |
    grep -E '^_(dlsym|dlopen|NSLookupSymbolInImage)$' >/dev/null; then
  fail 'dynamic fallback escaped'
fi
nm -u "$tmp/dns.o" | grep -F '_aim_bionic_errno_store' >/dev/null ||
  fail 'Bionic errno route missing'
for symbol in __get_h_errno getaddrinfo freeaddrinfo gai_strerror getnameinfo inet_ntop \
  gethostbyname getservbyname; do
  definition="aim_bionic_dns_$symbol"
  [[ "$symbol" == "__get_h_errno" ]] && definition=aim_bionic_dns_get_h_errno
  nm -gU "$tmp/dns.o" |
    grep -F " _$definition" >/dev/null ||
    fail "definition: $symbol"
done
ar rcs "$tmp/libaim-bionic-dns.a" "$tmp/dns.o"

fixture="$tmp/libbionic_dns_fixture.so"
"$android_cc" -std=c17 -O2 -fno-builtin -fPIC -fno-stack-protector \
  -U_FORTIFY_SOURCE -D_FORTIFY_SOURCE=0 -Wall -Wextra -Werror -shared -nostdlib \
  -fuse-ld=lld -Wl,--build-id=none -Wl,--hash-style=sysv -Wl,-z,now \
  -Wl,-z,norelro -Wl,-z,max-page-size=16384 \
  -Wl,-soname,libbionic_dns_fixture.so \
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
    -c "$dir/src/dns.cc" -o "$tmp/dns-$sanitizer.o"
  "$host_cc" -arch arm64 -isysroot "$sdk" -std=c17 -O1 -g \
    -Wall -Wextra -Werror "${san[@]}" \
    -I"$root/tools/bionic-errno-tls/include" \
    -I"$root/tools/bionic-errno-tls/generated" \
    -c "$root/tools/bionic-errno-tls/src/errno_tls.c" \
    -o "$tmp/errno-$sanitizer.o"
  "$host_cxx" -arch arm64 -isysroot "$sdk" -std=c++20 -O1 -g \
    -Wall -Wextra -Werror -Wpedantic "${san[@]}" "${includes[@]}" \
    -I"$root/crates/aim-elf-loader/include" \
    "$dir/probes/elf_runner.cc" "$tmp/dns-$sanitizer.o" \
    "$tmp/errno-$sanitizer.o" \
    "$loader_target/release/libaim_elf_loader.a" \
    -framework Security -o "$output"
}
build_runner address,undefined "$tmp/runner-asan"
ASAN_OPTIONS=detect_leaks=0:halt_on_error=1 UBSAN_OPTIONS=halt_on_error=1 \
  "$tmp/runner-asan" "$fixture"
build_runner thread "$tmp/runner-tsan"
TSAN_OPTIONS=halt_on_error=1 "$tmp/runner-tsan" "$fixture"

mkdir -p "$root/_build/bionic-dns-facade"
cp "$tmp/libaim-bionic-dns.a" "$root/_build/bionic-dns-facade/"
cp "$fixture" "$root/_build/bionic-dns-facade/"
clean
echo 'bionic-dns-facade: PASS imports=8 Android-ELF=yes localhost+passive+IPv4+IPv6+numeric-reverse+presentation=yes legacy-hostent+servent=thread-local-deep-copy allocation=deep-copy+retire+quiescent-reset policy=absolute-host-dns host-errno=preserved ASan+UBSan+TSan'
