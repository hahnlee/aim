#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
framework="$root/_prebuilt/android-16/bootclasspath/framework.jar"
core_oj="$root/_prebuilt/android-16/bootclasspath/core-oj.jar"
android_jar="$HOME/Library/Android/sdk/platforms/android-36/android.jar"
out="$root/_build/android16-framework-compat"
classes="$out/classes"
source_root="$root/_aosp/android16-sdkextensions-compat"
lock="$root/upstream/android16-sdkextensions-compat.lock"
sdkextensions_patch="$root/patches/sdkextensions/0001-detached-framework-sdk-level.patch"
mkdir -p "$classes" "$out/input" "$source_root/java/android/os/ext"

# shellcheck disable=SC1090
source "$lock"
sha256() { shasum -a 256 "$1" | awk '{print $1}'; }
sdkextensions_source="$source_root/$SDKEXTENSIONS_SOURCE"
if [[ ! -f "$sdkextensions_source" ]]; then
  staged_download="$(mktemp "${TMPDIR:-/tmp}/darwin-art-sdkextensions.XXXXXX")"
  /usr/bin/curl -fsSL \
    "https://android.googlesource.com/$SDKEXTENSIONS_PROJECT/+/$SDKEXTENSIONS_REVISION/$SDKEXTENSIONS_SOURCE?format=TEXT" \
    | /usr/bin/base64 -D > "$staged_download"
  [[ "$(sha256 "$staged_download")" == "$SDKEXTENSIONS_SOURCE_SHA256" ]]
  mv "$staged_download" "$sdkextensions_source"
fi
[[ "$(sha256 "$sdkextensions_source")" == "$SDKEXTENSIONS_SOURCE_SHA256" ]]
[[ "$(sha256 "$sdkextensions_patch")" == "$SDKEXTENSIONS_PATCH_SHA256" ]]
patched_root="$(mktemp -d "${TMPDIR:-/tmp}/darwin-art-sdkextensions-source.XXXXXX")"
trap 'rm -rf "$patched_root"' EXIT
mkdir -p "$patched_root/java/android/os/ext"
cp "$sdkextensions_source" "$patched_root/$SDKEXTENSIONS_SOURCE"
patch --batch --forward -p1 -d "$patched_root" < "$sdkextensions_patch" >/dev/null
patched_sdkextensions="$patched_root/$SDKEXTENSIONS_SOURCE"
[[ "$(sha256 "$patched_sdkextensions")" == "$PATCHED_SDKEXTENSIONS_SHA256" ]]

[[ -f "$framework" && -f "$core_oj" && -f "$android_jar" ]] || {
  echo "android16-framework-compat: framework/android.jar missing" >&2
  exit 69
}

javac --release 8 -encoding UTF-8 -d "$classes" -classpath "$android_jar" \
  "$root/tools/android-framework-compat/compile-stubs/android/annotation/IntDef.java" \
  "$root/tools/android-framework-compat/compile-stubs/android/annotation/NonNull.java" \
  "$root/tools/android-framework-compat/compile-stubs/android/os/SystemProperties.java" \
  "$patched_sdkextensions" \
  "$root/tools/android-framework-compat/src/android/provider/DeviceConfig.java" \
  "$root/tools/android-framework-compat/src/android/util/StatsEvent.java" \
  "$root/tools/android-framework-compat/src/android/util/StatsLog.java" \
  "$root/compat/java/android/media/MediaCommunicationManager.java" \
  "$root/tools/android-framework-compat/src/android/telephony/TelephonyManager.java" \
  "$root/tools/android-framework-compat/src/dev/darwinart/security/DarwinSecurityProvider.java" \
  "$root/tools/android-framework-compat/src/dev/darwinart/security/DarwinHttpsDiagnostic.java" \
  "$root/tools/android-framework-compat/src/dev/darwinart/security/DarwinAndroidCAStore.java" \
  "$root/tools/android-framework-compat/src/dev/darwinart/security/DarwinAndroidKeyStore.java" \
  "$root/tools/android-framework-compat/src/dev/darwinart/security/DarwinSecureRandom.java" \
  "$root/tools/android-framework-compat/src/dev/darwinart/security/DarwinTrustManagerFactory.java"
unzip -p "$framework" classes.dex > "$out/input/framework.dex"
find "$out" -maxdepth 1 -type f -name 'classes*.dex' -delete
if [[ -f "$out/framework-compat.jar" ]]; then
  mv "$out/framework-compat.jar" "$out/framework-compat.previous.jar"
