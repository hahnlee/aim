#!/bin/bash
set -euo pipefail
export LC_ALL=C

project_root="$(cd "$(dirname "$0")/.." && pwd)"
source "$project_root/upstream/android16-register-natives-bridge.lock"
source_root="$project_root/_aosp/art-register-natives-bridge"
bridge_root="$project_root/tools/android-register-natives-bridge"
fixture_root="$project_root/probes/android-elf-jni-fixture"
fixture_elf="$project_root/_build/android-elf-jni-fixture/libdarwin-art-jni-fixture.so"
fixture_child="$project_root/_build/android-elf-jni-fixture/libdarwin-art-jni-child.so"
fixture_grandchild="$project_root/_build/android-elf-jni-fixture/libdarwin-art-jni-grandchild.so"
generic_root="$project_root/_build/android-elf-jni-fixture/libdarwin-art-generic-root.so"
generic_child="$project_root/_build/android-elf-jni-fixture/libdarwin-art-generic-child.so"
generic_grandchild="$project_root/_build/android-elf-jni-fixture/libdarwin-art-generic-grandchild.so"
build_dir="$project_root/_build/register-natives-bridge"

fail() { echo "register-natives-bridge: $*" >&2; exit 3; }
missing() { echo "register-natives-bridge: missing $*" >&2; exit 2; }
sha() { shasum -a 256 "$1" | awk '{print $1}'; }

files=(
  runtime/jni/jni_internal.cc
  runtime/class_linker.cc
  runtime/entrypoints/jni/jni_entrypoints.cc
  libnativebridge/native_bridge.cc
  libnativebridge/include/nativebridge/native_bridge.h
)
hashes=(
  "$JNI_INTERNAL_CC_SHA256"
  "$CLASS_LINKER_CC_SHA256"
  "$JNI_ENTRYPOINTS_CC_SHA256"
  "$NATIVE_BRIDGE_CC_SHA256"
  "$NATIVE_BRIDGE_H_SHA256"
)
local_candidates=(
  "$project_root/_aosp/art/runtime/jni/jni_internal.cc"
  "$project_root/_aosp/art/runtime/class_linker.cc"
  "$project_root/_aosp/art/runtime/entrypoints/jni/jni_entrypoints.cc"
  "$project_root/_aosp/art-native-library-control-flow/libnativebridge/native_bridge.cc"
  "$project_root/_aosp/art/libnativebridge/include/nativebridge/native_bridge.h"
)
[[ "${#files[@]}" == "$LOCKED_ART_SOURCE_COUNT" ]] ||
  fail "locked ART source count mismatch"

for index in "${!files[@]}"; do
  relative="${files[$index]}"
  expected="${hashes[$index]}"
  destination="$source_root/$relative"
  if [[ ! -f "$destination" ]]; then
    mkdir -p "$(dirname "$destination")"
    candidate="${local_candidates[$index]}"
    if [[ -f "$candidate" && "$(sha "$candidate")" == "$expected" ]]; then
      cp "$candidate" "$destination"
    else
      staged="$(mktemp "${TMPDIR:-/tmp}/art-register-natives.XXXXXX")"
      curl -fsSL "https://android.googlesource.com/$ART_PROJECT/+/$ART_REVISION/$relative?format=TEXT" |
        base64 -D > "$staged"
      [[ "$(sha "$staged")" == "$expected" ]] ||
        fail "download SHA mismatch: $relative"
      mv "$staged" "$destination"
    fi
  fi
  [[ "$(sha "$destination")" == "$expected" ]] ||
    fail "source SHA mismatch: $relative"
done
printf '%s\n' "$ART_REVISION" > "$source_root/.source-revision"
[[ -z "$(find "$source_root" \( -name .git -o -name .gitmodules \) -print -quit)" ]] ||
  fail "Git metadata is forbidden in sparse source"

python3 - "$source_root" <<'PY'
import sys
from pathlib import Path

