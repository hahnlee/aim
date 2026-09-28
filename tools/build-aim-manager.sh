#!/usr/bin/env bash
set -euo pipefail

project_root="$(cd "$(dirname "$0")/.." && pwd)"
source "$project_root/tools/lib/sign-runtime-images.sh"
source "$project_root/tools/lib/package-system-root.sh"
source_root="$project_root/apps/AimManager"
app="$project_root/_build/aim Manager.app"
contents="$app/Contents"
binary="$contents/MacOS/AimManager"
shim_launcher="$contents/Resources/AimAppLauncher"
sdk="$(xcrun --sdk macosx --show-sdk-path)"
unwind_provider="$project_root/_build/android-unwind-provider/libaim_android_unwind.so"

# Conscrypt uses the Android unwind ABI even for Java-only APKs. Reject an
# incomplete runtime before removing the existing app bundle, not at launch.
[[ -f "$unwind_provider" ]] || {
  echo "manager runtime input missing: $unwind_provider" >&2
  exit 69
}
native_baseline="$(bash "$project_root/tools/prepare-runtime-system-root.sh")"
cargo run -q --manifest-path "$project_root/Cargo.toml" -p art-bootstrap -- build-runtime-payload-incremental

[[ "$app" == "$project_root/_build/aim Manager.app" ]] || {
  echo "refusing unexpected manager output: $app" >&2
  exit 70
}
rm -rf "$app"
mkdir -p "$contents/MacOS" "$contents/Resources/aim"
cp "$source_root/Info.plist" "$contents/Info.plist"
xcrun clang -arch arm64 -isysroot "$sdk" -mmacosx-version-min=14.0 \
  -fobjc-arc -fmodules -Wall -Wextra -Werror \
  -framework AppKit -framework ServiceManagement -framework UniformTypeIdentifiers \
  "$source_root"/Sources/*.m -o "$binary"
xcrun clang -arch arm64 -isysroot "$sdk" -mmacosx-version-min=14.0 \
  -fobjc-arc -fmodules -Wall -Wextra -Werror \
  -framework AppKit \
  "$source_root/AppShim/AIMAppShimMain.m" -o "$shim_launcher"

cargo build --manifest-path "$project_root/Cargo.toml" -q --release \
  -p aim-host \
  -p aim-profile --bins \
  -p aim-native-artifact --bin aim-native-resolve
"$project_root/tools/materialize-moltenvk.sh" >/dev/null
runtime="$contents/Resources/aim"
host_app="$runtime/AimHost.app"
mkdir -p "$host_app/Contents/MacOS"
cp "$source_root/HostInfo.plist" "$host_app/Contents/Info.plist"
cp "$project_root/target/release/aim-host" "$host_app/Contents/MacOS/aim-host"
chmod +x "$host_app/Contents/MacOS/aim-host"
# The embedded Host.app is the executable launched by installed apps.  Preserve
# the Android task x18 ABI on this copy as well as on the runtime payload copy.
"$project_root/tools/declare-darwin-x18-abi.sh" \
  "$host_app/Contents/MacOS/aim-host"
copy_file() {
  local source="$1"
  local destination="$runtime/$2"
  [[ -f "$source" ]] || {
    echo "manager runtime input missing: $source" >&2
    exit 69
  }
  mkdir -p "$(dirname "$destination")"
  cp "$source" "$destination"
}
copy_tree() {
  local source="$1"
  local destination="$runtime/$2"
  [[ -d "$source" ]] || {
    echo "manager runtime tree missing: $source" >&2
    exit 69
  }
  mkdir -p "$(dirname "$destination")"
  ditto "$source" "$destination"
}

copy_file "$project_root/target/release/aim-host" target/release/aim-host
for helper in aimctl aimd aim-native-resolve; do
  copy_file "$project_root/target/release/$helper" "target/release/$helper"
done
copy_file "$project_root/tools/run-android-apk-app.sh" tools/run-android-apk-app.sh
copy_file "$project_root/tools/lib/system-private-data.sh" tools/lib/system-private-data.sh
copy_file "$project_root/tools/lib/runtime-system-image.sh" tools/lib/runtime-system-image.sh
copy_file "$project_root/tools/lib/system-service-environment.sh" tools/lib/system-service-environment.sh
copy_file "$project_root/tools/lib/runtime-system-service.sh" tools/lib/runtime-system-service.sh
copy_file "$project_root/tools/lib/package-manager-launch.sh" tools/lib/package-manager-launch.sh
copy_file "$project_root/tools/prepare-aim-host.sh" tools/prepare-aim-host.sh
copy_file "$project_root/tools/declare-darwin-x18-abi.sh" tools/declare-darwin-x18-abi.sh
copy_file "$project_root/config/aim-host.entitlements" config/aim-host.entitlements
copy_file "$project_root/_build/runtime-graphics-link-probe/libaim_runtime_graphics.dylib" \
  _build/runtime-graphics-link-probe/libaim_runtime_graphics.dylib
copy_file "$project_root/_build/runtime-graphics-link-probe/libopenjdk-named-jni-owner.dylib" \
  _build/runtime-graphics-link-probe/libopenjdk-named-jni-owner.dylib
copy_file "$project_root/_build/android16-core-oj-compat/core-oj-compat.jar" \
  _build/android16-core-oj-compat/core-oj-compat.jar
copy_file "$project_root/_build/android16-framework-compat/framework-compat.jar" \
  _build/android16-framework-compat/framework-compat.jar
copy_file "$project_root/_build/bootclasspath/core-icu4j-api36.jar" \
  _build/bootclasspath/core-icu4j-api36.jar
copy_file "$project_root/_build/runtime-support-dex/dex/classes.dex" _build/runtime-support-dex/dex/classes.dex
copy_tree "$project_root/_build/icu-runtime-adapters/runtime" _build/icu-runtime-adapters/runtime
copy_file "$project_root/_prebuilt/android-16/bootclasspath/core-libart.jar" \
  _prebuilt/android-16/bootclasspath/core-libart.jar
copy_file "$project_root/_prebuilt/android-16/bootclasspath/framework-location.jar" \
  _prebuilt/android-16/bootclasspath/framework-location.jar
copy_file "$project_root/_prebuilt/android-16/resources/framework-res.apk" \
  _prebuilt/android-16/resources/framework-res.apk
# System/APEX/linkerconfig belong to the bundled runtime, not to each APK or
# writable profile. The launch-root cutover is separate from packaging.
source "$project_root/tools/lib/system-services-artifact.sh"
services_jar="$(aim_prepare_system_services_artifact)"
aim_package_system_root \
  "$native_baseline" "$runtime/android/system-root.tar" \
  "$project_root/_build/android16-system-fonts" "$project_root/_prebuilt/android-16/resources/framework-res.apk" "$services_jar"
copy_file "$project_root/probes/button/fonts.xml" probes/button/fonts.xml
copy_file "$project_root/_aosp/external/skia/resources/fonts/Roboto-Regular.ttf" \
  _aosp/external/skia/resources/fonts/Roboto-Regular.ttf
for relative in \
  art/javalib/okhttp.jar \
  conscrypt/javalib/conscrypt.jar \
  conscrypt/lib64/libc++.so \
  conscrypt/lib64/libcrypto.so \
  conscrypt/lib64/libjavacrypto.so \
  conscrypt/lib64/libssl.so \
  bt/javalib/framework-bluetooth.jar \
  mediaprovider/javalib/framework-mediaprovider.jar \
  permission/javalib/framework-permission.jar \
  permission/javalib/framework-permission-s.jar; do
  copy_file "$project_root/_build/android16-ps16k-r07/extracted/$relative" \
    "_build/android16-ps16k-r07/extracted/$relative"
done
copy_file "$unwind_provider" _build/android-unwind-provider/libaim_android_unwind.so
for library in "$project_root/_build/angle-source/out/AimRelease/"*.dylib; do
  copy_file "$library" "_build/angle-source/out/AimRelease/$(basename "$library")"
done
copy_file "$project_root/_build/moltenvk/libMoltenVK.dylib" \
  _build/moltenvk/libMoltenVK.dylib
copy_file "$project_root/_build/tracing-perfetto/perfetto-out/libperfetto_c.dylib" \
  _build/tracing-perfetto/perfetto-out/libperfetto_c.dylib
copy_file "$project_root/_build/moltenvk/LICENSE" _build/moltenvk/LICENSE

runtime_dylib="$runtime/_build/runtime-graphics-link-probe/libaim_runtime_graphics.dylib"
# Ship the resolver, original inventory and every selected component. Verify
# relocation before signing; developer paths must not remain launch inputs.
python3 "$project_root/tools/bootclasspath/package_runtime.py" "$runtime"
lz4_source="$(brew --prefix lz4)/lib/liblz4.1.dylib"
copy_file "$lz4_source" _build/runtime-graphics-link-probe/liblz4.1.dylib
install_name_tool -change "$lz4_source" @loader_path/liblz4.1.dylib "$runtime_dylib"
zstd_source="$(brew --prefix zstd)/lib/libzstd.1.dylib"
copy_file "$zstd_source" _build/runtime-graphics-link-probe/libzstd.1.dylib
install_name_tool -change "$zstd_source" @loader_path/libzstd.1.dylib "$runtime_dylib"

chmod +x "$runtime/tools/run-android-apk-app.sh" \
  "$runtime/tools/declare-darwin-x18-abi.sh" \
  "$runtime/target/release/aim-host" \
  "$runtime/target/release/"*
"$runtime/tools/declare-darwin-x18-abi.sh" \
  "$runtime/target/release/aim-host"
codesign --force --sign - --options runtime --timestamp=none \
  --entitlements "$project_root/config/aim-host.entitlements" \
  "$runtime/target/release/aim-host" >/dev/null
aim_sign_runtime_images "$runtime"
for helper in "$runtime/target/release/"*; do
  [[ "$helper" == "$runtime/target/release/aim-host" ]] && continue
  codesign --force --sign - --timestamp=none "$helper" >/dev/null
done
codesign --force --sign - --options runtime --timestamp=none \
  --entitlements "$project_root/config/aim-host.entitlements" "$host_app" >/dev/null
codesign --force --sign - --timestamp=none "$shim_launcher" >/dev/null
codesign --force --sign - --timestamp=none "$app" >/dev/null
plutil -lint "$contents/Info.plist" >/dev/null
codesign --verify --deep --strict "$app"

echo "$app"
