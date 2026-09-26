#!/usr/bin/env bash
set -euo pipefail
export LC_ALL=C

root="$(cd "$(dirname "$0")/.." && pwd)"
mode="all"
case "${1:-}" in
  "") ;;
  --build-only) mode="build-only" ;;
  --test-only)
    exec bash "$root/tools/tests/bionic-runtime-provider-closure-acceptance.sh"
    ;;
  --help|-h)
    echo "usage: $0 [--build-only|--test-only]"
    echo "  (default) build and audit provider archives, then run acceptance"
    echo "  --build-only build and audit production archives only"
    echo "  --test-only run acceptance against existing production archives"
    exit 0
    ;;
  *)
    echo "usage: $0 [--build-only|--test-only]" >&2
    exit 2
    ;;
esac
module="$root/tools/bionic-runtime-provider-closure"
build="$root/_build/bionic-runtime-provider-closure"
objects="$build/objects"
cargo_target="$build/cargo-target"
sdk="$(xcrun --sdk macosx --show-sdk-path)"
cc="$(xcrun --find clang)"
cxx="$(xcrun --find clang++)"
ar="$(xcrun --find ar)"

mkdir -p "$objects"
CARGO_TARGET_DIR="$cargo_target" cargo build --quiet --release \
  --manifest-path "$module/Cargo.toml"
cp "$cargo_target/release/libbionic_runtime_provider_closure.a" \
  "$build/libdarwin-art-bionic-rust-providers.a"
rust="$build/libdarwin-art-bionic-rust-providers.a"
# The production Rust closure must retain only unresolved wide/locale/ICU
# edges. Test-only Cargo integration archives are forbidden here because they
# would duplicate the process-wide ICU provider objects below.
if nm -gU "$rust" 2>/dev/null |
   awk '$2 ~ /^[TDS]$/ {print $3}' |
   grep -E '^(_u_.*_76|__Z16android_icu_initv|_darwin_art_bionic_(fputwc|getwc|ungetwc|wide_stdio_resolve))$' >/dev/null; then
  echo 'bionic-runtime-provider-closure: Rust archive embedded wide/ICU provider definitions' >&2
  exit 2
fi
# The standalone Rust facade builds each embed its own test-local errno object.
# The composed runtime owns errno once through the gdtoa archive, so remove all
# bundled copies before the closure is published.
: >"$build/archive-filter.log"
while errno_member="$("$ar" -t "$rust" | grep -E '^(errno|errno_tls)\.o$' | head -n 1)" &&
      [[ -n "$errno_member" ]]; do
  "$ar" -d "$rust" "$errno_member" 2>>"$build/archive-filter.log"
done
if "$ar" -t "$rust" | grep -E '^(errno|errno_tls)\.o$' >/dev/null; then
  echo 'bionic-runtime-provider-closure: Rust errno member filtering failed' >&2
  exit 2
fi
float_target="$build/float-target"
# Cargo may retain several build-script hash directories. Only its current
# build-script event identifies the archive for this invocation (also when fresh).
float_source="$(CARGO_TARGET_DIR="$float_target" cargo build --quiet --release --lib \
  --message-format=json \
  --manifest-path "$root/tools/bionic-float-conversion-facade/Cargo.toml" |
  python3 -c 'import json, sys
paths = set()
for line in sys.stdin:
    event = json.loads(line)
    if event.get("reason") == "build-script-executed" and "/bionic-float-conversion-facade#" in event.get("package_id", ""):
        paths.add(event["out_dir"] + "/libdarwin_art_bionic_float_conversion.a")
if len(paths) != 1:
    raise SystemExit("expected exactly one current float provider build-script output")
print(next(iter(paths)))')"
[[ -n "$float_source" && -f "$float_source" ]] || {
  echo 'bionic-runtime-provider-closure: float provider archive missing' >&2
  exit 2
}
cp "$float_source" "$build/libdarwin-art-bionic-float-conversion.a"