root = Path(sys.argv[1])
jni = (root / "runtime/jni/jni_internal.cc").read_text()
linker = (root / "runtime/class_linker.cc").read_text()
entry = (root / "runtime/entrypoints/jni/jni_entrypoints.cc").read_text()
bridge = (root / "libnativebridge/native_bridge.cc").read_text()
header = (root / "libnativebridge/include/nativebridge/native_bridge.h").read_text()

register = jni[jni.index("static jint RegisterNatives("):
               jni.index("static jint UnregisterNatives(")]
assert register.index("const void* fnPtr = methods[i].fnPtr") < register.index("FindMethod<true>")
assert register.index("is_class_loader_namespace_natively_bridged ||") < register.index(
    "android::NativeBridgeIsNativeBridgeFunctionPointer(fnPtr)")
assert register.index("GenerateNativeBridgeTrampoline(fnPtr, m)") < register.index(
    "class_linker->RegisterNative(soa.Self(), m, fnPtr)")

unregister = jni[jni.index("static jint UnregisterNatives("):
                 jni.index("static jint MonitorEnter(")]
assert "class_linker->UnregisterNative(soa.Self(), &m)" in unregister
generate = jni[jni.index("static bool IsClassLoaderNamespaceNativelyBridged"):
               jni.index("template <typename ArrayT", jni.index(
                   "static const void* GenerateNativeBridgeTrampoline"))]
assert generate.count("#if defined(ART_TARGET_ANDROID)") == 2
assert "return false;" in generate and "return fn_ptr;" in generate
assert "method->GetShorty(&shorty_length)" in generate
assert "method->IsCriticalNative()" in generate
assert "NativeBridgeGetTrampolineForFunctionPointer(" in generate

class_register = linker[linker.index("const void* ClassLinker::RegisterNative"):
                        linker.index("void ClassLinker::UnregisterNative")]
assert class_register.index("CHECK(native_method != nullptr)") < class_register.index(
    "RegisterNativeMethod(method")
assert class_register.index("RegisterNativeMethod(method") < class_register.index(
    "method->SetEntryPointFromJni(new_native_method)")
class_unregister = linker[linker.index("void ClassLinker::UnregisterNative"):
                          linker.index("const void* ClassLinker::GetRegisteredNative")]
assert "GetJniDlsymLookupCriticalStub" in class_unregister
assert "GetJniDlsymLookupStub" in class_unregister

lazy_start = entry.index("const void* artFindNativeMethodRunnable")
lazy = entry[lazy_start:entry.index(
    "extern \"C\" const void* artFindNativeMethod(Thread*", lazy_start)]
assert lazy.index("FindCodeForNativeMethod") < lazy.index(
    "class_linker->RegisterNative(self, method, native_code)")

fn_bridge = bridge[bridge.index("void* NativeBridgeGetTrampolineForFunctionPointer"):
                   bridge.index("bool NativeBridgeIsSupported")]
assert "CRITICAL_NATIVE_SUPPORT_VERSION = 7" in bridge
assert "IDENTIFY_NATIVELY_BRIDGED_FUNCTION_POINTERS_VERSION = 8" in bridge
assert "isCompatibleWith(CRITICAL_NATIVE_SUPPORT_VERSION)" in fn_bridge
assert "callbacks->getTrampolineForFunctionPointer(method, shorty, len, jni_call_type)" in fn_bridge
owner = bridge[bridge.index("bool NativeBridgeIsNativeBridgeFunctionPointer") :]
assert "isCompatibleWith(IDENTIFY_NATIVELY_BRIDGED_FUNCTION_POINTERS_VERSION)" in owner
assert "callbacks->isNativeBridgeFunctionPointer(method)" in owner
assert "kJNICallTypeRegular = 1" in header
assert "kJNICallTypeCriticalNative = 2" in header
assert header.index("getTrampolineForFunctionPointer") < header.index(
    "isNativeBridgeFunctionPointer")
