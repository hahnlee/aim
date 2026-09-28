#!/bin/bash
set -euo pipefail
export LC_ALL=C

dir="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$dir/../.." && pwd)"
# shellcheck disable=SC1090
source "$dir/sources.lock"

fail() { echo "android-classloader-native-state: $*" >&2; exit 2; }
sha() { shasum -a 256 "$1" | awk '{print $1}'; }

[[ "$(($(wc -l < "$dir/upstream-sources.tsv") - 1))" == "$LOCKED_SOURCE_COUNT" ]] ||
  fail 'locked source count drift'

source_root="$root/_aosp/android16-classloader-native-state"
while IFS=$'\t' read -r relative size expected; do
  [[ "$relative" != path ]] || continue
  destination="$source_root/$relative"
  if [[ ! -f "$destination" ]]; then
    mkdir -p "$(dirname "$destination")"
    staged="$(mktemp "${TMPDIR:-/tmp}/classloader-native-source.XXXXXX")"
    curl -fsSL \
      "https://android.googlesource.com/$AOSP_ART_PROJECT/+/$AOSP_ART_REVISION/$relative?format=TEXT" |
      base64 -D > "$staged"
    [[ "$(stat -f %z "$staged")" == "$size" && "$(sha "$staged")" == "$expected" ]] ||
      fail "AOSP source provenance mismatch: $relative"
    mv "$staged" "$destination"
  fi
  [[ "$(stat -f %z "$destination")" == "$size" && "$(sha "$destination")" == "$expected" ]] ||
    fail "AOSP sparse source drift: $relative"
done < "$dir/upstream-sources.tsv"

[[ "$(sha "$source_root/runtime/jni/java_vm_ext.cc")" == "$JAVA_VM_EXT_CPP_SHA256" ]]
[[ "$(sha "$source_root/libnativeloader/native_loader.cpp")" == "$NATIVE_LOADER_CPP_SHA256" ]]
[[ "$(sha "$source_root/libnativeloader/library_namespaces.cpp")" == \
   "$LIBRARY_NAMESPACES_CPP_SHA256" ]]
[[ "$(sha "$source_root/libnativeloader/library_namespaces.h")" == \
   "$LIBRARY_NAMESPACES_H_SHA256" ]]
[[ "$(sha "$source_root/libnativebridge/native_bridge.cc")" == "$NATIVE_BRIDGE_CPP_SHA256" ]]

python3 - "$source_root" <<'PY'
from pathlib import Path
import sys

root = Path(sys.argv[1])
vm = (root / "runtime/jni/java_vm_ext.cc").read_text()
loader = (root / "libnativeloader/native_loader.cpp").read_text()
namespaces = (root / "libnativeloader/library_namespaces.cpp").read_text()
namespaces_h = (root / "libnativeloader/library_namespaces.h").read_text()
bridge = (root / "libnativebridge/native_bridge.cc").read_text()

shared = vm[vm.index("class SharedLibrary") : vm.index("class Libraries")]
assert "class_loader_(env->NewWeakGlobalRef(class_loader))" in shared
assert "class_loader_allocator_(class_loader_allocator)" in shared
assert shared.index("DeleteWeakGlobalRef(class_loader_)") < shared.index("CloseNativeLibrary(")
assert "while (jni_on_load_result_ == kPending)" in shared
assert "jni_on_load_thread_id_ == self->GetThreadId()" in shared
assert "jni_on_load_result_ = result ? kOkay : kFailed" in shared

load = vm[vm.index("bool JavaVMExt::LoadNativeLibrary") :
          vm.index("static void* FindCodeForNativeMethodInAgents")]
assert load.index("class_linker->IsBootClassLoader(loader)") < load.index("class_loader = nullptr")
assert load.index("libraries_->Get(path)") < load.index("GetClassLoaderAllocator")
assert load.index("GetClassLoaderAllocator() != class_loader_allocator") < \
       load.index("library->CheckOnLoadResult()")
assert load.index("OpenNativeLibrary(") < load.index("new SharedLibrary")
assert load.index("new SharedLibrary") < load.index("libraries_->Put(path, library)")
assert load.index("libraries_->Put(path, library)") < load.index('FindSymbol("JNI_OnLoad"')
assert "It's unwise to call dlclose() here" in load
assert load.index("library->SetResult(was_successful)") > load.index("(*jni_on_load)(this, nullptr)")
# Android 16's race-loser branch waits on the winner but does not revalidate the
# allocator. The standalone contract deliberately closes this cross-loader race.
lost_race = load[load.index("if (!created_library)") : load.index("VLOG(jni) << \"[Added")]
assert "library->CheckOnLoadResult()" in lost_race
assert "GetClassLoaderAllocator" not in lost_race

libraries = vm[vm.index("class Libraries") : vm.index("class JII")]
lookup = libraries[libraries.index("void* FindNativeMethodInternal") :
                   libraries.index("void UnloadNativeLibraries")]
assert lookup.index("GetClassLoaderAllocator() != declaring_class_loader_allocator") < \
       lookup.index("library->FindSymbol")
