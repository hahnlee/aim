#!/bin/bash
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
bash "$root/tools/build-android16-linkerconfig-apex.sh"
bash "$root/tools/build-android16-apex-info-xml.sh"
out="$root/_build/linkerconfig-apex"
protobuf=/opt/homebrew/opt/protobuf
abseil=/opt/homebrew/opt/abseil
xcrun clang++ -std=c++20 -arch arm64 -O2 -DFMT_HEADER_ONLY \
  -I"$protobuf/include" -I"$abseil/include" -I"$out" \
  -I"$root/_aosp/android16-linkerconfig-apex" -I"$out/xml/include" \
  -I"$root/_aosp/external/tinyxml2" -I"$root/_aosp/system/libbase/include" \
  -I"$root/_aosp/external/fmtlib/include" \
  "$root/tools/native-loader-policy/apex_inventory.cc" \
  "$out/libapexutil-host.a" "$out/xml/libapex-info-xml-host.a" \
  "$root/_build/libbase-foundation/libandroid-base-darwin.a" \
  "$root/_build/graphics-foundations/liblog-darwin.a" \
  -L"$protobuf/lib" -lprotobuf-lite -L"$abseil/lib" \
  -labsl_log_internal_check_op -labsl_log_internal_message -labsl_raw_logging_internal \
  -Wl,-dead_strip -o "$out/apex-inventory"