binary128_target="$build/binary128-target"
binary128_source="$(CARGO_TARGET_DIR="$binary128_target" cargo build --quiet --release --lib \
  --message-format=json \
  --manifest-path "$root/tools/bionic-binary128-conversion-facade/Cargo.toml" |
  python3 -c 'import json, sys
paths = set()
for line in sys.stdin:
    event = json.loads(line)
    if event.get("reason") == "build-script-executed" and "/bionic-binary128-conversion-facade#" in event.get("package_id", ""):
        paths.add(event["out_dir"] + "/libdarwin_art_bionic_binary128_conversion.a")
if len(paths) != 1:
    raise SystemExit("expected exactly one current binary128 provider build-script output")
print(next(iter(paths)))')"
[[ -n "$binary128_source" && -f "$binary128_source" ]] || {
  echo 'bionic-runtime-provider-closure: binary128 provider archive missing' >&2
  exit 2
}
cp "$binary128_source" "$build/libdarwin-art-bionic-binary128-conversion.a"
if nm -gU "$build/libdarwin-art-bionic-float-conversion.a" \
          "$build/libdarwin-art-bionic-binary128-conversion.a" |
   grep -E '_conversion_test_(prepare_host_state|host_state_is_preserved)$' >/dev/null; then
  echo 'bionic-runtime-provider-closure: numeric archive contains test host-state mutation' >&2
  exit 2
fi

cflags=(-arch arm64 -isysroot "$sdk" -O2 -Wall -Wextra -Werror -Wpedantic)
cxxflags=(-arch arm64 -isysroot "$sdk" -std=c++20 -O2 -Wall -Wextra -Werror)

"$cc" "${cflags[@]}" -std=c17 -fno-builtin \
  -I"$root/tools/bionic-libc-leaf-facade/include" \
  -c "$root/tools/bionic-libc-leaf-facade/src/leaf.c" -o "$objects/leaf.o"
"$cc" "${cflags[@]}" -std=c17 \
  -I"$root/tools/bionic-libc-allocator-facade/include" \
  -c "$root/tools/bionic-libc-allocator-facade/src/allocator.c" -o "$objects/allocator.o"
"$cc" "${cflags[@]}" -std=c17 \
  -I"$root/tools/bionic-libc-allocator-facade/include" \
  -c "$root/tools/bionic-libc-allocator-facade/src/allocator_options.c" -o "$objects/allocator_options.o"
"$cc" "${cflags[@]}" -std=c17 \
  -I"$root/tools/bionic-time-facade/include" \
  -c "$root/tools/bionic-time-facade/src/shims.c" -o "$objects/time.o"
"$cxx" "${cxxflags[@]}" \
  -I"$root/tools/android-bionic-pthread-provider/include" \
  -c "$root/tools/android-bionic-pthread-provider/src/provider.cc" -o "$objects/pthread.o"
"$cxx" "${cxxflags[@]}" \
  -I"$root/tools/android-dl-iterate-phdr-provider/include" \
  -c "$root/tools/android-dl-iterate-phdr-provider/src/provider.cc" -o "$objects/phdr.o"
"$cxx" "${cxxflags[@]}" \
  -I"$root/tools/bionic-central-fd-broker/include" \
  -c "$root/tools/bionic-central-fd-broker/src/fd_broker.cc" \
  -o "$objects/central-fd-broker.o"
"$cxx" "${cxxflags[@]}" \
  -I"$root/tools/bionic-socket-broker-adapter/include" \
  -I"$root/tools/bionic-errno-tls/include" \
  -I"$root/tools/bionic-central-fd-broker/include" \
  -I"$root/tools/bionic-dns-facade/include" \
  -I"$root/tools/bionic-fs-facade/include" \
  -I"$root/tools/bionic-ioctl-facade/include" \
  -c "$root/tools/bionic-socket-broker-adapter/src/adapter.cc" \
  -o "$objects/socket-broker-adapter.o"
"$cxx" "${cxxflags[@]}" \
  -c "$root/tools/bionic-socket-broker-adapter/src/android_scm_exports.cc" \
  -o "$objects/android-scm-exports.o"