unload = libraries[libraries.index("void UnloadNativeLibraries") :]
assert unload.index("IsJWeakCleared(class_loader)") < unload.index("libraries_.erase(it)")
assert unload.index("libraries_.erase(it)") < unload.index("UnloadLibraries(")
unload_calls = libraries[libraries.index("static void UnloadLibraries") :]
assert unload_calls.index('FindSymbol("JNI_OnUnload"') < unload_calls.index("jni_on_unload(vm")

assert "std::list<std::pair<jweak, NativeLoaderNamespace>> namespaces_" in namespaces_h
assert "env->NewWeakGlobalRef(class_loader)" in namespaces
find_ns = namespaces[namespaces.index("LibraryNamespaces::FindNamespaceByClassLoader") :
                     namespaces.index("LibraryNamespaces::FindParentNamespaceByClassLoader")]
assert "env->IsSameObject(value.first, class_loader)" in find_ns
create_ns = namespaces[namespaces.index("LibraryNamespaces::Create") :]
assert "There is already a namespace associated with this classloader" in create_ns
assert "FindParentNamespaceByClassLoader(env, class_loader)" in create_ns
assert "g_namespaces_mutex held" in create_ns

open_native = loader[loader.index("void* OpenNativeLibrary(") :
                     loader.index("bool CloseNativeLibrary(")]
boot = open_native[:open_native.index("NativeLoaderNamespace* ns;")]
assert "if (class_loader == nullptr)" in boot
assert "FindApexNamespace(caller_location)" in boot
assert "OpenSystemLibrary(path, RTLD_NOW)" in boot
app = open_native[open_native.index("NativeLoaderNamespace* ns;") :]
assert app.index("FindNamespaceByClassLoader(env, class_loader)") < \
       app.index("CreateClassLoaderNamespaceLocked")
assert "create an isolated not-shared namespace" in app
assert app.index("*needs_native_bridge = ns->IsBridged()") < app.index("ns->Load(path)")

trampoline = bridge[bridge.index("void* NativeBridgeGetTrampoline2") :
                    bridge.index("void* NativeBridgeGetTrampolineForFunctionPointer")]
assert "callbacks->getTrampolineWithJNICallType" in trampoline
unload_bridge = bridge[bridge.index("int NativeBridgeUnloadLibrary") :
                       bridge.index("const char* NativeBridgeGetError")]
assert "callbacks->unloadLibrary(handle)" in unload_bridge
print("android-classloader-native-state: upstream semantics PASS sources=5")
print("android-classloader-native-state: AOSP-race-loser allocator-recheck=absent (closed by contract)")
PY

tmp="$(mktemp -d "${TMPDIR:-/tmp}/classloader-native-state.XXXXXX")"
cleanup() {
  [[ "$tmp" == "${TMPDIR:-/tmp}"/classloader-native-state.* ]] && find "$tmp" -depth -delete
}
trap cleanup EXIT
cxx="$(xcrun --find clang++)"
sdk="$(xcrun --sdk macosx --show-sdk-path)"
common=(-std=c++20 -arch arm64 -isysroot "$sdk" -pthread -Wall -Wextra -Werror -Wpedantic
        -I"$dir/include" "$dir/src/android_classloader_native_state.cc"
        "$dir/probes/state_machine_test.cc")

for sanitizer in address undefined thread; do
  binary="$tmp/state-$sanitizer"
  "$cxx" "${common[@]}" -O1 -g -fno-omit-frame-pointer -fsanitize="$sanitizer" -o "$binary"
  case "$sanitizer" in
    address) ASAN_OPTIONS=detect_leaks=0:halt_on_error=1 "$binary" ;;
    undefined) UBSAN_OPTIONS=halt_on_error=1:print_stacktrace=1 "$binary" ;;
    thread) TSAN_OPTIONS=halt_on_error=1 "$binary" ;;
  esac
done

if nm -u "$tmp/state-address" | grep -E '_(dlopen|dlsym|dlclose|dyld)' >/dev/null; then
  fail 'host dynamic-loader fallback entered standalone state machine'
fi
if rg -n '(^|[^A-Za-z])(dlopen|dlsym|dlclose|dyld|RTLD_)' "$dir/include" "$dir/src" \
    "$dir/probes" >/dev/null; then
  fail 'dynamic-loader API entered standalone source'
fi
xcrun clang-format --dry-run --Werror "$dir/include/android_classloader_native_state.h" \
  "$dir/src/android_classloader_native_state.cc" "$dir/probes/state_machine_test.cc"

git -C "$root" diff --check -- tools/android-classloader-native-state
git -C "$root" diff --cached --check -- tools/android-classloader-native-state
while IFS= read -r -d '' file; do
  set +e
  whitespace="$(git -C "$root" diff --no-index --check /dev/null "$file" 2>&1)"
  status=$?
  set -e
  [[ -z "$whitespace" ]] || fail "untracked whitespace: $file: $whitespace"
  [[ $status -le 1 ]] || fail "could not diff-check untracked file: $file"
done < <(git -C "$root" ls-files --others --exclude-standard -z -- \
         tools/android-classloader-native-state)

echo "android-classloader-native-state: PASS source=$AOSP_ART_TAG@$AOSP_ART_REVISION cache=path-owner namespaces=per-loader weak-lifecycle=ordered sanitizers=ASan+UBSan+TSan"
