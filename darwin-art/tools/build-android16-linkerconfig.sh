#!/bin/bash
# Build the original host generator. Never substitutes recovery configuration
# or an empty APEX inventory for a normal installed system.
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
bash "$root/tools/build-android16-linkerconfig-proto.sh"
bash "$root/tools/build-android16-linkerconfig-apex.sh"
bash "$root/tools/build-android16-apex-info-xml.sh"
src="$root/_aosp/android16-linkerconfig"
out="$root/_build/linkerconfig-host"
mkdir -p "$out/objects"
protobuf=/opt/homebrew/opt/protobuf
abseil=/opt/homebrew/opt/abseil
icu=/opt/homebrew/opt/icu4c
flags=(-std=gnu++20 -arch arm64 -O2 -DFMT_HEADER_ONLY -include unistd.h
  -I"$src/modules/include" -I"$src/contents/include" -I"$src/generator/include"
  -I"$root/_build/linkerconfig-proto" -I"$root/_build/linkerconfig-apex"
  -I"$root/_build/linkerconfig-apex/xml/include"
  -I"$root/_aosp/android16-linkerconfig-apex" -I"$root/_aosp/external/tinyxml2"
  -I"$root/_aosp/system/libbase/include" -I"$root/_aosp/external/fmtlib/include"
  -I"$protobuf/include" -I"$abseil/include" -I"$icu/include")
objects=()
for source_file in "$src/main.cc" "$src/modules/"*.cc "$src/generator/"*.cc \
    "$src/contents/namespace/"*.cc "$src/contents/section/"*.cc \
    "$src/contents/configuration/"*.cc "$src/contents/context/"*.cc \
    "$src/contents/common/"*.cc; do
  [[ "$source_file" != "$src/modules/configparser.cc" ]] || continue
  relative="${source_file#"$src/"}"
  object="$out/objects/${relative//\//_}.o"
  echo "Compile AOSP linkerconfig: $relative"
  xcrun clang++ "${flags[@]}" -c "$source_file" -o "$object"
  objects+=("$object")
done
xcrun clang++ -arch arm64 "${objects[@]}" \
  "$root/_build/linkerconfig-proto/liblinkerconfig-proto-host.a" \
  "$root/_build/linkerconfig-apex/libapexutil-host.a" \
  "$root/_build/linkerconfig-apex/xml/libapex-info-xml-host.a" \
  "$root/_build/libbase-foundation/libandroid-base-darwin.a" \
  "$root/_build/graphics-foundations/liblog-darwin.a" \
  -L"$protobuf/lib" -lprotobuf-lite -L"$abseil/lib" \
  -labsl_log_internal_check_op -labsl_log_internal_message -labsl_raw_logging_internal \
  -L"$icu/lib" -licui18n -licuuc -licudata -Wl,-dead_strip -o "$out/linkerconfig"
"$out/linkerconfig" --help
echo "AOSP linkerconfig host build PASS (installed-root generation not yet verified)"
