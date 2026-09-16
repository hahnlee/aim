#!/bin/bash
# Original libapexutil + manifest protobuf for the host linkerconfig generator.
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
revision=9921a73777f97def1d8851dd0a1529f1c45bcc72
src="$root/_aosp/android16-linkerconfig-apex"
out="$root/_build/linkerconfig-apex"
mkdir -p "$src" "$out"
while read -r source_path expected; do
  [[ -n "$source_path" && "$source_path" != \#* ]] || continue
  destination="$src/${source_path##*/}"
  if [[ ! -e "$destination" ]]; then
    staged="$(mktemp "$src/download.XXXXXX")"
    trap 'rm -f -- "$staged"' EXIT
    curl -fsSL "https://android.googlesource.com/platform/system/apex/+/$revision/$source_path?format=TEXT" |
      base64 -D > "$staged"
    [[ "$(shasum -a 256 "$staged" | awk '{print $1}')" == "$expected" ]]
    mv "$staged" "$destination"
  fi
  [[ "$(shasum -a 256 "$destination" | awk '{print $1}')" == "$expected" ]]
done < "$root/upstream/android16-linkerconfig-apex.sources"
protobuf=/opt/homebrew/opt/protobuf
abseil=/opt/homebrew/opt/abseil
"$protobuf/bin/protoc" --version
"$protobuf/bin/protoc" -I"$src" --cpp_out="lite:$out" "$src/apex_manifest.proto"
flags=(-std=c++20 -arch arm64 -O2 -DFMT_HEADER_ONLY
  -I"$protobuf/include" -I"$abseil/include" -I"$out" -I"$src"
  -I"$root/_aosp/system/libbase/include" -I"$root/_aosp/external/fmtlib/include")
xcrun clang++ "${flags[@]}" -c "$src/apexutil.cpp" -o "$out/apexutil.o"
xcrun clang++ "${flags[@]}" -c "$out/apex_manifest.pb.cc" -o "$out/apex_manifest.pb.o"
xcrun libtool -static -o "$out/libapexutil-host.a" "$out/apexutil.o" "$out/apex_manifest.pb.o"
xcrun clang++ "${flags[@]}" "$root/tools/native-loader-policy/apex_manifest_test.cc" \
  "$out/libapexutil-host.a" "$root/_build/libbase-foundation/libandroid-base-darwin.a" \
  "$root/_build/graphics-foundations/liblog-darwin.a" \
  -L"$protobuf/lib" -lprotobuf-lite -L"$abseil/lib" \
  -labsl_log_internal_check_op -labsl_log_internal_message -labsl_raw_logging_internal \
  -Wl,-dead_strip -o "$out/apex-manifest-test"
stage="$(mktemp -d "$out/test.XXXXXX")"
cleanup() {
  rm -f -- "$stage/com.android.i18n/apex_manifest.pb" "$stage/com.android.i18n@1/apex_manifest.pb"
  rmdir "$stage/com.android.i18n" "$stage/com.android.i18n@1" "$stage"
}
trap cleanup EXIT
mkdir "$stage/com.android.i18n" "$stage/com.android.i18n@1"
unzip -p "$root/_build/bootclasspath/api36-i18n-extract/com.android.i18n.apex" apex_manifest.pb \
  > "$stage/com.android.i18n/apex_manifest.pb"
[[ "$(shasum -a 256 "$stage/com.android.i18n/apex_manifest.pb" | awk '{print $1}')" == b3f750cc2e929d6a665f734411052cce2b035780d2e8539fb0df5d7b681cbf58 ]]
cp "$stage/com.android.i18n/apex_manifest.pb" "$stage/com.android.i18n@1/apex_manifest.pb"
"$out/apex-manifest-test" "$stage"
