#!/bin/bash
set -euo pipefail
export LC_ALL=C

project_root="$(cd "$(dirname "$0")/.." && pwd)"
source "$project_root/upstream/android16-native-library-control-flow.lock"
source_root="$project_root/_aosp/art-native-library-control-flow"

fail() { echo "native-library-control-flow: $*" >&2; exit 3; }
sha() { shasum -a 256 "$1" | awk '{print $1}'; }

files=(
  libnativeloader/native_loader.cpp
  libnativeloader/native_loader_namespace.cpp
  libnativebridge/native_bridge.cc
  runtime/jni/java_vm_ext.cc
  libnativebridge/include/nativebridge/native_bridge.h
  libnativeloader/include/nativeloader/native_loader.h
)
hashes=(
  "$NATIVE_LOADER_CPP_SHA256"
  "$NATIVE_LOADER_NAMESPACE_CPP_SHA256"
  "$NATIVE_BRIDGE_CPP_SHA256"
  "$JAVA_VM_EXT_CPP_SHA256"
  "$NATIVE_BRIDGE_H_SHA256"
  "$NATIVE_LOADER_H_SHA256"
)
[[ "${#files[@]}" == "$LOCKED_SOURCE_COUNT" ]] || fail "locked source count mismatch"

for index in "${!files[@]}"; do
  relative="${files[$index]}"
  expected="${hashes[$index]}"
  destination="$source_root/$relative"
  if [[ ! -f "$destination" ]]; then
    mkdir -p "$(dirname "$destination")"
    staged="$(mktemp "${TMPDIR:-/tmp}/art-control-flow.XXXXXX")"
    curl -fsSL "https://android.googlesource.com/$ART_PROJECT/+/$ART_REVISION/$relative?format=TEXT" |
      base64 -D > "$staged"
    [[ "$(sha "$staged")" == "$expected" ]] || fail "download SHA mismatch: $relative"
    mv "$staged" "$destination"
  fi
  [[ "$(sha "$destination")" == "$expected" ]] || fail "source SHA mismatch: $relative"
done
printf '%s\n' "$ART_REVISION" > "$source_root/.source-revision"
[[ -z "$(find "$source_root" -name .git -o -name .gitmodules -print -quit)" ]] ||
  fail "Git metadata is forbidden in sparse source"

python3 - "$source_root" <<'PY'
import sys
from pathlib import Path

root = Path(sys.argv[1])
loader = (root / "libnativeloader/native_loader.cpp").read_text()
namespace = (root / "libnativeloader/native_loader_namespace.cpp").read_text()
bridge = (root / "libnativebridge/native_bridge.cc").read_text()
vm = (root / "runtime/jni/java_vm_ext.cc").read_text()
header = (root / "libnativebridge/include/nativebridge/native_bridge.h").read_text()

open_start = loader.index("void* OpenNativeLibrary(")
close_start = loader.index("bool CloseNativeLibrary(", open_start)
host = loader[open_start:close_start]
assert host.index("*needs_native_bridge = false") < host.index("dlopen(path_arg, RTLD_NOW)")
assert host.index("NativeBridgeIsSupported(path_arg)") < host.index("*needs_native_bridge = true")
assert host.index("*needs_native_bridge = true") < host.index("NativeBridgeLoadLibrary(path_arg, RTLD_NOW)")
close = loader[close_start:loader.index("void NativeLoaderFreeErrorMessage", close_start)]
assert close.index("if (needs_native_bridge)") < close.index("NativeBridgeUnloadLibrary(handle)")
assert close.index("NativeBridgeUnloadLibrary(handle)") < close.index("dlclose(handle)")

load = namespace[namespace.index("Result<void*> NativeLoaderNamespace::Load") :]
assert load.index("if (!IsBridged())") < load.index("android_dlopen_ext")
assert load.index("android_dlopen_ext") < load.index("NativeBridgeLoadLibraryExt")

trampoline = bridge[bridge.index("void* NativeBridgeGetTrampoline2") :
                    bridge.index("void* NativeBridgeGetTrampolineForFunctionPointer")]
