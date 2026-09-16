#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "$0")/../.." && pwd)"
module="$root/tools/android-apk-app-runtime"
sdk="${ANDROID_HOME:-$HOME/Library/Android/sdk}"
aapt2="$sdk/build-tools/35.0.0/aapt2"
d8="$sdk/build-tools/35.0.0/d8"
android_jar="$sdk/platforms/android-36/android.jar"
ndk_revision="28.2.13676358"
ndk="$sdk/ndk/$ndk_revision"
toolchain="$ndk/toolchains/llvm/prebuilt/darwin-x86_64"
android_clang="$toolchain/bin/aarch64-linux-android35-clang"
build="$root/_build/android-apk-app-runtime"
classes="$build/classes"
dex="$build/dex"
apk="$build/simple-no-native.apk"
debug_apk="$build/simple-debuggable.apk"
jni_apk="$build/simple-jni.apk"

[[ -x "$aapt2" && -x "$d8" && -x "$android_clang" && -f "$android_jar" ]] || {
  echo "Android SDK 35 build tools and platform 36 are required" >&2
  exit 1
}

cargo fmt --manifest-path "$module/Cargo.toml" -- --check
cargo clippy --manifest-path "$module/Cargo.toml" --all-targets -- -D warnings
cargo test --manifest-path "$module/Cargo.toml"

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
  "$module/fixture/FontBootstrap.java" \
  "$module/fixture/MainActivity.java" \
  "$module/fixture/DarwinServiceBridge.java"

