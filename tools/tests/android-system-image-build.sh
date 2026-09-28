#!/bin/bash
set -euo pipefail
export LC_ALL=C

root="$(cd "$(dirname "$0")/../.." && pwd)"
builder="$root/tools/build-android-system-image.sh"
stage="$(mktemp -d "${TMPDIR:-/tmp}/android-system-image-build.XXXXXX")"
cleanup() {
  find "$stage" -type d -exec chmod u+w {} + 2>/dev/null || true
  rm -rf -- "$stage"
}
trap cleanup EXIT

native="$stage/native"
fonts="$stage/fonts"
framework_dir="$stage/framework"
destination="$stage/out/system-root.tar"
mkdir -p "$native/apex" "$native/system/etc" "$native/system/lib64" \
  "$native/linkerconfig" "$fonts/system/etc" "$fonts/system/fonts" \
  "$framework_dir" "$stage/out"

# Native bytes must meet the production inventory contract even in this test.
# Reuse the verified build snapshot, never relax the production hash gate.
native_source="$(bash "$root/tools/prepare-runtime-system-root.sh")"
while read -r digest guest; do
  [[ -n "$digest" && "$digest" != \#* ]] || continue
  cp -c "$native_source$guest" "$native$guest"
done < "$root/upstream/android16-system-native-files.lock"
for manifest in apex/apex-info-list.xml \
  apex/com.android.art/apex_manifest.pb \
  apex/com.android.runtime/apex_manifest.pb \
  apex/com.android.conscrypt/apex_manifest.pb \
  apex/com.android.i18n/apex_manifest.pb \
  apex/com.android.tzdata/apex_manifest.pb; do
  mkdir -p "$native/$(dirname "$manifest")"
  printf 'fixture:%s\n' "$manifest" >"$native/$manifest"
done
printf 'fixture-linker-config\n' >"$native/linkerconfig/ld.config.txt"
printf 'jni com_android_conscrypt libjavacrypto.so\n' >"$native/linkerconfig/apex.libraries.config.txt"
printf 'fixture-public-libraries\n' >"$native/system/etc/public.libraries.txt"
printf 'fixture-linker-config-pb\n' >"$native/system/etc/linker.config.pb"

printf '<fonts><font>Roboto-Regular.ttf</font></fonts>\n' >"$fonts/system/etc/fonts.xml"
printf '<fonts><font>Roboto-Regular.ttf</font></fonts>\n' >"$fonts/system/etc/font_fallback.xml"
printf 'Roboto fixture bytes\n' >"$fonts/system/fonts/Roboto-Regular.ttf"
fonts_digest="$(shasum -a 256 "$fonts/system/etc/fonts.xml" | awk '{print $1}')"
font_fallback_digest="$(shasum -a 256 "$fonts/system/etc/font_fallback.xml" | awk '{print $1}')"
roboto_digest="$(shasum -a 256 "$fonts/system/fonts/Roboto-Regular.ttf" | awk '{print $1}')"
cat >"$fonts/manifest.json" <<EOF
{
  "version": 1,
  "files": [
    {"path": "system/etc/fonts.xml", "sha256": "$fonts_digest"},
    {"path": "system/etc/font_fallback.xml", "sha256": "$font_fallback_digest"},
    {"path": "system/fonts/Roboto-Regular.ttf", "sha256": "$roboto_digest"}
  ],
  "absent_in_image": []
}
EOF

framework_res="$framework_dir/framework-res.apk"
framework_stage="$stage/framework-content"
mkdir -p "$framework_stage"
printf 'fixture manifest\n' >"$framework_stage/AndroidManifest.xml"
printf 'fixture resources\n' >"$framework_stage/resources.arsc"
(cd "$framework_stage" && zip -q "$framework_res" AndroidManifest.xml resources.arsc)
# Keep input metadata fixed so a repeated package operation tests archive
# reproducibility rather than a changing source timestamp.
touch -t 200001010000 "$framework_res"

native_before="$(shasum -a 256 "$native/system/lib64/libc++.so")"
fonts_before="$(shasum -a 256 "$fonts/system/fonts/Roboto-Regular.ttf")"
framework_before="$(shasum -a 256 "$framework_res")"

"$builder" "$native" "$destination" "$fonts" "$framework_res" >/dev/null
[[ -f "$destination" && ! -L "$destination" ]]

expanded="$stage/expanded"
mkdir "$expanded"
/usr/bin/tar -xf "$destination" -C "$expanded"
cmp "$native/system/lib64/libc++.so" "$expanded/system/lib64/libc++.so"
cmp "$fonts/system/etc/fonts.xml" "$expanded/system/etc/fonts.xml"
cmp "$fonts/system/etc/font_fallback.xml" "$expanded/system/etc/font_fallback.xml"
cmp "$fonts/system/fonts/Roboto-Regular.ttf" "$expanded/system/fonts/Roboto-Regular.ttf"
cmp "$framework_res" "$expanded/system/framework/framework-res.apk"
cmp "$root/_build/android16-system-services/services.jar" "$expanded/system/framework/services.jar"
[[ "$(shasum -a 256 "$native/system/lib64/libc++.so")" == "$native_before" ]]
[[ "$(shasum -a 256 "$fonts/system/fonts/Roboto-Regular.ttf")" == "$fonts_before" ]]
[[ "$(shasum -a 256 "$framework_res")" == "$framework_before" ]]

first_digest="$(shasum -a 256 "$destination")"
first_inode="$(stat -f %i "$destination")"
# Wrong service bytes must not replace the last verified archive.
if "$builder" "$native" "$destination" "$fonts" "$framework_res" "$framework_res" >/dev/null 2>&1; then
  echo 'non-service JAR unexpectedly accepted' >&2
  exit 1
fi
[[ "$(shasum -a 256 "$destination")" == "$first_digest" ]]
[[ "$(stat -f %i "$destination")" == "$first_inode" ]]
sleep 2
"$builder" "$native" "$destination" "$fonts" "$framework_res" >/dev/null
[[ "$(shasum -a 256 "$destination")" == "$first_digest" ]]
[[ "$(stat -f %i "$destination")" == "$first_inode" ]]
[[ -z "$(find "$stage/out" -maxdepth 1 -name '.system-root-stage.*' -print -quit)" ]]

# A stale or altered input must fail before replacing a previously good archive.
printf 'invalid native replacement\n' > "$native/system/lib64/libc++.so"
if "$builder" "$native" "$destination" "$fonts" "$framework_res" >/dev/null 2>&1; then
  echo 'changed native input unexpectedly accepted' >&2
  exit 1
fi
[[ "$(shasum -a 256 "$destination")" == "$first_digest" ]]
[[ "$(stat -f %i "$destination")" == "$first_inode" ]]
cp "$native_source/system/lib64/libc++.so" "$native/system/lib64/libc++.so"

# A destination symlink is never followed or replaced by this build edge.
rm -f "$destination"
ln -s "$stage/other.tar" "$destination"
if "$builder" "$native" "$destination" "$fonts" "$framework_res" >/dev/null 2>&1; then
  echo 'symlink destination unexpectedly accepted' >&2
  exit 1
fi

echo 'android system image build: bounded archive, atomic publication, reproducible reuse, input preservation PASS'
