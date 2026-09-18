#!/bin/bash
set -euo pipefail

script_dir="$(cd "$(dirname "$0")" && pwd)"
project_root="$(cd "$script_dir/.." && pwd)"
if [[ $# == 0 ]]; then
  bash "$script_dir/build-android16-surfaceflinger-core.sh" --archive-only
elif [[ $# != 1 || "$1" != --prepared ]]; then
  echo 'usage: test-android16-surfaceflinger-core.sh [--prepared]' >&2
  exit 2
fi

# shellcheck disable=SC1090
source "$project_root/upstream/android16-surfaceflinger-core.lock"
source_root="$project_root/_aosp/android16-surfaceflinger-core"
frameworks_native="$source_root/frameworks-native"
hardware_interfaces="$source_root/hardware-interfaces"
libhidl="$source_root/system-libhidl"
libfmq="$source_root/system-libfmq"
uapi="$source_root/binder-uapi/libc/kernel"
output_root="$project_root/_build/surfaceflinger-core"
workspace="$output_root/work"
shadow="$workspace/frameworks-native"
generated="$workspace/generated"
nativehelper="$project_root/_aosp/libnativehelper-full"

fail_test() {
  echo "surfaceflinger-core-test: $1" >&2
  exit 3
}

archive="$output_root/libsurfaceflinger-frontend-darwin.a"
binder_archive="$output_root/libbinder-darwin.a"
gui_archive="$output_root/libgui-transaction-darwin.a"
fence_archive="$output_root/libui-fence-darwin.a"
for required in \
  "$archive" "$binder_archive" "$gui_archive" "$fence_archive" \
  "$shadow/services/surfaceflinger/FrontEnd/TransactionHandler.cpp" \
  "$generated/gui/src/android/gui/LayerMetadata.cpp" \
  "$project_root/probes/surfaceflinger_transaction_handler_compile.cc" \
  "$project_root/probes/surfaceflinger_transaction_handler_runtime.cc" \
  "$project_root/tools/tests/parcel-native-handle-test.cc" \
  "$project_root/tools/tests/surfaceflinger-binder-platform-syscalls-fixture.cc" \
  "$project_root/tools/tests/surfaceflinger-binder-process-registry-fixture.cc" \
  "$project_root/compat/surfaceflinger/transaction_bridge.h" \
  "$project_root/compat/binder/rpc_identity.cc" \
  "$project_root/compat/binder/calling_identity.cc" \
  "$project_root/compat/binder/peer_credentials.cc"; do
  [[ -e "$required" ]] || fail_test "missing audit input $required"
done

stage="$(mktemp -d "${TMPDIR:-/tmp}/darwin-art-surfaceflinger-test.XXXXXX")"
trap 'rm -rf "$stage"' EXIT

cxx="$(xcrun --find clang++)"
sdk_root="$(xcrun --sdk macosx --show-sdk-path)"
source "$script_dir/lib/surfaceflinger-compile-flags.sh"

compile_object() {
  local object="$1"
  shift
  "$cxx" "$@" -o "$object"
}

probe_object="$stage/transaction-handler-compile-probe.o"
compile_object "$probe_object" "${flags[@]}" -c \
  "$project_root/probes/surfaceflinger_transaction_handler_compile.cc"
probe_definitions="$(nm -gUC "$probe_object")"
grep -F ' T _darwin_art_surfaceflinger_frontend_has_pending' \
  <<<"$probe_definitions" >/dev/null ||
  fail_test 'missing separately compiled frontend API proof'
archive_definitions="$(nm -gUC "$archive")"
if grep -F '_darwin_art_surfaceflinger_frontend_has_pending' \
    <<<"$archive_definitions" >/dev/null; then
  fail_test 'frontend archive includes test-only API proof'
fi

runtime_probe_object="$stage/transaction-handler-runtime-probe.o"
compile_object "$runtime_probe_object" "${flags[@]}" -c \
  -include "$project_root/compat/surfaceflinger/transaction_bridge.h" \
  "$project_root/probes/surfaceflinger_transaction_handler_runtime.cc"

rpc_identity_objects=()
for rpc_identity_source in \
  compat/binder/rpc_identity.cc \
  compat/binder/calling_identity.cc \
  compat/binder/peer_credentials.cc \
  tools/tests/surfaceflinger-binder-platform-syscalls-fixture.cc \
  tools/tests/surfaceflinger-binder-process-registry-fixture.cc; do
  object="$stage/rpc-$(basename "${rpc_identity_source%.*}").o"
  compile_object "$object" "${flags[@]}" -I"$project_root/compat" \
    -c "$project_root/$rpc_identity_source"
  rpc_identity_objects+=("$object")
done

runtime_probe="$stage/surfaceflinger-transaction-runtime"
"$cxx" -arch arm64 -isysroot "$sdk_root" -Wl,-dead_strip \
  "$runtime_probe_object" \
  -Wl,-force_load,"$archive" \
  -Wl,-force_load,"$gui_archive" \
  -Wl,-force_load,"$binder_archive" \
  -Wl,-force_load,"$fence_archive" \
  "${rpc_identity_objects[@]}" \
  "$project_root/_build/ui-types-foundation/libui-types.a" \
  "$project_root/_build/graphics-foundations/libutils-darwin.a" \
  "$project_root/_build/graphics-foundations/libcutils-darwin.a" \
  "$project_root/_build/graphics-foundations/liblog-darwin.a" \
  "$project_root/_build/libbase-foundation/libandroid-base-darwin.a" \
  -o "$runtime_probe"
"$runtime_probe"
bash "$script_dir/test-release-record-transport.sh"

native_handle_object="$stage/parcel-native-handle-test.o"
compile_object "$native_handle_object" "${flags[@]}" -c \
  "$project_root/tools/tests/parcel-native-handle-test.cc"
native_handle_test="$stage/parcel-native-handle-test"
"$cxx" -arch arm64 -isysroot "$sdk_root" -Wl,-dead_strip \
  "$native_handle_object" -Wl,-force_load,"$binder_archive" \
  "${rpc_identity_objects[@]}" \
  "$project_root/_build/graphics-foundations/libutils-darwin.a" \
  "$project_root/_build/graphics-foundations/libcutils-darwin.a" \
  "$project_root/_build/graphics-foundations/liblog-darwin.a" \
  "$project_root/_build/libbase-foundation/libandroid-base-darwin.a" \
  -o "$native_handle_test"
"$native_handle_test"

mkdir -p "$output_root"
ditto "$runtime_probe" "$output_root/surfaceflinger-transaction-runtime"
echo "surfaceflinger-core-test: compile-proof=PASS runtime=PASS release-record=PASS parcel-native-handle=PASS archives=4"