fi
"$HOME/Library/Android/sdk/build-tools/35.0.1/d8" \
  --lib "$android_jar" --output "$out" \
  "$out/input/framework.dex" \
  "$classes/android/provider/DeviceConfig.class" \
  "$classes/android/provider/DeviceConfig\$Properties.class" \
  "$classes/android/provider/DeviceConfig\$OnPropertiesChangedListener.class" \
  "$classes/android/util/StatsEvent.class" \
  "$classes/android/util/StatsEvent\$Builder.class" \
  "$classes/android/util/StatsLog.class" \
  "$classes/android/media/MediaCommunicationManager.class" \
  "$classes/android/media/MediaCommunicationManager\$SessionCallback.class" \
  "$classes/android/telephony/TelephonyManager.class" \
  "$classes/dev/darwinart/security/DarwinSecurityProvider.class" \
  "$classes/dev/darwinart/security/DarwinHttpsDiagnostic.class" \
  "$classes/dev/darwinart/security/DarwinAndroidCAStore.class" \
  "$classes/dev/darwinart/security/DarwinAndroidKeyStore.class" \
  "$classes/dev/darwinart/security/DarwinAndroidKeyStore\$HmacKey.class" \
  "$classes/dev/darwinart/security/DarwinAndroidKeyStore\$HmacKeyGenerator.class" \
  "$classes/dev/darwinart/security/DarwinAndroidKeyStore\$HmacMac.class" \
  "$classes/dev/darwinart/security/DarwinSecureRandom.class" \
  "$classes/dev/darwinart/security/DarwinTrustManagerFactory.class" \
  "$classes/dev/darwinart/security/DarwinTrustManagerFactory\$DarwinTrustManager.class" \
  "$classes/android/os/ext/SdkExtensions.class"
mv "$out/classes.dex" "$out/framework-compat.raw.dex"
staged="$(mktemp -d "$out/staged.XXXXXX")"
unzip -q "$framework" -d "$staged"
cp "$out/framework-compat.raw.dex" "$staged/classes.dex"
next_dex=6
for extra_dex in "$out"/classes[2-9]*.dex; do
  [[ -f "$extra_dex" ]] || continue
  while [[ -e "$staged/classes${next_dex}.dex" ]]; do
    next_dex=$((next_dex + 1))
  done
  cp "$extra_dex" "$staged/classes${next_dex}.dex"
  next_dex=$((next_dex + 1))
done
(cd "$staged" && zip -q -qr "$out/framework-compat.jar" .)

# Connectivity framework policy remains owned by the pinned Android 16
# framework-connectivity boot jars. The compatibility output must not publish
# a process-local definition ahead of those originals on the boot class path.
connectivity_audit="$patched_root/framework-compat-dex-packages.txt"
"$HOME/Library/Android/sdk/cmdline-tools/latest/bin/apkanalyzer" \
  dex packages --defined-only "$out/framework-compat.jar" > "$connectivity_audit"
for framework_class in \
  android.net.ConnectivityManager \
  android.net.LinkProperties \
  android.net.Network \
  android.net.NetworkCapabilities \
  android.net.NetworkInfo \
  android.net.NetworkRequest; do
  if awk -v target="$framework_class" \
      '$1 == "C" && ($NF == target || index($NF, target "$") == 1) { found = 1 }
       END { exit(found ? 0 : 1) }' "$connectivity_audit"; then
    echo "android16-framework-compat: shadowed framework class: $framework_class" >&2
    exit 70
  fi
done
core_out="$root/_build/android16-core-oj-compat"
mkdir -p "$core_out"
core_staged="$(mktemp -d "$core_out/staged.XXXXXX")"
unzip -q "$core_oj" -d "$core_staged"
sed '/^security\.provider\.[0-9][0-9]*=/d' \
  "$core_staged/java/security/security.properties" \
  > "$core_staged/java/security/security.properties.updated"
mv "$core_staged/java/security/security.properties.updated" \
  "$core_staged/java/security/security.properties"
cat >> "$core_staged/java/security/security.properties" <<'EOF'

security.provider.1=dev.darwinart.security.DarwinSecurityProvider
security.provider.2=com.android.org.conscrypt.OpenSSLProvider
security.provider.3=sun.security.provider.CertPathProvider
security.provider.4=com.android.org.conscrypt.JSSEProvider
securerandom.source=file:/dev/urandom
EOF
(cd "$core_staged" && zip -q -qr "$core_out/core-oj-compat.jar" .)

