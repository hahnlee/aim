#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "$0")/../.." && pwd)"
module="$root/tools/fixtures/simple-apk"
sdk="${ANDROID_HOME:-$HOME/Library/Android/sdk}"
aapt2="$sdk/build-tools/35.0.0/aapt2"
d8="$sdk/build-tools/35.0.0/d8"
android_jar="$sdk/platforms/android-36/android.jar"
ndk_revision="28.2.13676358"
ndk="$sdk/ndk/$ndk_revision"
toolchain="$ndk/toolchains/llvm/prebuilt/darwin-x86_64"
android_clang="$toolchain/bin/aarch64-linux-android35-clang"
build="$root/_build/simple-apk-fixture"
classes="$build/classes"
dex="$build/dex"
apk="$build/simple-no-native.apk"
debug_apk="$build/simple-debuggable.apk"
jni_apk="$build/simple-jni.apk"

[[ -x "$aapt2" && -x "$d8" && -x "$android_clang" && -f "$android_jar" ]] || {
  echo "Android SDK 35 build tools and platform 36 are required" >&2
  exit 1
}

if [[ ! -f "$root/_build/dex-probe/dex/classes.dex" ]]; then
  cargo run -q -p art-bootstrap -- build-dex
fi

if [[ -d "$build" ]]; then
  chmod -R u+w "$build"
fi
rm -rf "$build"
mkdir -p "$classes" "$dex" "$build/jni/lib/arm64-v8a"
javac --release 8 -encoding UTF-8 \
  -classpath "$android_jar:$root/_build/dex-probe/classes" \
  -d "$classes" \
  "$module/FontBootstrap.java" \
  "$module/MainActivity.java"

app_classes=("$classes"/dev/darwinart/simple/*.class)
"$d8" --lib "$android_jar" --output "$dex" "${app_classes[@]}"

resource_zip="$build/resources.zip"
"$aapt2" compile --dir "$module/res" -o "$resource_zip"

"$aapt2" link \
  -I "$android_jar" \
  --auto-add-overlay \
  -R "$resource_zip" \
  --manifest "$module/AndroidManifest.xml" \
  --min-sdk-version 35 \
  --target-sdk-version 35 \
  -o "$apk"
(cd "$dex" && zip -q -j "$apk" classes.dex)

# aapt2's --debug-mode sets android:debuggable=true in binary XML, matching
# the package-manager input used for ART's Java-debuggable process policy.
"$aapt2" link \
  -I "$android_jar" \
  --auto-add-overlay \
  --debug-mode \
  -R "$resource_zip" \
  --manifest "$module/AndroidManifest.xml" \
  --min-sdk-version 35 \
  --target-sdk-version 35 \
  -o "$debug_apk"
(cd "$dex" && zip -q -j "$debug_apk" classes.dex)

"$android_clang" -std=c17 -O2 -fPIC -fvisibility=hidden -Wall -Wextra -Werror \
  -shared -nostdlib -fuse-ld=lld -Wl,--build-id=none -Wl,--hash-style=sysv \
  -Wl,-z,now -Wl,-z,norelro -Wl,-soname,libdarwin-art-simple-zchild.so \
  "$module/native_child.c" -o "$build/jni/lib/arm64-v8a/libdarwin-art-simple-zchild.so"
"$android_clang" -std=c17 -O2 -fPIC -fvisibility=hidden -Wall -Wextra -Werror \
  -shared -nostdlib -fuse-ld=lld -Wl,--build-id=none -Wl,--hash-style=sysv \
  -Wl,-z,now -Wl,-z,norelro -Wl,-soname,libdarwin-art-simple-jni.so \
  -L "$build/jni/lib/arm64-v8a" -Wl,--no-as-needed -ldarwin-art-simple-zchild \
  "$module/native_app.c" -o "$build/jni/lib/arm64-v8a/libdarwin-art-simple-jni.so"
cp "$apk" "$jni_apk"
(cd "$build/jni" && zip -q -r "$jni_apk" lib)

entries="$(unzip -Z1 "$apk")"
[[ "$(grep -c '^classes\.dex$' <<<"$entries")" == 1 ]]
! grep -Eq '(^|/)classes[2-9][0-9]*\.dex$|\.so$' <<<"$entries"
dex_summary="$($root/_build/dex-probe/dex-probe "$dex/classes.dex")"
[[ "$dex_summary" == "AOSP DEX: verified=yes version=35 classes=3 methods=79 "* ]] &&
  [[ "$dex_summary" == *'Ldev/darwinart/simple/MainActivity;'* ]] &&
  [[ "$dex_summary" != *'Ldev/darwinart/simple/DarwinServiceBridge'* ]] || {
  printf 'unexpected DEX summary:\n%s\n' "$dex_summary" >&2
  exit 1
}

"$aapt2" dump badging "$apk" | grep -F "launchable-activity: name='dev.darwinart.simple.MainActivity'" >/dev/null

jni_entries="$(unzip -Z1 "$jni_apk")"
[[ "$(grep -c '^classes\.dex$' <<<"$jni_entries")" == 1 ]]
grep -Fx 'lib/arm64-v8a/libdarwin-art-simple-jni.so' <<<"$jni_entries" >/dev/null
grep -Fx 'lib/arm64-v8a/libdarwin-art-simple-zchild.so' <<<"$jni_entries" >/dev/null
file "$build/jni/lib/arm64-v8a/libdarwin-art-simple-jni.so" \
  "$build/jni/lib/arm64-v8a/libdarwin-art-simple-zchild.so" |
  grep -F 'ELF 64-bit LSB shared object, ARM aarch64' >/dev/null
llvm_readelf="$toolchain/bin/llvm-readelf"
"$llvm_readelf" -d "$build/jni/lib/arm64-v8a/libdarwin-art-simple-jni.so" |
  grep -F '(SONAME)' >/dev/null
"$llvm_readelf" -d "$build/jni/lib/arm64-v8a/libdarwin-art-simple-jni.so" |
  grep -F 'libdarwin-art-simple-zchild.so' >/dev/null

git -C "$root" diff --check
echo "simple-apk-fixture: PASS APK=real-binary-manifest native-so=accepted launcher=dev.darwinart.simple.MainActivity"