print("register-natives-bridge: upstream-flow=PASS darwin-current=raw-pointer-bypass")
PY

python3 - "$project_root" <<'PY'
import sys
from pathlib import Path

root = Path(sys.argv[1])
adapter = (root / "compat/darwin_runtime_native_loader.cc").read_text()
lifecycle = (root / "compat/darwin_runtime_elf_lifecycle.cc").read_text()
registration = (root / "compat/darwin_runtime_jni_registration.cc").read_text()
resolver = (root / "compat/darwin_runtime_elf_resolver.cc").read_text()
probe = (root / "probes/runtime_elf_probe.cc").read_text()
open_start = adapter.index("void* OpenNativeLibrary(")
discovery_start = adapter.index("darwin_art_elf_discover_sibling_graph(", open_start)
elf_start = adapter.index("if (discovery_status == DARWIN_ART_ELF_OK) {", discovery_start)
host_start = adapter.index("void* handle =", elf_start)
elf = adapter[elf_start:host_start]
assert discovery_start < adapter.index("std::make_unique<ElfLibrary>()", elf_start)
open_setup = adapter[open_start:discovery_start]
for flag in ("O_DIRECTORY", "O_CLOEXEC", "O_NOFOLLOW"):
    assert flag in open_setup
assert "darwin_art_elf_graph_load_with_lifecycle(" in elf
assert "darwin_art_elf_graph_lookup_root(" in adapter + resolver
assert "darwin_art_elf_graph_unload(" in lifecycle
assert "darwin_art_bionic_namespace_bind_builtins(" in elf
assert "darwin_art_bionic_namespace_seal(" in elf
assert "darwin_art_bionic_namespace_teardown(" in lifecycle
assert "darwin_art_elf_load_bytes(" not in adapter
assert "dlopen(" not in elf and "dlsym(" not in elf
assert elf.index("darwin_art_bionic_namespace_seal(") < elf.index(
    "darwin_art_elf_graph_load_with_lifecycle(")
assert elf.index("darwin_art_elf_graph_load_with_lifecycle(") < elf.index(
    "darwin_art_jni_proxy_init(")
assert elf.index("darwin_art_elf_graph_load_with_lifecycle(") < elf.index(
    "*needs_native_bridge = true")
assert "DestroyRegularTrampolines" in lifecycle
assert "TeardownProviderNamespace(library)" in lifecycle
assert "darwin_art_bionic_namespace_teardown(" in lifecycle
for source in (adapter, lifecycle, registration, resolver):
    for forbidden in ("fixture_graph", "IsExactFixtureGraph", "g_elf_fixture",
                      "kDarwinArtElfJniHostProviderSoname",
                      "darwin_art_fixture_record_lifecycle"):
        assert forbidden not in source, forbidden
assert '"libm.so"' in adapter[open_start:elf_start]
assert 'ReadJournal() == "124567"' in probe
assert 'ReadJournal() != "123"' in probe
assert "darwin_art_image_registry::ContainsAddress(" in registration
assert "RegisterNatives=generic+fixture" in probe
print("register-natives-bridge: local-graph-flow=PASS providers=sealed-before-graph teardown=graph-before-namespace publish=after-complete no-dyld=ELF")
PY

[[ "$(sha "$fixture_root/native_fixture.c")" == "$FIXTURE_C_SHA256" ]] ||
  fail "fixture C source SHA mismatch"
[[ "$(sha "$fixture_root/child.c")" == "$FIXTURE_CHILD_C_SHA256" ]] ||
  fail "fixture child C source SHA mismatch"
[[ "$(sha "$fixture_root/grandchild.c")" == "$FIXTURE_GRANDCHILD_C_SHA256" ]] ||
  fail "fixture grandchild C source SHA mismatch"
[[ "$(sha "$fixture_root/host_provider.c")" == "$FIXTURE_HOST_PROVIDER_C_SHA256" ]] ||
  fail "fixture host-provider C source SHA mismatch"