assert "if (!NativeBridgeInitialized())" in trampoline
assert trampoline.index("callbacks->getTrampolineWithJNICallType") < trampoline.rindex("callbacks->getTrampoline")
load_ext = bridge[bridge.index("void* NativeBridgeLoadLibraryExt") :
                  bridge.index("bool NativeBridgeIsNativeBridgeFunctionPointer")]
assert "isCompatibleWith(NAMESPACE_VERSION)" in load_ext and "callbacks->loadLibraryExt" in load_ext
assert "kJNICallTypeRegular = 1" in header and "kJNICallTypeCriticalNative = 2" in header

shared = vm[vm.index("class SharedLibrary") : vm.index("class Libraries")]
assert shared.index("needs_native_bridge_(needs_native_bridge)") < shared.index("CloseNativeLibrary")
assert shared.index("NeedsNativeBridge() ? FindSymbolWithNativeBridge") < shared.index("dlsym(handle_")
assert "NativeBridgeGetTrampoline2(" in shared and "shorty, len, jni_call_type" in shared
assert "uint32_t len = 0" in shared

lookup = vm[vm.index("void* FindNativeMethodInternal") : vm.index("void UnloadNativeLibraries")]
assert lookup.index("GetClassLoaderAllocator() != declaring_class_loader_allocator") < \
       lookup.index("const char* arg_shorty = library->NeedsNativeBridge() ? shorty : nullptr")
assert lookup.index("jni_short_name") < lookup.index("jni_long_name")
assert "kJNICallTypeCriticalNative" in vm and "kJNICallTypeRegular" in vm

load_vm = vm[vm.index("bool JavaVMExt::LoadNativeLibrary") : vm.index("static void* FindCodeForNativeMethodInAgents")]
assert load_vm.index("OpenNativeLibrary(") < load_vm.index("new SharedLibrary")
assert load_vm.index("new SharedLibrary") < load_vm.index('FindSymbol("JNI_OnLoad"')
assert 'FindSymbol("JNI_OnLoad", nullptr, android::kJNICallTypeRegular)' in load_vm
assert load_vm.index("SetClassLoaderOverride(class_loader)") < load_vm.index("(*jni_on_load)(this, nullptr)")
assert load_vm.index("(*jni_on_load)(this, nullptr)") < load_vm.index("SetClassLoaderOverride(old_class_loader.get())")
assert "It's unwise to call dlclose() here" in load_vm
assert load_vm.index("JNI_ERR returned from JNI_OnLoad") < load_vm.index("library->SetResult(was_successful)")

unload = vm[vm.index("static void UnloadLibraries") : vm.index("private:", vm.index("static void UnloadLibraries"))]
assert unload.index('FindSymbol("JNI_OnUnload", nullptr, android::kJNICallTypeRegular)') < \
       unload.index("jni_on_unload(vm, nullptr)")
print("native-library-control-flow: upstream-control-flow=PASS sources=6")
PY

stage="$(mktemp -d "${TMPDIR:-/tmp}/native-bridge-state.XXXXXX")"
trap 'rm -rf "$stage"' EXIT
cxx="$(xcrun --find clang++)"
macos_sdk="$(xcrun --sdk macosx --show-sdk-path)"
integration="$project_root/tools/android-native-bridge-integration"
"$cxx" -std=c++20 -arch arm64 -isysroot "$macos_sdk" -Wall -Wextra -Werror \
  -I"$integration/include" -I"$integration" \
  "$integration/native_bridge_state_machine_smoke.cc" -o "$stage/native-bridge-state-smoke"
if nm -u "$stage/native-bridge-state-smoke" | grep -E '_(dlopen|dlsym|dlclose)$' >/dev/null; then
  fail "state machine accidentally depends on dyld lookup"
fi
output="$("$stage/native-bridge-state-smoke")"
grep -F 'handle-tag=atomic classloader-namespace=isolated' <<< "$output" >/dev/null ||
  fail "handle/namespace state smoke failed"
grep -F 'OnLoad-failure=resident OnUnload-before-close=1' <<< "$output" >/dev/null ||
  fail "lifecycle state smoke failed"
grep -F 'shorty=len-normalized regular=1 critical=2' <<< "$output" >/dev/null ||
  fail "shorty/call-type smoke failed"
printf '%s\n' "$output"
echo "native-library-control-flow: PASS source-lock=$ART_REVISION C-ABI=v1 runtime-files-modified=0"