"$cxx" "${cxxflags[@]}" \
  -c "$root/tools/bionic-socket-broker-adapter/src/fd_inheritance.cc" \
  -o "$objects/fd-inheritance.o"
"$cxx" "${cxxflags[@]}" \
  -c "$root/tools/bionic-socket-broker-adapter/src/ancillary_intake.cc" \
  -o "$objects/scm-ancillary-intake.o"
"$cxx" "${cxxflags[@]}" \
  -c "$root/tools/bionic-socket-broker-adapter/src/scm_channel.cc" \
  -o "$objects/scm-channel.o"
"$cxx" "${cxxflags[@]}" \
  -I"$root/tools/bionic-central-fd-broker/include" \
  -I"$root/tools/bionic-fs-facade/include" \
  -I"$root/tools/bionic-ioctl-facade/include" \
  -c "$root/tools/bionic-socket-broker-adapter/src/scm_guest_group.cc" \
  -o "$objects/scm-guest-group.o"
"$cxx" "${cxxflags[@]}" \
  -I"$root/tools/bionic-central-fd-broker/include" \
  -I"$root/tools/bionic-fs-facade/include" \
  -I"$root/tools/bionic-ioctl-facade/include" \
  -c "$root/tools/bionic-socket-broker-adapter/src/scm_android_receive.cc" \
  -o "$objects/scm-android-receive.o"
"$cxx" "${cxxflags[@]}" \
  -c "$root/tools/bionic-socket-broker-adapter/src/scm_endpoint_provider.cc" \
  -o "$objects/scm-endpoint-provider.o"
"$cxx" "${cxxflags[@]}" \
  -I"$root/tools/bionic-central-fd-broker/include" \
  -c "$root/tools/bionic-socket-broker-adapter/src/retained_scm_export.cc" \
  -o "$objects/retained-scm-export.o"
"$cxx" "${cxxflags[@]}" \
  -I"$root/tools/bionic-errno-tls/include" \
  -c "$root/tools/bionic-socket-broker-adapter/src/eventfd_owner.cc" \
  -o "$objects/eventfd-owner.o"
"$cxx" "${cxxflags[@]}" \
  -c "$root/tools/bionic-socket-broker-adapter/src/sync_fence_merge.cc" \
  -o "$objects/sync-fence-merge.o"
"$cxx" "${cxxflags[@]}" \
  -I"$root/tools/bionic-central-fd-broker/include" \
  -c "$root/tools/bionic-socket-broker-adapter/src/sync_fence_broker.cc" \
  -o "$objects/sync-fence-broker.o"
"$cxx" "${cxxflags[@]}" \
  -I"$root/tools/bionic-socket-broker-adapter/include" \
  -I"$root/tools/bionic-errno-tls/include" \
  -c "$root/tools/bionic-socket-broker-adapter/src/fdsan.cc" -o "$objects/fdsan.o"
"$cxx" "${cxxflags[@]}" \
  -c "$root/tools/bionic-socket-broker-adapter/src/fdsan_property.cc" -o "$objects/fdsan-property.o"
"$cxx" "${cxxflags[@]}" \
  -c "$root/tools/bionic-socket-broker-adapter/src/fdsan_symbols.cc" -o "$objects/fdsan-symbols.o"
"$cxx" "${cxxflags[@]}" \
  -I"$root/tools/bionic-dns-facade/include" \
  -c "$root/tools/bionic-dns-facade/src/dns.cc" -o "$objects/dns.o"

icu_root="$root/_aosp/external/icu-graphics"
"$cxx" "${cxxflags[@]}" -fno-builtin -fvisibility=hidden -DANDROID \
  -I"$root/tools/bionic-locale-facade/include" \
  -I"$icu_root/android_icu4c/include" \
  -I"$icu_root/icu4c/source/common" \
  -I"$icu_root/libandroidicuinit/include" \
  -c "$root/tools/bionic-locale-facade/src/provider.cc" -o "$objects/locale.o"
