#!/bin/bash
set -euo pipefail
export LC_ALL=C

dir="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$dir/../.." && pwd)"
# shellcheck disable=SC1090
source "$dir/sources.lock"

fail() { echo "android-managed-native-load: FAIL $*" >&2; exit 2; }
sha() { shasum -a 256 "$1" | awk '{print $1}'; }

mode="${1:-}"
case "$mode" in
  ""|--build-only) ;;
  *) fail "usage: $0 [--build-only]" ;;
esac

finish_audit() {
  xcrun clang-format --dry-run --Werror "$dir/fixture/managed_native.c" \
    "$dir/probes/runtime_registrar_smoke.cc"
  bash -n "$dir/audit.sh"
  git -C "$root" diff --check -- tools/android-managed-native-load
  git -C "$root" diff --cached --check -- tools/android-managed-native-load
  while IFS= read -r -d '' file; do
    set +e
    local whitespace
    whitespace="$(git -C "$root" diff --no-index --check /dev/null "$file" 2>&1)"
    local status=$?
    set -e
    [[ -z "$whitespace" ]] || fail "untracked whitespace: $file: $whitespace"
    [[ $status -le 1 ]] || fail "could not diff-check untracked file: $file"
  done < <(git -C "$root" ls-files --others --exclude-standard -z -- \
           tools/android-managed-native-load)
}

build="$root/_build/android-managed-native-load"
source_root="$build/source"
mkdir -p "$source_root"