# core-libart is deliberately kept as the pinned DEX input at runtime. Its
# hidden Java APIs are nevertheless part of the platform boot class path and
# javac cannot inspect a DEX container. Publish a compiler-only companion from
# the shared compiler-capability sources so every ART test sees the same hidden
# API surface without putting compiler classes in an app DEX.
libcore_compiler_out="$root/_build/android16-libcore-compiler-api"
libcore_compiler_classes="$libcore_compiler_out/classes"
mkdir -p "$libcore_compiler_classes"
# Keep the compiler views tied to the same pinned libcore checkout that owns
# the runtime classes. The reduced declarations below are only a classfile
# projection for javac; ART still loads the complete implementation from the
# pinned core-libart DEX.
for source in \
  "$root/_aosp/libcore-full/luni/src/main/java/libcore/util/FP16.java" \
  "$root/_aosp/libcore-full/libart/src/main/java/java/lang/StringFactory.java"; do
  [[ -f "$source" ]] || {
    echo "android16-framework-compat: pinned libcore source missing: $source" >&2
    exit 69
  }
done
libcore_compiler_sources=(
  "$root/probes/compiler-stubs/android/compat/annotation/UnsupportedAppUsage.java"
  "$root/probes/compiler-stubs/com/android/art/flags/Flags.java"
  "$root/probes/compiler-stubs/com/android/libcore/Flags.java"
  "$root/probes/compiler-stubs/dalvik/annotation/compat/VersionCodes.java"
  "$root/probes/compiler-stubs/dalvik/annotation/optimization/DeadReferenceSafe.java"
  "$root/probes/compiler-stubs/dalvik/annotation/optimization/NeverInline.java"
  "$root/probes/compiler-stubs/dalvik/annotation/optimization/ReachabilitySensitive.java"
  "$root/probes/compiler-stubs/dalvik/system/AnnotatedStackTraceElement.java"
  "$root/probes/compiler-stubs/dalvik/system/BaseDexClassLoader.java"
  "$root/probes/compiler-stubs/dalvik/system/DexFile.java"
  "$root/probes/compiler-stubs/dalvik/system/DelegateLastClassLoader.java"
  "$root/probes/compiler-stubs/dalvik/system/EmulatedStackFrame.java"
  "$root/probes/compiler-stubs/dalvik/system/PathClassLoader.java"
  "$root/probes/compiler-stubs/dalvik/system/VMDebug.java"
  "$root/probes/compiler-stubs/dalvik/system/VMRuntime.java"
  "$root/probes/compiler-stubs/dalvik/system/ZygoteHooks.java"
  "$root/probes/compiler-stubs/android/os/IBinder.java"
  "$root/probes/compiler-stubs/android/os/Parcelable.java"
  "$root/probes/compiler-stubs/android/os/Parcel.java"
  "$root/probes/compiler-stubs/java/lang/StringFactory.java"
  "$root/probes/compiler-stubs/java/lang/invoke/Transformers.java"
  "$root/probes/compiler-stubs/jdk/internal/misc/Unsafe.java"
  "$root/probes/compiler-stubs/libcore/util/EmptyArray.java"
  "$root/probes/compiler-stubs/libcore/util/FP16.java"
  "$root/probes/compiler-stubs/libcore/util/NativeAllocationRegistry.java"
  "$root/probes/compiler-stubs/org/apache/harmony/dalvik/ddmc/Chunk.java"
  "$root/probes/compiler-stubs/org/apache/harmony/dalvik/ddmc/ChunkHandler.java"
  "$root/probes/compiler-stubs/org/apache/harmony/dalvik/ddmc/DdmServer.java"
  "$root/probes/compiler-stubs/org/apache/harmony/dalvik/ddmc/DdmVmInternal.java"
  "$root/probes/compiler-stubs/sun/misc/Cleaner.java"
)
for source in "${libcore_compiler_sources[@]}"; do
  [[ -f "$source" ]] || {
    echo "android16-framework-compat: compiler API source missing: $source" >&2
    exit 69
  }
done
find "$libcore_compiler_classes" -type f -delete
javac -g -Xlint:-options -implicit:none -source 8 -target 8 -encoding UTF-8 \
  -bootclasspath "$libcore_compiler_classes:$android_jar:$HOME/Library/Android/sdk/platforms/android-36/core-for-system-modules.jar" \
  -d "$libcore_compiler_classes" "${libcore_compiler_sources[@]}"
jar -cf "$libcore_compiler_out/core-libart-compiler.jar" \
  -C "$libcore_compiler_classes" .
echo "android16-framework-compat: PASS $out/framework-compat.jar"
echo "android16-framework-compat: PASS $libcore_compiler_out/core-libart-compiler.jar"