"$cxx" "${cxxflags[@]}" -fno-builtin -fvisibility=hidden \
  -I"$root/tools/bionic-wide-stdio-facade/include" \
  -I"$root/tools/bionic-locale-facade/include" \
  -c "$root/tools/bionic-wide-stdio-facade/src/provider.cc" \
  -o "$objects/wide-stdio.o"
"$cc" "${cflags[@]}" -std=c17 -fno-builtin \
  -I"$root/tools/bionic-wide-stdio-facade/include" \
  -c "$root/tools/bionic-wide-stdio-facade/src/shims.c" \
  -o "$objects/wide-stdio-shims.o"
"$cc" "${cflags[@]}" -std=c17 -fno-builtin \
  -I"$root/tools/bionic-numeric-facade/include" \
  -c "$root/tools/bionic-numeric-facade/src/provider.c" -o "$objects/numeric.o"
"$cxx" "${cxxflags[@]}" -fno-builtin -fvisibility=hidden \
  -I"$root/tools/bionic-scanf-facade/include" \
  -I"$root/tools/bionic-stdio-facade/include" \
  -I"$root/tools/bionic-numeric-facade/include" \
  -I"$root/tools/bionic-float-conversion-facade/include" \
  -I"$root/tools/bionic-errno-tls/include" \
  -I"$root/tools/bionic-stdio-facade/include" \
  -c "$root/tools/bionic-scanf-facade/src/scanf.cc" -o "$objects/scanf.o"
"$cc" -arch arm64 -isysroot "$sdk" \
  -c "$root/tools/bionic-scanf-facade/src/aapcs64_entry.S" \
  -o "$objects/scanf-entry.o"
"$cxx" "${cxxflags[@]}" -fno-builtin \
  -I"$root/tools/bionic-swprintf-facade/include" \
  -I"$root/tools/bionic-format-facade/include" \
  -I"$root/tools/bionic-errno-tls/include" \
  -c "$root/tools/bionic-swprintf-facade/src/provider.cc" \
  -o "$objects/swprintf.o"
"$cc" -arch arm64 -isysroot "$sdk" \
  -c "$root/tools/bionic-swprintf-facade/src/aapcs64_entry.S" \
  -o "$objects/swprintf-entry.o"
"$cc" "${cflags[@]}" -std=gnu17 -fno-builtin -fvisibility=hidden \
  -Wno-sign-compare -Wno-extra-semi \
  -I"$root/tools/bionic-float-conversion-facade/include" \
  -I"$root/_aosp/bionic-float-conversion-facade/libc/upstream-openbsd/android/include" \
  -I"$root/_aosp/bionic-float-conversion-facade/libc/upstream-openbsd/lib/libc/gdtoa" \
  -include darwin_art_gdtoa_compat.h \
  -D__gdtoa=darwin_art_aosp_gdtoa \
  -c "$root/_aosp/bionic-swprintf-facade/libc/upstream-openbsd/lib/libc/gdtoa/gdtoa.c" \
  -o "$objects/swprintf-gdtoa.o"
"$cxx" "${cxxflags[@]}" \
  -I"$root/tools/bionic-ioctl-facade/include" \
  -I"$root/tools/bionic-errno-tls/include" \
  -c "$root/tools/bionic-ioctl-facade/src/ioctl.cc" \
  -o "$objects/ioctl.o"
"$cc" -arch arm64 -isysroot "$sdk" \
  -c "$root/tools/bionic-ioctl-facade/src/aapcs64_entry.S" \
  -o "$objects/ioctl-entry.o"
"$cxx" "${cxxflags[@]}" \
  -I"$root/tools/bionic-sendfile-facade/include" \
  -I"$root/tools/bionic-errno-tls/include" \
  -c "$root/tools/bionic-sendfile-facade/src/sendfile.cc" \
  -o "$objects/sendfile.o"
strftime_source="$root/_aosp/bionic-strftime-facade/platform/bionic/libc/tzcode/strftime.c"
[[ -f "$strftime_source" ]] || {
  echo 'bionic-runtime-provider-closure: run tools/bionic-strftime-facade/audit.sh to materialize pinned strftime.c' >&2
  exit 2
}
"$cc" "${cflags[@]}" -std=c17 \
  -I"$root/tools/bionic-strftime-facade/include" \
  -I"$root/tools/bionic-errno-tls/include" \
  -c "$root/tools/bionic-strftime-facade/src/provider.c" \
  -o "$objects/strftime.o"
