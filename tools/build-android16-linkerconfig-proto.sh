#!/bin/bash
# Host build tool only: these Homebrew libraries never enter the guest runtime.
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
source "$root/upstream/android16-linkerconfig-proto.lock"
bash "$root/tools/sync-android16-linkerconfig.sh"
source_dir="$root/_aosp/android16-linkerconfig-proto"
out="$root/_build/linkerconfig-proto"
mkdir -p "$source_dir" "$out"
schema="$source_dir/linker_config.proto"
if [[ ! -f "$schema" ]]; then
  staged="$(mktemp "$source_dir/schema.XXXXXX")"
  trap 'rm -f -- "$staged"' EXIT
  curl -fsSL "https://android.googlesource.com/platform/build/soong/+/$SOONG_REVISION/linkerconfig/proto/linker_config.proto?format=TEXT" |
    base64 -D > "$staged"
  [[ "$(shasum -a 256 "$staged" | awk '{print $1}')" == "$LINKER_PROTO_SHA256" ]]
  mv "$staged" "$schema"
fi
[[ "$(shasum -a 256 "$schema" | awk '{print $1}')" == "$LINKER_PROTO_SHA256" ]]
protobuf=/opt/homebrew/opt/protobuf
abseil=/opt/homebrew/opt/abseil
"$protobuf/bin/protoc" --version
"$protobuf/bin/protoc" -I"$source_dir" --cpp_out="lite:$out" "$schema"
flags=(-std=c++20 -arch arm64 -O2 -DFMT_HEADER_ONLY
  -I"$protobuf/include" -I"$abseil/include" -I"$out"
  -I"$root/_aosp/android16-linkerconfig/modules/include"
  -I"$root/_aosp/system/libbase/include" -I"$root/_aosp/external/fmtlib/include")
xcrun clang++ "${flags[@]}" -c \
  "$root/_aosp/android16-linkerconfig/modules/configparser.cc" -o "$out/configparser.o"
xcrun clang++ "${flags[@]}" -c "$out/linker_config.pb.cc" -o "$out/linker_config.pb.o"
xcrun libtool -static -o "$out/liblinkerconfig-proto-host.a" \
  "$out/configparser.o" "$out/linker_config.pb.o"
xcrun clang++ "${flags[@]}" "$root/tools/native-loader-policy/linker_proto_test.cc" \
  "$out/liblinkerconfig-proto-host.a" \
  "$root/_build/libbase-foundation/libandroid-base-darwin.a" \
  "$root/_build/graphics-foundations/liblog-darwin.a" \
  -L"$protobuf/lib" -lprotobuf-lite -L"$abseil/lib" \
  -labsl_log_internal_check_op -labsl_log_internal_message -labsl_raw_logging_internal \
  -Wl,-dead_strip -o "$out/linker-proto-test"
"$out/linker-proto-test" "$root/_build/android16-linker-config/system/etc/linker.config.pb"
