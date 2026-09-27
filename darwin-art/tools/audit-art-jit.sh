#!/usr/bin/env bash
# ART JIT/compiler acceptance: runs the JIT fixtures (probes/runtime_entry_probe.cc,
# linked against the product runtime as the graphics fixture client) through
# darwin-art-fixture-runner. It needs the built product dylib and the probe
# DEX outputs (build-dex, build-button-dex), so it is not one of the
# tools/tests standalone scripts; run it after `cargo xtask build`.
set -euo pipefail
jit_root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$jit_root"
python3 "$jit_root/tools/audit-art-arm64-intrinsics.py" --root "$jit_root"
# Use the same compatibility boot class path as installed applications.
jit_tail="$jit_root/_prebuilt/android-16/bootclasspath/framework-location.jar"
for module in conscrypt/javalib/conscrypt.jar bt/javalib/framework-bluetooth.jar mediaprovider/javalib/framework-mediaprovider.jar permission/javalib/framework-permission.jar permission/javalib/framework-permission-s.jar art/javalib/okhttp.jar; do
  jit_tail="$jit_tail:$jit_root/_build/android16-ps16k-r07/extracted/$module"
done
jit_tail="$jit_tail:$jit_root/_build/bootclasspath/core-icu4j-api36.jar"
# JitUnsafe is a compiler acceptance fixture, not application payload. Direct
# references to the hidden JDK Unsafe API are only verifier-valid from a
# trusted boot class, matching upstream ART compiler tests.
jit_unsafe_boot="$jit_root/_build/dex-probe/unsafe-boot-dex/unsafe-boot.jar"
test -f "$jit_unsafe_boot"
jit_tail="$jit_tail:$jit_unsafe_boot"
# Do not force the production setting here: Android application processes must
# enter ART with JIT enabled by default. The acceptance run therefore doubles
# as a regression gate for the no-override launcher contract.
unset DARWIN_ART_JIT
export DARWIN_ART_JIT_ACCEPTANCE_ONLY=1
export DARWIN_ART_TEST_SURFACE_LOCK_CANVAS=1
export DARWIN_ART_TEST_MEDIA_CODEC_SURFACE=1
export ANDROID_I18N_ROOT="$jit_root/_build/icu-runtime-adapters/runtime/i18n"
export ANDROID_DATA="$jit_root/_build/icu-runtime-adapters/runtime/data"
export ANDROID_TZDATA_ROOT="$jit_root/_build/icu-runtime-adapters/runtime/tzdata"
export DARWIN_ART_TEST_FONTS_XML="$jit_root/probes/button/fonts.xml"
export DARWIN_ART_TEST_FONT="$jit_root/_aosp/external/skia/resources/fonts/Roboto-Regular.ttf"
export DARWIN_ART_FRAMEWORK_RES_APK="$jit_root/_prebuilt/android-16/resources/framework-res.apk"
export DARWIN_ART_APK_APP_RESOURCE_APK="$DARWIN_ART_FRAMEWORK_RES_APK"
export DARWIN_ART_RUNTIME_HOST_FILES="$jit_root/_build/android16-core-oj-compat/core-oj-compat.jar:$jit_root/_prebuilt/android-16/bootclasspath/core-libart.jar:$jit_root/_build/android16-framework-compat/framework-compat.jar:$jit_tail:$jit_root/_build/button-dex/dex/classes.dex"
audit_log="$(mktemp "${TMPDIR:-/tmp}/darwin-art-jit-audit.XXXXXX.log")"
trap 'rm -f "$audit_log"' EXIT
# The JIT fixtures are test-client code (probes/runtime_entry_probe.cc),
# linked against the product runtime as the graphics fixture client.
cargo run -q -p art-bootstrap --release -- build-runtime-graphics-fixture-client >/dev/null
if ! target/debug/darwin-art-fixture-runner \
  --fixture-image "$jit_root/_build/native-fixtures/graphics/libdarwin_art_fixture.dylib" \
  --window-seconds 3 \
  "$jit_root/_build/runtime-graphics-link-probe/libdarwin_art_runtime_graphics.dylib" \
  "$jit_root/_build/android16-core-oj-compat/core-oj-compat.jar" \
  "$jit_root/_prebuilt/android-16/bootclasspath/core-libart.jar" \
  "$jit_root/_build/android16-framework-compat/framework-compat.jar" \
  "$jit_tail" "$jit_root/_build/button-dex/dex/classes.dex" 2>&1 | tee "$audit_log"
then
  echo "ART JIT audit: runtime failed; log=$audit_log" >&2
  exit 1
fi
if ! rg -a -q 'ART empty checkpoint mutex contention PASS checkpoint_us=' "$audit_log"; then
  echo "ART JIT audit: empty-checkpoint contention fixture did not execute; log=$audit_log" >&2
  exit 1
fi
if ! rg -a -q 'ART Nterp acceptance: AOSP admission and native interpreter execution PASS' "$audit_log"; then
  echo "ART JIT audit: Nterp admission/execution fixture did not execute; log=$audit_log" >&2
  exit 1
fi
if ! rg -a -q 'ART Android Surface: Java lockCanvas/unlockCanvasAndPost PASS' "$audit_log"; then
  echo "ART JIT audit: Surface software-Canvas fixture did not execute; log=$audit_log" >&2
  exit 1
fi
if ! rg -a -q 'ART Android MediaCodec: setOutputSurface producer lifetime PASS' "$audit_log"; then
  echo "ART JIT audit: MediaCodec output-surface fixture did not execute; log=$audit_log" >&2
  exit 1
fi