"$cc" "${cflags[@]}" -std=c17 -U_FORTIFY_SOURCE -D_FORTIFY_SOURCE=0 \
  -DHAVE_STRFTIME_L=1 -D__BIONIC__=1 -D__LP64__=1 \
  -DDEPRECATE_TWO_DIGIT_YEARS=0 \
  -include "$root/tools/bionic-strftime-facade/src/upstream_shim.h" \
  -c "$strftime_source" -o "$objects/strftime-upstream.o"
"$cxx" "${cxxflags[@]}" \
  -I"$root/tools/bionic-format-facade/include" \
  -I"$root/tools/bionic-libc-allocator-facade/include" \
  -I"$root/tools/bionic-errno-tls/include" \
  -c "$root/tools/bionic-format-facade/src/format.cc" -o "$objects/format.o"
"$cc" -arch arm64 -isysroot "$sdk" \
  -c "$root/tools/bionic-format-facade/src/aapcs64_entry.S" -o "$objects/format-entry.o"
"$cxx" "${cxxflags[@]}" -fno-builtin \
  -I"$root/tools/bionic-formatted-stdio-facade/include" \
  -I"$root/tools/bionic-format-facade/include" \
  -I"$root/tools/bionic-stdio-facade/include" \
  -I"$root/tools/bionic-libc-allocator-facade/include" \
  -I"$root/tools/bionic-errno-tls/include" \
  -c "$root/tools/bionic-formatted-stdio-facade/src/provider.cc" \
  -o "$objects/formatted-stdio.o"
"$cc" -arch arm64 -isysroot "$sdk" \
  -c "$root/tools/bionic-formatted-stdio-facade/src/aapcs64_entry.S" \
  -o "$objects/formatted-stdio-entry.o"
"$cxx" "${cxxflags[@]}" \
  -I"$root/tools/bionic-syslog-facade/include" \
  -I"$root/tools/bionic-format-facade/include" \
  -I"$root/tools/bionic-libc-allocator-facade/include" \
  -I"$root/tools/bionic-errno-tls/include" \
  -c "$root/tools/bionic-syslog-facade/src/syslog.cc" -o "$objects/syslog.o"
"$cc" -arch arm64 -isysroot "$sdk" \
  -c "$root/tools/bionic-syslog-facade/src/aapcs64_entry.S" -o "$objects/syslog-entry.o"
"$cxx" "${cxxflags[@]}" \
  -I"$root/tools/bionic-syscall-facade/include" \
  -I"$root/tools/bionic-errno-tls/include" \
  -c "$root/tools/bionic-syscall-facade/src/syscall.cc" -o "$objects/syscall.o"
"$cc" -arch arm64 -isysroot "$sdk" \
  -c "$root/tools/bionic-syscall-facade/src/aapcs64_entry.S" -o "$objects/syscall-entry.o"
"$cc" "${cflags[@]}" -std=c17 -fno-builtin \
  -I"$root/tools/bionic-strerror-facade/include" \
  -I"$root/tools/bionic-strerror-facade/generated" \
  -c "$root/tools/bionic-strerror-facade/src/strerror.c" -o "$objects/strerror.o"
"$cc" "${cflags[@]}" -std=c17 -fno-builtin \
  -I"$root/tools/bionic-wide-integer-facade/include" \
  -c "$root/tools/bionic-wide-integer-facade/src/provider.c" -o "$objects/wide-integer.o"
"$cxx" "${cxxflags[@]}" -fno-builtin -fvisibility=hidden -DANDROID \
  -I"$root/tools/bionic-wide-float-facade/include" \
  -I"$root/tools/bionic-float-conversion-facade/include" \
  -I"$root/tools/bionic-libc-allocator-facade/include" \
  -I"$root/tools/bionic-errno-tls/include" \
  -I"$icu_root/android_icu4c/include" \
  -I"$icu_root/icu4c/source/common" \
  -I"$icu_root/libandroidicuinit/include" \
  -c "$root/tools/bionic-wide-float-facade/src/provider.cc" \
  -o "$objects/wide-float.o"
