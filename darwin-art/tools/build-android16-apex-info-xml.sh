#!/bin/bash
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
bash "$root/tools/build-android16-xsdc.sh"
schema="$root/_aosp/android16-linkerconfig-apex/ApexInfoList.xsd"
[[ "$(shasum -a 256 "$schema" | awk '{print $1}')" == d02e30bc40b826186800d12ba2345171c97573417751c87bc87e2b868a8883fc ]]
tinyxml="$root/_aosp/external/tinyxml2"
[[ "$(shasum -a 256 "$tinyxml/tinyxml2.cpp" | awk '{print $1}')" == d5a7fd2ad255d716c4d2a64d5686f90c7e27a8285c49c59a5d105cb400bdf703 ]]
[[ "$(shasum -a 256 "$tinyxml/tinyxml2.h" | awk '{print $1}')" == 02f2389feb67fbb3efe42aaa6942034bc7d54695f357c5aac3cbd2f78a06ca4a ]]
out="$root/_build/linkerconfig-apex/xml"
mkdir -p "$out"
/opt/homebrew/opt/openjdk@17/bin/java \
  -cp "$root/_build/xsdc/xsdc.jar:$root/_build/xsdc/commons-cli-1.2.jar" \
  com.android.xsdc.Main -c -w -t -p com.android.apex -r apex-info-list -o "$out" "$schema"
flags=(-std=c++20 -arch arm64 -O2 -I"$out/include" -I"$tinyxml")
for unit in com_android_apex com_android_apex_enums; do
  xcrun clang++ "${flags[@]}" -c "$out/$unit.cpp" -o "$out/$unit.o"
done
xcrun clang++ "${flags[@]}" -c "$tinyxml/tinyxml2.cpp" -o "$out/tinyxml2.o"
xcrun libtool -static -o "$out/libapex-info-xml-host.a" \
  "$out/com_android_apex.o" "$out/com_android_apex_enums.o" "$out/tinyxml2.o"
xcrun clang++ "${flags[@]}" "$root/tools/native-loader-policy/apex_xml_test.cc" \
  "$out/libapex-info-xml-host.a" -o "$out/apex-xml-test"
"$out/apex-xml-test"
