#!/bin/bash
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
bash "$root/tools/sync-android16-linkerconfig.sh"
out="$root/_build/linkerconfig-golden"
mkdir -p "$out"
protobuf=/opt/homebrew/opt/protobuf
abseil=/opt/homebrew/opt/abseil
for spec in android16-linkerconfig-proto/linker_config.proto android16-linkerconfig-apex/apex_manifest.proto; do
  "$protobuf/bin/protoc" -I"$root/_aosp/${spec%/*}" --cpp_out="$out" "$root/_aosp/$spec"
done
xcrun clang++ -std=c++20 -arch arm64 -O2 -I"$out" -I"$protobuf/include" -I"$abseil/include" \
  "$out/linker_config.pb.cc" "$out/apex_manifest.pb.cc" \
  "$root/tools/native-loader-policy/fixture_json_to_proto.cc" \
  -L"$protobuf/lib" -lprotobuf -L"$abseil/lib" \
  -labsl_log_internal_check_op -labsl_log_internal_message -labsl_raw_logging_internal \
  -labsl_status -labsl_strings -o "$out/fixture-converter"
python3 "$root/tools/native-loader-policy/generator_golden_test.py"