"$cc" "${cflags[@]}" -std=c17 -fno-builtin \
  -I"$root/tools/bionic-abort-facade/include" \
  -c "$root/tools/bionic-abort-facade/src/provider.c" -o "$objects/abort.o"
"$cxx" "${cxxflags[@]}" \
  -I"$root/tools/android-liblog-exec-provider/include" \
  -I"$root/tools/bionic-format-facade/include" \
  -I"$root/_aosp/system/logging/liblog/include" \
  -c "$root/tools/android-liblog-exec-provider/liblog_provider.cc" -o "$objects/liblog.o"
"$cxx" "${cxxflags[@]}" \
  -I"$root/_aosp/system/logging/liblog/include" \
  -c "$root/tools/android-liblog-exec-provider/assert_abi.cc" -o "$objects/liblog-assert.o"
"$cc" "${cflags[@]}" \
  -c "$root/tools/android-liblog-exec-provider/aapcs64_log.S" \
  -o "$objects/liblog-aapcs64.o"
"$cxx" "${cxxflags[@]}" \
  -I"$root/tools/bionic-provider-namespace/include" \
  -I"$root/tools/bionic-provider-namespace/generated" \
  -I"$root/tools/bionic-stdio-facade/include" \
  -c "$root/tools/bionic-provider-namespace/src/namespace.cc" -o "$objects/namespace.o"
"$cxx" "${cxxflags[@]}" \
  -I"$root/tools/bionic-provider-namespace/include" \
  -c "$root/tools/bionic-provider-namespace/src/builtin_adapters.cc" \
  -o "$objects/builtin_adapters.o"
"$cxx" "${cxxflags[@]}" \
  -I"$root/tools/bionic-math-facade/include" \
  -c "$root/tools/bionic-math-facade/src/math.cc" -o "$objects/math.o"

native="$build/libdarwin-art-bionic-native-providers.a"
bash "$root/tools/build-android16-ftw.sh"
ftw="$root/_build/android16-ftw"
"$ar" rcs "$native" \
  "$ftw/fts.c.o" "$ftw/ftw.cpp.o" "$ftw/recallocarray.c.o" "$ftw/bindings.o" \
  "$ftw/resolver.o" \
  "$objects/leaf.o" "$objects/allocator.o" "$objects/allocator_options.o" "$objects/time.o" \
  "$objects/pthread.o" "$objects/phdr.o" \
  "$objects/central-fd-broker.o" "$objects/socket-broker-adapter.o" "$objects/android-scm-exports.o" "$objects/fd-inheritance.o" "$objects/scm-endpoint-provider.o" "$objects/scm-ancillary-intake.o" "$objects/scm-channel.o" "$objects/scm-guest-group.o" "$objects/scm-android-receive.o" "$objects/retained-scm-export.o" "$objects/eventfd-owner.o" "$objects/fdsan.o" "$objects/fdsan-property.o" \
  "$objects/sync-fence-merge.o" "$objects/sync-fence-broker.o" \
  "$objects/fdsan-symbols.o" \
  "$objects/dns.o" "$objects/locale.o" \
  "$objects/wide-stdio.o" "$objects/wide-stdio-shims.o" \
  "$objects/numeric.o" "$objects/scanf.o" "$objects/scanf-entry.o" \
  "$objects/swprintf.o" "$objects/swprintf-entry.o" \
  "$objects/swprintf-gdtoa.o" \
  "$objects/ioctl.o" "$objects/ioctl-entry.o" \
  "$objects/sendfile.o" \
  "$objects/strftime.o" "$objects/strftime-upstream.o" \
  "$objects/format.o" "$objects/format-entry.o" \
  "$objects/formatted-stdio.o" "$objects/formatted-stdio-entry.o" \
  "$objects/syslog.o" "$objects/syslog-entry.o" \
  "$objects/syscall.o" "$objects/syscall-entry.o" \
  "$objects/strerror.o" "$objects/wide-integer.o" "$objects/wide-float.o" \
  "$objects/abort.o" \
  "$objects/liblog.o" "$objects/liblog-assert.o" "$objects/liblog-aapcs64.o" "$objects/namespace.o" \
  "$objects/builtin_adapters.o" \
  "$objects/math.o"

