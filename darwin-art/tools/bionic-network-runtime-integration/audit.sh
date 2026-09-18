#!/bin/bash
set -euo pipefail
export LC_ALL=C

dir="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$dir/../.." && pwd)"
# shellcheck disable=SC1091
source "$dir/sources.lock"
fail() { echo "bionic-network-runtime-integration: $*" >&2; exit 3; }
missing() { echo "bionic-network-runtime-integration: missing $*" >&2; exit 2; }
sha() { shasum -a 256 "$1" | awk '{print $1}'; }
check() { [[ "$(sha "$1")" == "$2" ]] || fail "hash: $1"; }

git -C "$root" diff --check -- tools/bionic-network-runtime-integration ||
  fail 'diff check'
[[ ! -e "$dir/target" ]] || fail 'source-local target directory'

check "$dir/README.md" "$README_SHA256"
check "$dir/audit.sh" "$AUDIT_SHA256"
check "$dir/probes/network_jni.c" "$FIXTURE_SHA256"
check "$dir/probes/NetworkRuntimeFixture.java" "$JAVA_SHA256"
check "$dir/probes/exports.map" "$EXPORTS_SHA256"
check "$dir/manifests/routes.tsv" "$ROUTES_SHA256"
check "$dir/manifests/lifecycle.tsv" "$LIFECYCLE_SHA256"
check "$dir/manifests/required-broker-abi.tsv" "$REQUIRED_BROKER_ABI_SHA256"
check "$dir/manifests/shared-scope.tsv" "$SHARED_SCOPE_SHA256"
[[ "$(git -C "$root" rev-parse HEAD:darwin-art/tools/bionic-socket-facade)" == \
  "$SOCKET_TREE" ]] || fail 'socket provider tree drift'
[[ "$(git -C "$root" rev-parse HEAD:darwin-art/tools/bionic-dns-facade)" == \
  "$DNS_TREE" ]] || fail 'DNS provider tree drift'
[[ "$(git -C "$root" rev-parse HEAD:darwin-art/tools/bionic-errno-tls)" == \
  "$ERRNO_TREE" ]] || fail 'errno provider tree drift'
[[ "$(git -C "$root" rev-parse HEAD:darwin-art/tools/bionic-network-stack-acceptance)" == \
  "$NETWORK_ACCEPTANCE_TREE" ]] || fail 'network acceptance tree drift'
[[ "$(git -C "$root" rev-parse HEAD:darwin-art/tools/bionic-central-fd-broker)" == \
  "$BROKER_TREE" ]] || fail 'central broker tree drift'

ndk="${ANDROID_NDK_ROOT:-${ANDROID_HOME:-$HOME/Library/Android/sdk}/ndk/$NDK_REVISION}"
tc="$ndk/toolchains/llvm/prebuilt/darwin-x86_64/bin"
cc="$tc/aarch64-linux-android${ANDROID_API}-clang"
readelf="$tc/llvm-readelf"
for input in "$cc" "$readelf"; do [[ -x "$input" ]] || missing "$input"; done
command -v javac >/dev/null || missing javac
sdk="${ANDROID_SDK_ROOT:-${ANDROID_HOME:-$HOME/Library/Android/sdk}}"
d8="$(find "$sdk/build-tools" -mindepth 2 -maxdepth 2 -name d8 -type f | sort | tail -1)"
[[ -x "$d8" ]] || missing d8
platform="$sdk/platforms/android-$ANDROID_API/android.jar"
[[ -f "$platform" ]] || missing "$platform"

tmp="$(mktemp -d "${TMPDIR:-/tmp}/bionic-network-runtime.XXXXXX")"
trap 'find "$tmp" -depth -delete' EXIT
fixture="$tmp/libdarwin_art_network_runtime.so"
"$cc" -std=c17 -O2 -fno-builtin -fPIC -fno-stack-protector \
  -U_FORTIFY_SOURCE -D_FORTIFY_SOURCE=0 -Wall -Wextra -Werror -shared \
  -nostdlib -fuse-ld=lld -Wl,--build-id=none -Wl,--hash-style=sysv -Wl,-z,now \
  -Wl,-z,norelro -Wl,-z,max-page-size=16384 \
  -Wl,-soname,libdarwin_art_network_runtime.so \
  -Wl,--version-script,"$dir/probes/exports.map" \
  "$dir/probes/network_jni.c" -lc -o "$fixture"

"$readelf" --dyn-syms --wide "$fixture" |
  awk '$7=="UND"&&$8!=""{name=$8;sub(/@.*/,"",name);print name}' | sort -u \
  > "$tmp/imports"
tail -n +2 "$dir/manifests/routes.tsv" | cut -f1 | sort -u > "$tmp/routes"
diff -u "$tmp/routes" "$tmp/imports" || fail 'Android fixture import drift'
"$readelf" --dyn-syms --wide "$fixture" |
  awk '$7=="UND"&&$8!=""&&$8!~ /@LIBC$/{bad=1}END{exit bad}' ||
  fail 'Android fixture version drift'