[[ "$(sha "$fixture_root/child.exports.map")" == "$FIXTURE_CHILD_MAP_SHA256" ]] ||
  fail "fixture child export map SHA mismatch"
[[ "$(sha "$fixture_root/grandchild.exports.map")" == "$FIXTURE_GRANDCHILD_MAP_SHA256" ]] ||
  fail "fixture grandchild export map SHA mismatch"
for entry in \
  "generic_root.c:$GENERIC_ROOT_C_SHA256" \
  "generic_child.c:$GENERIC_CHILD_C_SHA256" \
  "generic_grandchild.c:$GENERIC_GRANDCHILD_C_SHA256" \
  "generic_root.exports.map:$GENERIC_ROOT_MAP_SHA256" \
  "generic_child.exports.map:$GENERIC_CHILD_MAP_SHA256" \
  "generic_grandchild.exports.map:$GENERIC_GRANDCHILD_MAP_SHA256"; do
  file="${entry%%:*}"
  expected="${entry#*:}"
  [[ "$(sha "$fixture_root/$file")" == "$expected" ]] ||
    fail "generic graph source SHA mismatch: $file"
done
grep -F '(*env)->RegisterNatives(env, fixture, &method, 1)' \
  "$fixture_root/generic_root.c" >/dev/null ||
  fail "generic graph no longer exercises proxy RegisterNatives"
for operation in NewStringUTF GetStringUTFLength GetStringUTFChars \
  ReleaseStringUTFChars NewGlobalRef DeleteGlobalRef DeleteLocalRef \
  NewLocalRef NewByteArray GetArrayLength GetByteArrayRegion \
  SetByteArrayRegion ExceptionOccurred ExceptionClear ExceptionCheck; do
  grep -F "(*env)->$operation" "$fixture_root/generic_root.c" >/dev/null ||
    fail "generic native no longer exercises proxy $operation"
done
grep -E 'foreign\.fnPtr = .*uintptr_t\)1' \
  "$fixture_root/generic_root.c" >/dev/null ||
  fail "generic graph no longer rejects a foreign native address"
[[ "$(sha "$fixture_root/NativeFixture.java")" == "$FIXTURE_JAVA_SHA256" ]] ||
  fail "fixture Java source SHA mismatch"
grep -F "{\"nativeAdd\", \"$FIXTURE_ADD_DESCRIPTOR\"" "$fixture_root/native_fixture.c" >/dev/null ||
  fail "nativeAdd descriptor mismatch"
grep -F "\"$FIXTURE_SPILL_DESCRIPTOR\"" "$fixture_root/native_fixture.c" >/dev/null ||
  fail "nativeSpill descriptor mismatch"

ANDROID_SDK_ROOT="${ANDROID_SDK_ROOT:-$HOME/Library/Android/sdk}" \
  "$project_root/tools/build-android-elf-jni-fixture.sh" >/dev/null
[[ -f "$fixture_elf" ]] || missing "$fixture_elf"
[[ "$(sha "$fixture_elf")" == "$FIXTURE_ELF_SHA256" ]] ||
  fail "fixture ELF SHA mismatch"
[[ -f "$fixture_child" && "$(sha "$fixture_child")" == "$FIXTURE_CHILD_ELF_SHA256" ]] ||
  fail "fixture child ELF SHA mismatch"
[[ -f "$fixture_grandchild" && "$(sha "$fixture_grandchild")" == "$FIXTURE_GRANDCHILD_ELF_SHA256" ]] ||
  fail "fixture grandchild ELF SHA mismatch"
[[ -f "$generic_root" && "$(sha "$generic_root")" == "$GENERIC_ROOT_ELF_SHA256" ]] ||
  fail "generic root ELF SHA mismatch"
[[ -f "$generic_child" && "$(sha "$generic_child")" == "$GENERIC_CHILD_ELF_SHA256" ]] ||
  fail "generic child ELF SHA mismatch"