float="$build/libdarwin-art-bionic-float-conversion.a"
binary128="$build/libdarwin-art-bionic-binary128-conversion.a"
icu="$root/_build/icu-foundation"
symbols="$build/provider-resolvers.txt"
nm -gU "$native" "$binary128" "$float" "$rust" > "$build/all-symbols.txt" 2>/dev/null
cat > "$symbols" <<'EOF'
_darwin_art_bionic_abort_resolve
_darwin_art_android_binder_ndk_resolve
_darwin_art_android_aaudio_resolve
_darwin_art_bionic_allocator_resolve
_darwin_art_bionic_binary128_conversion_resolve
_darwin_art_bionic_dso_lifecycle_resolve
_darwin_art_bionic_errno_resolve
_darwin_art_bionic_float_conversion_resolve
_darwin_art_bionic_format_resolve
_darwin_art_bionic_formatted_stdio_resolve
_darwin_art_bionic_fs_resolve
_darwin_art_bionic_ftw_resolve
_darwin_art_bionic_libc_leaf_resolve
_darwin_art_bionic_locale_resolve
_darwin_art_bionic_numeric_resolve
_darwin_art_bionic_process_state_resolve
_darwin_art_bionic_pthread_resolve
_darwin_art_bionic_scanf_resolve
_darwin_art_bionic_sendfile_resolve
_darwin_art_bionic_socket_broker_resolve
_darwin_art_bionic_socket_broker_dns_resolve
_darwin_art_bionic_strftime_resolve
_darwin_art_bionic_stdio_resolve
_darwin_art_bionic_strerror_resolve
_darwin_art_bionic_syslog_resolve
_darwin_art_bionic_syscall_resolve
_darwin_art_bionic_time_resolve
_darwin_art_bionic_vm_resolve
_darwin_art_bionic_swprintf_resolve
_darwin_art_bionic_ioctl_resolve
_darwin_art_bionic_wide_float_resolve
_darwin_art_bionic_wide_integer_resolve
_darwin_art_bionic_wide_stdio_resolve
_darwin_art_dl_phdr_resolve
_darwin_art_liblog_provider_resolve
EOF
while IFS= read -r symbol; do
  grep -E " [TDS] ${symbol}$" "$build/all-symbols.txt" >/dev/null || {
    echo "bionic-runtime-provider-closure: missing resolver $symbol" >&2
    exit 2
  }
done < "$symbols"
nm -gU "$native" "$binary128" "$float" "$rust" 2>/dev/null |
  awk '$2 ~ /^[TDS]$/ && $3 ~ /^_darwin_art_/ {print $3}' |
  sort | uniq -d > "$build/duplicate-provider-definitions.txt"
[[ ! -s "$build/duplicate-provider-definitions.txt" ]] || {
  cat "$build/duplicate-provider-definitions.txt" >&2
  echo 'bionic-runtime-provider-closure: duplicate provider definitions' >&2
  exit 2
}
nm -gU "$native" "$binary128" "$float" "$rust" \
  "$icu/libandroidicuinit-darwin.a" "$icu/libicuuc-common-darwin.a" \
  "$icu/libicuuc-stubdata-darwin.a" \
  "$root/_build/graphics-foundations/liblog-darwin.a" \
  > "$build/ownership-symbols.txt" 2>/dev/null
awk '$2 ~ /^[TDS]$/ && ($3 ~ /^_u_.*_76$/ || $3 == "__Z16android_icu_initv") {print $3}' \
  "$build/ownership-symbols.txt" | sort | uniq -d > "$build/duplicate-icu-definitions.txt"
