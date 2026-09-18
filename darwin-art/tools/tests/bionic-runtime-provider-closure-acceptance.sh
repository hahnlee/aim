#!/usr/bin/env bash
set -euo pipefail
export LC_ALL=C

# This is the downstream acceptance gate for the production provider archives.
# Keep every fixture/probe input here: build-bionic-runtime-provider-closure.sh
# must be able to publish an archive without reading any of these paths.
root="$(cd "$(dirname "$0")/../.." && pwd)"
bash "$root/tools/tests/property-client-logging-test.sh"
bash "$root/tools/tests/numeric-provider-production-boundary.sh"
module="$root/tools/bionic-runtime-provider-closure"
build="$root/_build/bionic-runtime-provider-closure"
native="$build/libdarwin-art-bionic-native-providers.a"
float="$build/libdarwin-art-bionic-float-conversion.a"
binary128="$build/libdarwin-art-bionic-binary128-conversion.a"
rust="$build/libdarwin-art-bionic-rust-providers.a"
icu="$root/_build/icu-foundation"
smoke="$build/full-link-smoke"

# These are test-only inputs. Runtime/provider sources belong to the
# production build script and must not be added to this list.
test_sources=(
  "$module/full_link_smoke.cc"
  "$root/tools/android16-ftw/traversal_smoke.cc"
  "$module/property_iteration_smoke.cc"
  "$module/signed_numeric_smoke.cc"
  "$module/mkdirat_smoke.cc"
  "$module/credentials_snapshot_smoke.cc"
  "$root/tools/bionic-socket-broker-adapter/probes/unix_connect.cc"
  "$root/tools/bionic-socket-broker-adapter/probes/fdsan.cc"
  "$root/tools/android16-property-client/client_smoke.cc"
  "$root/tools/bionic-process-state-facade/probes/configured_snapshot.cc"
  "$root/tools/bionic-stdio-facade/probes/fortify_stream.cc"
  "$root/tools/android-liblog-exec-provider/buf_print_call_test.S"
  "$root/tools/android-liblog-exec-provider/assert_call_test.S"
  "$root/tools/android-liblog-exec-provider/assert_call_test.cc"
)

for archive in "$native" "$float" "$binary128" "$rust" \
               "$icu/libandroidicuinit-darwin.a" \
               "$icu/libicuuc-common-darwin.a" \
               "$icu/libicuuc-stubdata-darwin.a" \
               "$root/_build/graphics-foundations/liblog-darwin.a"; do
  [[ -f "$archive" ]] || {
    echo "bionic-runtime-provider-closure: missing production archive $archive; run --build-only first" >&2
    exit 2
  }
done
for source in "${test_sources[@]}"; do
  [[ -f "$source" ]] || {
    echo "bionic-runtime-provider-closure: missing acceptance fixture $source" >&2
    exit 2
  }
done

sdk="$(xcrun --sdk macosx --show-sdk-path)"
cxx="$(xcrun --find clang++)"
cxxflags=(-arch arm64 -isysroot "$sdk" -std=c++20 -O2 -Wall -Wextra -Werror)

"$cxx" "${cxxflags[@]}" \
  -I"$root/tools/bionic-stdio-facade/include" \
  -I"$root/tools/bionic-provider-namespace/include" \
  -I"$root/tools/bionic-provider-namespace/generated" \
  -I"$root/tools/bionic-process-state-facade/include" \
  -I"$root/tools/bionic-fs-facade/include" \
  -I"$root/tools/bionic-ioctl-facade/include" \
  -I"$root/tools/bionic-central-fd-broker/include" \
  -I"$root/tools/bionic-socket-broker-adapter/include" \
  -I"$root/tools/bionic-dns-facade/include" \
  -I"$root/tools/bionic-vm-facade/include" \
  -I"$root/_aosp/system/logging/liblog/include" \
  "${test_sources[@]}" \
  -Wl,-force_load,"$binary128" \
  "$native" "$float" "$rust" \
  -Wl,-force_load,"$icu/libandroidicuinit-darwin.a" \
  "$icu/libicuuc-common-darwin.a" "$icu/libicuuc-stubdata-darwin.a" \
  "$root/_build/graphics-foundations/liblog-darwin.a" \
  -framework Security -lresolv -o "$smoke"

"$smoke"
if otool -L "$smoke" | grep -E '(/opt/homebrew|/usr/local|libicu(uc|i18n))' >/dev/null; then
  echo 'bionic-runtime-provider-closure: host/dynamic ICU escaped' >&2
  exit 2
fi
echo 'bionic-runtime-provider-closure: PASS providers=36 bind_builtins=sealed Rust+C+C++=linked duplicate-provider=0 ICU-owner=1 host-fallback=0'