[[ -f "$generic_grandchild" && "$(sha "$generic_grandchild")" == "$GENERIC_GRANDCHILD_ELF_SHA256" ]] ||
  fail "generic grandchild ELF SHA mismatch"

ndk="${ANDROID_SDK_ROOT:-$HOME/Library/Android/sdk}/ndk/$NDK_REVISION"
toolchain="$ndk/toolchains/llvm/prebuilt/darwin-x86_64/bin"
readelf="$toolchain/llvm-readelf"
elf_nm="$toolchain/llvm-nm"
[[ -x "$readelf" ]] || missing "$readelf"
[[ -x "$elf_nm" ]] || missing "$elf_nm"

add_value="0x$($elf_nm -n "$fixture_elf" | awk '$3 == "NativeAdd" {print $1}')"
spill_value="0x$($elf_nm -n "$fixture_elf" | awk '$3 == "NativeSpill" {print $1}')"
[[ "$((add_value))" == "$((FIXTURE_NATIVE_ADD))" ]] || fail "NativeAdd ELF value mismatch"
[[ "$((spill_value))" == "$((FIXTURE_NATIVE_SPILL))" ]] || fail "NativeSpill ELF value mismatch"
read -r exec_offset exec_begin exec_file_size exec_memory_size exec_alignment < <(
  "$readelf" -lW "$fixture_elf" |
    awk '$1 == "LOAD" && $7 == "R" && $8 == "E" { print $2, $3, $5, $6, $9 }'
)
[[ "$exec_offset" == "$FIXTURE_EXEC_OFFSET" &&
   "$((exec_begin))" == "$((FIXTURE_EXEC_BEGIN))" &&
   "$exec_file_size" == "$exec_memory_size" &&
   "$exec_alignment" == 0x4000 &&
   "$((exec_begin + exec_memory_size))" == "$((FIXTURE_EXEC_END))" ]] ||
  fail "fixture executable PT_LOAD mismatch"
root_dynamic="$("$readelf" -d "$fixture_elf")"
child_dynamic="$("$readelf" -d "$fixture_child")"
grandchild_dynamic="$("$readelf" -d "$fixture_grandchild")"
[[ "$(grep -c '(NEEDED)' <<< "$root_dynamic")" == 3 ]] ||
  fail "fixture root dependency count drift"
grep -E '\(NEEDED\).*libdarwin-art-jni-child\.so' <<< "$root_dynamic" >/dev/null ||
  fail "fixture root child dependency drift"
grep -E '\(NEEDED\).*libdarwin-art-jni-host\.so' <<< "$root_dynamic" >/dev/null ||
  fail "fixture root virtual-provider dependency drift"
grep -E '\(NEEDED\).*libc\.so' <<< "$root_dynamic" >/dev/null ||
  fail "fixture root Bionic provider dependency drift"
[[ "$(grep -c '(NEEDED)' <<< "$child_dynamic")" == 3 ]] ||
  fail "fixture child dependency count drift"
grep -E '\(NEEDED\).*libdarwin-art-jni-grandchild\.so' <<< "$child_dynamic" >/dev/null ||
  fail "fixture child grandchild dependency drift"
grep -E '\(NEEDED\).*libdarwin-art-jni-host\.so' <<< "$child_dynamic" >/dev/null ||
  fail "fixture child virtual-provider dependency drift"
grep -E '\(NEEDED\).*libc\.so' <<< "$child_dynamic" >/dev/null ||
  fail "fixture child Bionic lifecycle dependency drift"
[[ "$(grep -c '(NEEDED)' <<< "$grandchild_dynamic" || true)" == 0 ]] ||
  fail "fixture grandchild dependency count drift"
for elf in "$fixture_elf" "$fixture_child"; do
  "$readelf" -rW "$elf" |
    grep -E 'R_AARCH64_JUMP_SLOT.*__cxa_atexit' >/dev/null ||
    fail "fixture per-DSO lifecycle import drift"