[[ "$("$readelf" --dyn-syms --wide "$fixture" |
  awk '$7!="UND"&&$5=="GLOBAL"&&$6=="DEFAULT"&&$8!=""{name=$8;sub(/@@.*/,"",name);print name}' |
  sort -u)" == "JNI_OnLoad" ]] || fail 'Android fixture export drift'
"$readelf" -d "$fixture" | grep -F '(SONAME)' | grep -F '[libdarwin_art_network_runtime.so]' >/dev/null ||
  fail 'Android fixture SONAME drift'

mkdir -p "$tmp/classes" "$tmp/dex"
javac --release 8 -encoding UTF-8 -d "$tmp/classes" \
  "$dir/probes/NetworkRuntimeFixture.java"
"$d8" --lib "$platform" --min-api "$ANDROID_API" --output "$tmp/dex" \
  "$tmp/classes/dev/darwinart/probe/NetworkRuntimeFixture.class"
[[ -s "$tmp/dex/classes.dex" ]] || fail 'network runtime DEX missing'
strings "$tmp/dex/classes.dex" | grep -F 'Ldev/darwinart/probe/NetworkRuntimeFixture;' >/dev/null ||
  fail 'network runtime DEX class missing'
strings "$tmp/dex/classes.dex" | grep -F 'nativeLoopbackHttp' >/dev/null ||
  fail 'network runtime DEX native missing'

[[ "$(tail -n +2 "$dir/manifests/routes.tsv" | cut -f1 | sort | uniq -d)" == "" ]] ||
  fail 'duplicate process route'
awk -F '\t' 'NR>1 && ($2!="libc.so" || $3!="LIBC"){bad=1} END{exit bad}' \
  "$dir/manifests/routes.tsv" || fail 'non-exact route'
[[ "$(awk -F '\t' '$1=="close"{print $4}' "$dir/manifests/routes.tsv")" == \
  'central-fd-broker' ]] || fail 'close is not centrally owned'
[[ "$(tail -n +2 "$dir/manifests/lifecycle.tsv" | wc -l | tr -d ' ')" == 12 ]] ||
  fail 'lifecycle phase drift'
awk -F '\t' 'NR>1 {if ($1 != NR-1) bad=1} END{exit bad}' \
  "$dir/manifests/lifecycle.tsv" || fail 'lifecycle order drift'
[[ "$(tail -n +2 "$dir/manifests/required-broker-abi.tsv" | wc -l | tr -d ' ')" == 13 ]] ||
  fail 'required broker ABI drift'
[[ "$(tail -n +2 "$dir/manifests/shared-scope.tsv" | wc -l | tr -d ' ')" == 7 ]] ||
  fail 'shared integration scope drift'

broker="$root/tools/bionic-central-fd-broker/include/darwin_art_bionic_fd_broker.h"
[[ -f "$broker" ]] || missing "$broker"
rg -q 'DARWIN_ART_FD_OWNER_ABI_V3' "$broker" || fail 'broker v3 ABI missing'
rg -q 'darwin_art_fd_broker_socket_operation' "$broker" ||
  fail 'typed socket dispatch missing'
adapter="$root/tools/bionic-socket-broker-adapter/src/adapter.cc"
rg -q 'darwin_art_bionic_socket_broker_dns_resolve' "$adapter" ||
  fail 'DNS lifecycle wrapper missing'
rg -q 'kCentralBrokerTokenMarker' "$adapter" ||
  fail 'central close token classification missing'
rg -q 'DARWIN_ART_ANDROID_NETWORK_FIXTURE' \
  "$root/probes/runtime_fixture_options.cc" ||
  fail 'actual ART network fixture option missing'
rg -q 'nativeLoopbackHttp' "$root/probes/runtime_acceptance_phases.cc" ||
  fail 'actual ART network acceptance phase missing'
rg -q '127\.0\.0\.1' "$dir/probes/network_jni.c" || fail 'loopback target missing'
if rg -n 'localhost|https?://|8\.8\.8\.8|AF_INET6' "$dir/probes/network_jni.c" >/dev/null; then
  fail 'fixture escaped numeric IPv4 loopback scope'
fi

mkdir -p "$root/_build/bionic-network-runtime-integration"
cp "$fixture" "$root/_build/bionic-network-runtime-integration/"
cp "$tmp/dex/classes.dex" "$root/_build/bionic-network-runtime-integration/"
echo 'bionic-network-runtime-integration: PASS Android-ELF=JNI_OnLoad+RegisterNatives DEX=nativeLoopbackHttp imports=8 routes=exact close=central broker=v3 DNS=leased lifecycle=12 Internet=no activation=ART-loopback'