materialize() {
  local project="$1" revision="$2" relative="$3" expected="$4"
  local destination="$source_root/$relative"
  local remote="$relative"
  if [[ "$project" == "$ART_PROJECT" && "$relative" == art/* ]]; then
    remote="${relative#art/}"
  fi
  if [[ ! -f "$destination" ]]; then
    mkdir -p "$(dirname "$destination")"
    local staged
    staged="$(mktemp "${TMPDIR:-/tmp}/managed-load-source.XXXXXX")"
    curl -fsSL \
      "https://android.googlesource.com/$project/+/$revision/$remote?format=TEXT" |
      base64 -D > "$staged"
    [[ "$(sha "$staged")" == "$expected" ]] ||
      fail "download hash mismatch: $relative"
    mv "$staged" "$destination"
  fi
  [[ "$(sha "$destination")" == "$expected" ]] ||
    fail "source hash mismatch: $relative"
}

materialize "$LIBCORE_PROJECT" "$LIBCORE_REVISION" \
  ojluni/src/main/java/java/lang/System.java "$SYSTEM_JAVA_SHA256"
materialize "$LIBCORE_PROJECT" "$LIBCORE_REVISION" \
  ojluni/src/main/java/java/lang/Runtime.java "$RUNTIME_JAVA_SHA256"
materialize "$LIBCORE_PROJECT" "$LIBCORE_REVISION" \
  ojluni/src/main/java/java/lang/ClassLoader.java "$CLASSLOADER_JAVA_SHA256"
materialize "$LIBCORE_PROJECT" "$LIBCORE_REVISION" \
  ojluni/src/main/native/Runtime.c "$RUNTIME_C_SHA256"
materialize "$LIBCORE_PROJECT" "$LIBCORE_REVISION" \
  ojluni/src/main/native/OnLoad.cpp "$ONLOAD_CPP_SHA256"
materialize "$LIBCORE_PROJECT" "$LIBCORE_REVISION" \
  ojluni/src/main/native/UnixNativeDispatcher.c \
  "$UNIX_NATIVE_DISPATCHER_C_SHA256"
for header_and_hash in \
  "jni_util.h:$JNI_UTIL_H_SHA256" \
  "jlong.h:$JLONG_H_SHA256" \
  "jlong_md.h:$JLONG_MD_H_SHA256" \
  "jvm.h:$JVM_H_SHA256" \
  "jvm_md.h:$JVM_MD_H_SHA256" \
  "classfile_constants.h:$CLASSFILE_CONSTANTS_H_SHA256"; do
  materialize "$LIBCORE_PROJECT" "$LIBCORE_REVISION" \
    "ojluni/src/main/native/${header_and_hash%%:*}" "${header_and_hash#*:}"
done
materialize "$ART_PROJECT" "$ART_REVISION" \
  art/openjdkjvm/OpenjdkJvm.cc "$OPENJDKJVM_CC_SHA256"
materialize "$ART_PROJECT" "$ART_REVISION" \
  runtime/jni/java_vm_ext.cc "$JAVA_VM_EXT_CC_SHA256"
materialize "$ART_PROJECT" "$ART_REVISION" \
  runtime/well_known_classes.cc "$WELL_KNOWN_CLASSES_CC_SHA256"
materialize "$ART_PROJECT" "$ART_REVISION" \
  runtime/jni/jni_internal.cc "$JNI_INTERNAL_CC_SHA256"

python3 - "$source_root" "$dir/manifests/native-owner.tsv" <<'PY'
import csv
import sys
import re
from pathlib import Path

root = Path(sys.argv[1])
manifest = Path(sys.argv[2])
system = (root / "ojluni/src/main/java/java/lang/System.java").read_text()
runtime = (root / "ojluni/src/main/java/java/lang/Runtime.java").read_text()
loader = (root / "ojluni/src/main/java/java/lang/ClassLoader.java").read_text()
runtime_c = (root / "ojluni/src/main/native/Runtime.c").read_text()
onload = (root / "ojluni/src/main/native/OnLoad.cpp").read_text()
unix = (root / "ojluni/src/main/native/UnixNativeDispatcher.c").read_text()
openjdk = (root / "art/openjdkjvm/OpenjdkJvm.cc").read_text()
vm = (root / "runtime/jni/java_vm_ext.cc").read_text()
well_known = (root / "runtime/well_known_classes.cc").read_text()
jni = (root / "runtime/jni/jni_internal.cc").read_text()

system_load = system[system.index("public static void load(String filename)") :]
assert system_load.index("Runtime.getRuntime().load0") < system_load.index("public static void loadLibrary")
system_library = system_load[system_load.index("public static void loadLibrary") :]
assert "Runtime.getRuntime().loadLibrary0(Reflection.getCallerClass(), libname)" in system_library

load0 = runtime[runtime.index("synchronized void load0") : runtime.index("public void loadLibrary")]
assert load0.index("file.isAbsolute()") < load0.index("nativeLoad(filename")
assert "fromClass.getClassLoader(), fromClass" in load0
load_library = runtime[runtime.index("private synchronized void loadLibrary0") :
                       runtime.index("private volatile String[] mLibPaths")]
assert load_library.index("loader.findLibrary") < load_library.index("nativeLoad(filename")
assert "loader.getClass() == PathClassLoader.class" in load_library
assert "System.mapLibraryName(libraryName)" in load_library

native_access = loader.index("// -- Native library access --")
removed = loader[loader.index("// Android-removed: Remove unused codes.", native_access) :
                 loader.index("// -- Assertion management --", native_access)]
assert "private final NativeLibraries libraries" in removed
assert removed.lstrip().startswith("// Android-removed: Remove unused codes.\n    /*")
assert "*//*\n    static long findNative" in removed

table = runtime_c[runtime_c.index("static JNINativeMethod gMethods[]") :
                  runtime_c.index("void register_java_lang_Runtime")]
expected = ["freeMemory", "totalMemory", "maxMemory", "nativeGc", "nativeExit", "nativeLoad"]
for method in expected:
    assert f"(Runtime, {method}," in table
assert table.count("_METHOD(Runtime,") == 6
assert "return JVM_NativeLoad(env, javaFilename, javaLoader, caller);" in runtime_c
assert onload.index("register_java_lang_System(env)") < onload.index("register_java_lang_Runtime(env)")
assert "Java_sun_nio_fs_UnixNativeDispatcher_init" in unix
assert "NATIVE_METHOD(Java_sun_nio_fs_UnixNativeDispatcher, init, \"()I\")" in unix
assert "void register_java_sun_nio_fs_UnixNativeDispatcher" in unix

native_load = openjdk[openjdk.index("JNIEXPORT jstring JVM_NativeLoad") :
                      openjdk.index("JNIEXPORT void JVM_StartThread")]
assert native_load.index("vm->LoadNativeLibrary") < native_load.index("if (success)")
load_vm = vm[vm.index("bool JavaVMExt::LoadNativeLibrary") :]
assert load_vm.index("OpenNativeLibrary") < load_vm.index('FindSymbol("JNI_OnLoad"')
assert '"nativeLoad"' in well_known and '"Ljava/lang/String;"' in well_known
assert "method == WellKnownClasses::java_lang_Runtime_nativeLoad" in jni
assert "GetClassLoaderOverride()" in jni

with manifest.open(newline="") as stream:
    rows = list(csv.DictReader(stream, delimiter="\t"))
assert len(rows) == 9
assert [int(row["order"]) for row in rows] == list(range(1, 10))
assert rows[2]["state_before_integration"] == "first-missing-native-init"
assert rows[3]["complete_owner"] == "libopenjdk Runtime.c complete six-method table"
assert rows[-1]["state_before_integration"] == "not-an-executable-path"
print("android-managed-native-load: source-control-flow=PASS owners=9 NativeLibraries=android-removed")
PY

core_oj="$root/_prebuilt/android-16/bootclasspath/core-oj.jar"
[[ -f "$core_oj" && "$(sha "$core_oj")" == "$CORE_OJ_JAR_SHA256" ]] ||
  fail 'pinned core-oj.jar missing or changed'
tmp="$(mktemp -d "${TMPDIR:-/tmp}/managed-native-load.XXXXXX")"
cleanup() {
  [[ "$tmp" == "${TMPDIR:-/tmp}"/managed-native-load.* ]] && find "$tmp" -depth -delete
}
trap cleanup EXIT
unzip -p "$core_oj" classes.dex > "$tmp/core-oj.dex"
[[ "$(sha "$tmp/core-oj.dex")" == "$CORE_OJ_DEX_SHA256" ]] ||
  fail 'prebuilt core-oj classes.dex changed'

sdk="${ANDROID_HOME:-$HOME/Library/Android/sdk}"
dexdump="$sdk/build-tools/35.0.0/dexdump"
d8="$sdk/build-tools/35.0.0/d8"
android_jar="$sdk/platforms/android-35/android.jar"
[[ -x "$dexdump" && -x "$d8" && -f "$android_jar" ]] ||
  fail 'Android 35 DEX toolchain missing'
"$dexdump" -d "$tmp/core-oj.dex" > "$tmp/core-oj.dump"
python3 - "$tmp/core-oj.dump" <<'PY'
import re
import sys
from pathlib import Path

text = Path(sys.argv[1]).read_text(errors="replace")
def class_chunk(descriptor):
    marker = f"Class descriptor  : '{descriptor}'"
    start = text.index(marker)
    end = text.find("Class #", start + len(marker))
    return text[start:] if end == -1 else text[start:end]

system = class_chunk("Ljava/lang/System;")
runtime = class_chunk("Ljava/lang/Runtime;")
loader = class_chunk("Ljava/lang/ClassLoader;")
assert "Ljava/lang/Runtime;.load0:(Ljava/lang/Class;Ljava/lang/String;)V" in system
assert "Ljava/lang/Runtime;.loadLibrary0:(Ljava/lang/Class;Ljava/lang/String;)V" in system
assert "Ljava/lang/Runtime;.nativeLoad:(Ljava/lang/String;Ljava/lang/ClassLoader;Ljava/lang/Class;)Ljava/lang/String;" in runtime
assert "invoke-virtual" in runtime and "Ljava/lang/ClassLoader;.findLibrary" in runtime
assert "Ljava/lang/System;.mapLibraryName" in runtime
assert "NativeLibraries" not in loader
native = re.search(
    r"name\s+: 'nativeLoad'\s+type\s+: "
    r"'\(Ljava/lang/String;Ljava/lang/ClassLoader;Ljava/lang/Class;\)Ljava/lang/String;'"
    r"\s+access\s+: [^\n]+NATIVE",
    runtime,
)
assert native is not None
print("android-managed-native-load: prebuilt-core-oj=PASS System+Runtime+ClassLoader bytecode")
PY

native_dir="$source_root/ojluni/src/main/native"
jni_home=/opt/homebrew/opt/openjdk@17/include
cc="$(xcrun --find clang)"
cxx="$(xcrun --find clang++)"
sdk_path="$(xcrun --sdk macosx --show-sdk-path)"
runtime_object="$build/Runtime.c.o"
mkdir -p "$build"
"$cc" -std=c17 -arch arm64 -isysroot "$sdk_path" -O2 -fPIC \
  -Wall -Wextra -Werror -Wno-unused-parameter \
  -I"$native_dir" -I"$jni_home" -I"$jni_home/darwin" \
  -I"$root/_aosp/libnativehelper-full/include" \
  -I"$root/_aosp/libnativehelper/include_jni" \
  -I"$root/_aosp/libnativehelper/header_only_include" \
  -I"$root/_aosp/libnativehelper/platform_header_only_include" \
  -I"$root/_aosp/system/logging/liblog/include" \
  -c "$native_dir/Runtime.c" -o "$runtime_object"
archive="$build/libopenjdk-runtime-managed-load-darwin.a"
libtool -static -o "$archive" "$runtime_object" >/dev/null
nm -gU "$runtime_object" | grep -F '_register_java_lang_Runtime' >/dev/null ||
  fail 'Runtime registrar export missing'
nm -gU "$runtime_object" | grep -F '_Runtime_nativeLoad' >/dev/null ||
  fail 'Runtime nativeLoad implementation missing'
actual_undefined="$(nm -u "$runtime_object" | awk '{print $NF}' | sort -u)"
expected_undefined="$(printf '%s\n' \
  _JVM_Exit _JVM_FreeMemory _JVM_GC _JVM_MaxMemory _JVM_NativeLoad \
  _JVM_TotalMemory _jniRegisterNativeMethods | sort -u)"
[[ "$actual_undefined" == "$expected_undefined" ]] || {
  diff -u <(printf '%s\n' "$expected_undefined") \
    <(printf '%s\n' "$actual_undefined") || true
  fail 'Runtime.c undefined owner set changed'
}

common=(-std=c++20 -arch arm64 -isysroot "$sdk_path" -Wall -Wextra -Werror
        -I"$jni_home" -I"$jni_home/darwin"
        "$dir/probes/runtime_registrar_smoke.cc" "$runtime_object")
for sanitizer in address undefined; do
  registrar="$tmp/runtime-registrar-$sanitizer"
  "$cxx" "${common[@]}" -O1 -g -fno-omit-frame-pointer \
    -fsanitize="$sanitizer" -o "$registrar"
  case "$sanitizer" in
    address) ASAN_OPTIONS=detect_leaks=0:halt_on_error=1 "$registrar" ;;
    undefined) UBSAN_OPTIONS=halt_on_error=1:print_stacktrace=1 "$registrar" ;;
  esac
done

fixture_classes="$tmp/classes"
fixture_dex_dir="$tmp/dex"
mkdir -p "$fixture_classes" "$fixture_dex_dir"
javac --release 8 -encoding UTF-8 -classpath "$android_jar" \
  -d "$fixture_classes" "$dir/fixture/ManagedNativeLoad.java" \
  "$dir/fixture/Hello.java"
baseline="$root/_build/dex-probe/classes"
baseline_classes=(
  "$baseline/android/test/mock/MockPackageManager.class"
  "$baseline/dev/aim/probe/ProbeActivity.class"
  "$baseline/dev/aim/probe/ProbeCanvas.class"
  "$baseline/dev/aim/probe/ProbeContentResolver.class"
  "$baseline/dev/aim/probe/ProbeContentRoot.class"
  "$baseline/dev/aim/probe/ProbeContext.class"
  "$baseline/dev/aim/probe/ProbePackageManager.class"
  "$baseline/dev/aim/probe/ProbeResources.class"
  "$baseline/dev/aim/probe/ProbeView.class"
  "$baseline/dev/aim/probe/ProbeXmlResourceParser.class"
)
for input in "${baseline_classes[@]}"; do
  [[ -f "$input" ]] || fail "baseline app class missing: $input"
done
"$d8" --lib "$android_jar" --classpath "$baseline" --output "$fixture_dex_dir" \
  "${baseline_classes[@]}" \
  "$fixture_classes/dev/aim/probe/Hello.class" \
  "$fixture_classes/dev/aim/managedload/ManagedNativeLoad.class"
fixture_dex="$fixture_dex_dir/classes.dex"
cp "$fixture_dex" "$build/classes.dex"
"$dexdump" -d "$fixture_dex" > "$tmp/fixture.dump"
python3 - "$tmp/fixture.dump" <<'PY'
import sys
from pathlib import Path

text = Path(sys.argv[1]).read_text(errors="replace")
assert "Class descriptor  : 'Ldev/aim/managedload/ManagedNativeLoad;'" in text
assert "Ljava/lang/System;.load:(Ljava/lang/String;)V" in text
assert "Ldev/aim/managedload/ManagedNativeLoad;.nativeProbe:()I" in text
assert "AIM_MANAGED_NATIVE_FIXTURE" in text
assert "Class descriptor  : 'Ldev/aim/probe/Hello;'" in text
print("android-managed-native-load: app-dex=PASS actual-DEX System.load+nativeProbe")
PY

ndk="${ANDROID_NDK_ROOT:-$sdk/ndk/$NDK_REVISION}"
[[ -f "$ndk/source.properties" && \
   "$(sha "$ndk/source.properties")" == "$NDK_SOURCE_PROPERTIES_SHA256" ]] ||
  fail 'pinned NDK missing or changed'
toolchain="$ndk/toolchains/llvm/prebuilt/darwin-x86_64/bin"
android_cc="$toolchain/aarch64-linux-android${ANDROID_API}-clang"
readelf="$toolchain/llvm-readelf"
fixture_so="$build/libmanaged-native-load.so"
"$android_cc" -std=c17 -O2 -fPIC -fno-stack-protector \
  -Wall -Wextra -Werror -shared -nostdlib -fuse-ld=lld \
  -Wl,--build-id=none -Wl,--hash-style=sysv -Wl,-z,now -Wl,-z,norelro \
  -Wl,-z,max-page-size=16384 -Wl,-soname,libmanaged-native-load.so \
  -Wl,--version-script,"$dir/fixture/exports.map" \
  "$dir/fixture/managed_native.c" -o "$fixture_so"
"$readelf" -h "$fixture_so" | grep -F 'Machine:                           AArch64' >/dev/null ||
  fail 'managed fixture is not Android AArch64 ELF'
"$readelf" --dyn-syms --wide "$fixture_so" | grep -F ' JNI_OnLoad@@MANAGED_NATIVE_LOAD' >/dev/null ||
  fail 'JNI_OnLoad export missing'
"$readelf" --dyn-syms --wide "$fixture_so" |
  grep -F ' Java_dev_aim_managedload_ManagedNativeLoad_nativeProbe@@MANAGED_NATIVE_LOAD' \
    >/dev/null || fail 'named JNI export missing'
if "$readelf" -d "$fixture_so" | grep -F '(NEEDED)' >/dev/null; then
  fail 'managed fixture unexpectedly has DT_NEEDED dependencies'
fi

if [[ "$mode" == "--build-only" ]]; then
  finish_audit
  echo 'android-managed-native-load: BUILD-ONLY PASS Runtime6 archive + actual DEX + Android arm64 ELF'
  exit 0
fi

runtime="$root/_build/runtime-link-probe/libaim_runtime.dylib"
host="$root/target/debug/aim-host"
core_libart="$root/_prebuilt/android-16/bootclasspath/core-libart.jar"
framework="$root/_prebuilt/android-16/bootclasspath/framework.jar"
icu="$root/_build/bootclasspath/core-icu4j.jar"
for input in "$runtime" "$host" "$core_libart" "$framework" "$icu"; do
  [[ -f "$input" ]] || fail "actual ART prerequisite missing: $input"
done
set +e
AIM_MANAGED_NATIVE_FIXTURE="$fixture_so" \
  "$host" "$runtime" "$core_oj" "$core_libart" "$framework" "$icu" \
  "$fixture_dex" > "$tmp/art.stdout" 2> "$tmp/art.stderr"
art_status=$?
set -e
runtime_ready=0
unix_ready=0
nm -gU "$runtime" | grep -F '_register_java_lang_Runtime' >/dev/null && runtime_ready=1
nm -gU "$runtime" | grep -F '_register_java_sun_nio_fs_UnixNativeDispatcher' \
  >/dev/null && unix_ready=1
if [[ $runtime_ready -eq 1 && $unix_ready -eq 1 ]]; then
  [[ $art_status -eq 0 ]] || {
    sed -n '1,160p' "$tmp/art.stderr" >&2
    fail 'managed System.load ART path failed after Runtime registrar integration'
  }
  grep -F 'ART Darwin launcher: main(String[])=ok' "$tmp/art.stdout" >/dev/null ||
    fail 'managed ART launcher completion missing'
  echo 'android-managed-native-load: PASS actual-ART=System.load PathClassLoader=single path=single nativeProbe=42 direct-JavaVMExt=0'
else
  [[ $art_status -ne 0 ]] || fail 'managed System.load unexpectedly succeeded without Runtime registrar'
  if [[ $unix_ready -eq 0 ]]; then
    grep -F 'sun.nio.fs.UnixNativeDispatcher.init' "$tmp/art.stderr" >/dev/null ||
      fail 'first missing UnixNativeDispatcher.init failure was not observed'
    grep -F 'java.lang.Runtime.load0' "$tmp/art.stderr" >/dev/null ||
      fail 'managed Runtime.load0 stack evidence missing'
    echo 'android-managed-native-load: BLOCKED actual-ART first-missing=UnixNativeDispatcher.init Runtime-registrar-archive=ready'
  else
    grep -F 'java.lang.Runtime.nativeLoad' "$tmp/art.stderr" >/dev/null ||
      fail 'first missing Runtime.nativeLoad failure was not observed'
    echo 'android-managed-native-load: BLOCKED actual-ART first-missing=register_java_lang_Runtime/Runtime.nativeLoad archive=ready'
  fi
fi

finish_audit