done

stage="$(mktemp -d "${TMPDIR:-/tmp}/register-natives-bridge.XXXXXX")"
trap 'rm -rf "$stage"' EXIT
cxx="$(xcrun --find clang++)"
ar="$(xcrun --find ar)"
host_nm="$(xcrun --find nm)"
macos_sdk="$(xcrun --sdk macosx --show-sdk-path)"
common_flags=(-std=c++20 -arch arm64 -isysroot "$macos_sdk" -O2 -Wall -Wextra -Werror
              -I"$bridge_root/include")
"$cxx" "${common_flags[@]}" -c "$bridge_root/registered_native_bridge.cc" \
  -o "$stage/registered_native_bridge.o"
"$ar" rcs "$stage/libdarwin-art-registered-native-bridge.a" \
  "$stage/registered_native_bridge.o"
"$cxx" "${common_flags[@]}" \
  "$bridge_root/registered_native_bridge_smoke.cc" \
  "$stage/libdarwin-art-registered-native-bridge.a" \
  -o "$stage/registered-native-bridge-smoke"

"$cxx" "${common_flags[@]}" -O1 -g -fsanitize=address,undefined \
  "$bridge_root/registered_native_bridge.cc" \
  "$bridge_root/registered_native_lifetime_test.cc" \
  -o "$stage/registered-native-lifetime-test"
"$stage/registered-native-lifetime-test"

file "$stage/registered_native_bridge.o" | grep -F 'Mach-O 64-bit object arm64' >/dev/null ||
  fail "bridge object is not Darwin arm64"
[[ "$($ar -t "$stage/libdarwin-art-registered-native-bridge.a" | grep -c '\.o$' | tr -d ' ')" == 1 ]] ||
  fail "bridge archive member count mismatch"
for symbol in \
  _darwin_art_is_android_function_pointer \
  _darwin_art_get_registered_native_trampoline \
  _darwin_art_resolve_registered_native \
  _darwin_art_registered_native_cache_retire_image; do
  "$host_nm" -gU "$stage/libdarwin-art-registered-native-bridge.a" |
    awk '{print $3}' | grep -Fx "$symbol" >/dev/null || fail "missing C ABI symbol $symbol"
done
if "$host_nm" -u "$stage/registered-native-bridge-smoke" |
    grep -E '_(dlopen|dlsym|dlclose)$' >/dev/null; then
  fail "bridge smoke accidentally depends on Darwin global loader lookup"
fi

output="$("$stage/registered-native-bridge-smoke" \
  "$FIXTURE_EXEC_BEGIN" "$FIXTURE_EXEC_END" \
  "$FIXTURE_NATIVE_ADD" "$FIXTURE_NATIVE_SPILL")"
grep -F "fixture-shorty add=$FIXTURE_ADD_SHORTY spill=$FIXTURE_SPILL_SHORTY" <<< "$output" >/dev/null ||
  fail "fixture shorty smoke failed"
grep -F "callable add=105 spill=$FIXTURE_SPILL_DIGEST regular=$JNI_CALL_REGULAR critical=$JNI_CALL_CRITICAL_NATIVE" <<< "$output" >/dev/null ||
  fail "Darwin-callable thunk smoke failed"
grep -F 'lifecycle=retire-on-image-close retired=3 generation-reuse=fresh' <<< "$output" >/dev/null ||
  fail "cache lifecycle smoke failed"

mkdir -p "$build_dir"
cp "$stage/libdarwin-art-registered-native-bridge.a" "$build_dir/"
cp "$stage/registered-native-bridge-smoke" "$build_dir/"
printf '%s\n' "$output"
echo "register-natives-bridge: PASS ART=$ART_TAG bridge=v$NATIVE_BRIDGE_POINTER_OWNER_VERSION/v$NATIVE_BRIDGE_CRITICAL_CALL_VERSION ART-graph+Bionic-providers=integrated"