app_classes=("$classes"/dev/darwinart/simple/*.class)
"$d8" --lib "$android_jar" --output "$dex" "${app_classes[@]}"

resource_zip="$build/resources.zip"
"$aapt2" compile --dir "$module/fixture/res" -o "$resource_zip"

"$aapt2" link \
  -I "$android_jar" \
  --auto-add-overlay \
  -R "$resource_zip" \
  --manifest "$module/fixture/AndroidManifest.xml" \
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
  --manifest "$module/fixture/AndroidManifest.xml" \
  --min-sdk-version 35 \
  --target-sdk-version 35 \
  -o "$debug_apk"
(cd "$dex" && zip -q -j "$debug_apk" classes.dex)

"$android_clang" -std=c17 -O2 -fPIC -fvisibility=hidden -Wall -Wextra -Werror \
  -shared -nostdlib -fuse-ld=lld -Wl,--build-id=none -Wl,--hash-style=sysv \
  -Wl,-z,now -Wl,-z,norelro -Wl,-soname,libdarwin-art-simple-zchild.so \
  "$module/fixture/native_child.c" -o "$build/jni/lib/arm64-v8a/libdarwin-art-simple-zchild.so"
"$android_clang" -std=c17 -O2 -fPIC -fvisibility=hidden -Wall -Wextra -Werror \
  -shared -nostdlib -fuse-ld=lld -Wl,--build-id=none -Wl,--hash-style=sysv \
  -Wl,-z,now -Wl,-z,norelro -Wl,-soname,libdarwin-art-simple-jni.so \
  -L "$build/jni/lib/arm64-v8a" -Wl,--no-as-needed -ldarwin-art-simple-zchild \
  "$module/fixture/native_app.c" -o "$build/jni/lib/arm64-v8a/libdarwin-art-simple-jni.so"
cp "$apk" "$jni_apk"
(cd "$build/jni" && zip -q -r "$jni_apk" lib)

entries="$(unzip -Z1 "$apk")"
[[ "$(grep -c '^classes\.dex$' <<<"$entries")" == 1 ]]
! grep -Eq '(^|/)classes[2-9][0-9]*\.dex$|\.so$' <<<"$entries"
dex_summary="$($root/_build/dex-probe/dex-probe "$dex/classes.dex")"
expected_dex='AOSP DEX: verified=yes version=35 classes=24 methods=275 class[0]=Ldev/darwinart/simple/DarwinServiceBridge$$ExternalSyntheticLambda0; class[1]=Ldev/darwinart/simple/DarwinServiceBridge$$ExternalSyntheticLambda1; class[2]=Ldev/darwinart/simple/DarwinServiceBridge$$ExternalSyntheticLambda2; class[3]=Ldev/darwinart/simple/DarwinServiceBridge$$ExternalSyntheticLambda3; class[4]=Ldev/darwinart/simple/DarwinServiceBridge$$ExternalSyntheticLambda4; class[5]=Ldev/darwinart/simple/DarwinServiceBridge$$ExternalSyntheticLambda5; class[6]=Ldev/darwinart/simple/DarwinServiceBridge$$ExternalSyntheticLambda6; class[7]=Ldev/darwinart/simple/DarwinServiceBridge$$ExternalSyntheticLambda7; class[8]=Ldev/darwinart/simple/DarwinServiceBridge$$ExternalSyntheticLambda8; class[9]=Ldev/darwinart/simple/DarwinServiceBridge$$ExternalSyntheticLambda9; class[10]=Ldev/darwinart/simple/DarwinServiceBridge$1; class[11]=Ldev/darwinart/simple/DarwinServiceBridge$ActivityClientHandler$$ExternalSyntheticLambda0; class[12]=Ldev/darwinart/simple/DarwinServiceBridge$ActivityClientHandler; class[13]=Ldev/darwinart/simple/DarwinServiceBridge$ActivityManagerHandler; class[14]=Ldev/darwinart/simple/DarwinServiceBridge$ActivityRecord; class[15]=Ldev/darwinart/simple/DarwinServiceBridge$ActivityTaskHandler$$ExternalSyntheticLambda0; class[16]=Ldev/darwinart/simple/DarwinServiceBridge$ActivityTaskHandler; class[17]=Ldev/darwinart/simple/DarwinServiceBridge$DisplayHandler; class[18]=Ldev/darwinart/simple/DarwinServiceBridge$ManagerHandler; class[19]=Ldev/darwinart/simple/DarwinServiceBridge$WindowManagerHandler; class[20]=Ldev/darwinart/simple/DarwinServiceBridge; class[21]=Ldev/darwinart/simple/FontBootstrap; class[22]=Ldev/darwinart/simple/MainActivity$$ExternalSyntheticLambda0; class[23]=Ldev/darwinart/simple/MainActivity;'
[[ "$dex_summary" == "AOSP DEX: verified=yes version=35 classes=48 methods=482 "* ]] &&
  [[ "$dex_summary" == *'Ldev/darwinart/simple/DarwinServiceBridge;'* ]] &&
  [[ "$dex_summary" == *'Ldev/darwinart/simple/MainActivity;'* ]] || {
  printf 'unexpected DEX summary:\n%s\n' "$dex_summary" >&2
  exit 1
}

expected='apk-app-runtime: package=dev.darwinart.simple application=android.app.Application activity=dev.darwinart.simple.MainActivity launch_component=dev.darwinart.simple.MainActivity screen_orientation=-1 descriptor=Ldev/darwinart/simple/MainActivity; activities=dev.darwinart.simple.MainActivity=0x1030241 activity_aliases=none services=dev.darwinart.simple.ImageService>dev.darwinart.simple>1>none>0>1,dev.darwinart.simple.DisabledService>dev.darwinart.simple>0>none>0>0 receivers=none service_metadata=none providers=none application_metadata=none permissions=android.permission.ACCESS_NETWORK_STATE version_code=0 version_name= theme=0x1030241 target_sdk=35 debuggable=0 has_code=1 hardware_accelerated=1 supports_rtl=0 activity_hardware_accelerated=1 label=Darwin ART APK label_res=0x0 icon=none dex=apk-1 manifest_schema=4 native=0 native_root=none'
actual="$(cargo run -q --manifest-path "$module/Cargo.toml" -- "$apk")"
[[ "$actual" == "$expected" ]] || {
  printf 'unexpected inspector output:\n%s\n' "$actual" >&2
  exit 1
}
debug_actual="$(cargo run -q --manifest-path "$module/Cargo.toml" -- "$debug_apk")"
[[ "$debug_actual" == *' target_sdk=35 debuggable=1 label='* ]] || {
  printf 'debuggable manifest state was not decoded:\n%s\n' "$debug_actual" >&2
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

native_apk="$build/accept-native.apk"
cp "$apk" "$native_apk"
mkdir -p "$build/reject/lib/arm64-v8a"
: >"$build/reject/lib/arm64-v8a/libforbidden.so"
(cd "$build/reject" && zip -q "$native_apk" lib/arm64-v8a/libforbidden.so)
native_metadata="$(cargo run -q --manifest-path "$module/Cargo.toml" -- "$jni_apk")"
grep -F 'dex=apk-1 native=2 native_root=libdarwin-art-simple-jni.so' \
  <<<"$native_metadata" >/dev/null

secondary_apk="$build/reject-secondary-dex.apk"
cp "$apk" "$secondary_apk"
cp "$dex/classes.dex" "$build/classes2.dex"
(cd "$build" && zip -q "$secondary_apk" classes2.dex)
secondary_metadata="$(cargo run -q --manifest-path "$module/Cargo.toml" -- "$secondary_apk")"
grep -F 'dex=apk-2 native=0 native_root=none' <<<"$secondary_metadata" >/dev/null

git -C "$root" diff --check
echo "android-apk-app-runtime: PASS APK=real-binary-manifest multidex=accepted native-so=accepted launcher=dev.darwinart.simple.MainActivity"