[[ ! -s "$build/duplicate-icu-definitions.txt" ]] || {
  cat "$build/duplicate-icu-definitions.txt" >&2
  echo 'bionic-runtime-provider-closure: duplicate ICU provider definitions' >&2
  exit 2
}
for symbol in _darwin_art_bionic_malloc_result _darwin_art_bionic_free \
              _darwin_art_bionic_strtod _darwin_art_bionic_strtof \
              _darwin_art_bionic___errno _darwin_art_bionic_errno_store \
              _darwin_art_bionic_format_resolve \
              _darwin_art_bionic_formatted_stdio_resolve \
              _darwin_art_bionic_fprintf _darwin_art_bionic_vfprintf \
              _darwin_art_bionic_fs_process_install \
              _darwin_art_bionic_fs_process_uninstall \
              _darwin_art_bionic_fs_seed_private_directory \
              _darwin_art_bionic_socket_broker_activate \
              _darwin_art_bionic_socket_broker_deactivate \
              _darwin_art_bionic_socket_broker_is_active \
              _darwin_art_bionic_socket_broker_resolve \
              _darwin_art_bionic_socket_broker_dns_resolve \
              _darwin_art_bionic_dns_resolve \
              _darwin_art_bionic_dns_reset_for_test \
              _darwin_art_bionic_stdio_process_install \
              _darwin_art_bionic_stdio_process_uninstall \
              _darwin_art_bionic_stdio_fwrite_core \
              _darwin_art_bionic_stdio_scan_core \
              _darwin_art_bionic_scanf_resolve \
              _darwin_art_bionic_fscanf _darwin_art_bionic_sscanf \
              _darwin_art_bionic_vsscanf \
              _darwin_art_bionic_swprintf_resolve \
              _darwin_art_bionic_swprintf \
              _darwin_art_aosp_gdtoa \
              _darwin_art_bionic_ioctl_resolve \
              _darwin_art_bionic_ioctl_activate \
              _darwin_art_bionic_ioctl_deactivate \
              _darwin_art_bionic_ioctl \
              _darwin_art_bionic_sendfile_resolve \
              _darwin_art_bionic_sendfile_activate \
              _darwin_art_bionic_sendfile_deactivate \
              _darwin_art_bionic_sendfile \
              _darwin_art_bionic_strftime_resolve \
              _darwin_art_bionic_strftime_activate \
              _darwin_art_bionic_strftime_deactivate \
              _darwin_art_bionic_strftime_l \
              _darwin_art_bionic_wide_stdio_resolve \
              _darwin_art_bionic_fputwc _darwin_art_bionic_getwc \
              _darwin_art_bionic_ungetwc \
              _darwin_art_bionic_syslog_resolve \
              _darwin_art_bionic_syscall_resolve _darwin_art_bionic_syscall \
              _darwin_art_liblog_provider_resolve ___android_log_write \
              _darwin_art_bionic_wide_float_resolve __Z16android_icu_initv \
              _u_hasBinaryProperty_76 \
              _darwin_art_bionic_binary128_conversion_resolve \
              _darwin_art_bionic_vm_resolve \
              _darwin_art_bionic_vm_process_install \
              _darwin_art_bionic_vm_process_uninstall \
              _darwin_art_bionic_vm_mmap_core \
              _darwin_art_bionic_strtold _darwin_art_bionic_strtold_l \
              _darwin_art_bionic_wcstold _darwin_art_aosp_strtorQ; do
  [[ "$(grep -Ec " [TDS] ${symbol}$" "$build/ownership-symbols.txt")" == 1 ]] || {
    echo "bionic-runtime-provider-closure: non-unique provider $symbol" >&2
    exit 2
  }
done
echo 'bionic-runtime-provider-closure: ARCHIVE_PASS providers=36 bind_builtins=sealed Rust+C+C++=linked duplicate-provider=0 ICU-owner=1'
if [[ "$mode" == "all" ]]; then
  exec bash "$root/tools/tests/bionic-runtime-provider-closure-acceptance.sh"
fi
echo "bionic-runtime-provider-closure: build-only complete archives=$build"
