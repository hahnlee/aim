#!/bin/bash
set -euo pipefail
export LC_ALL=C

root="$(cd "$(dirname "$0")/.." && pwd)"
source "$root/upstream/android16-activity-thread.lock"

source_root="$root/_aosp/android16-activity-thread"
out="$root/_build/activity-thread"
mkdir -p "$source_root/core/jni" "$source_root/bionic" "$out"

sha256() { shasum -a 256 "$1" | awk '{print $1}'; }

materialize() {
  local destination="$1" url="$2" expected="$3"
  if [[ ! -f "$destination" ]]; then
    local staged
    staged="$(mktemp "$source_root/download.XXXXXX")"
    if ! curl -fsSL "$url?format=TEXT" | base64 -D > "$staged"; then
      rm -f "$staged"
      return 1
    fi
    if [[ "$(sha256 "$staged")" != "$expected" ]]; then
      rm -f "$staged"
      echo "activity-thread: source hash mismatch: $destination" >&2
      return 1
    fi
    mv "$staged" "$destination"
  fi
  if [[ "$(sha256 "$destination")" != "$expected" ]]; then
    echo "activity-thread: source hash mismatch: $destination" >&2
    return 1
  fi
}

materialize "$source_root/core/jni/android_app_ActivityThread.cpp" \
  "https://android.googlesource.com/$FRAMEWORKS_BASE_PROJECT/+/$FRAMEWORKS_BASE_REVISION/core/jni/android_app_ActivityThread.cpp" \
  "$ACTIVITY_THREAD_CPP_SHA256"
materialize "$source_root/core/jni/android_app_Activity.cpp" \
  "https://android.googlesource.com/$FRAMEWORKS_BASE_PROJECT/+/$FRAMEWORKS_BASE_REVISION/core/jni/android_app_Activity.cpp" \
  "$ACTIVITY_CPP_SHA256"
materialize "$source_root/malloc.h" \
  "https://android.googlesource.com/$BIONIC_PROJECT/+/$BIONIC_REVISION/libc/include/malloc.h" \
  "$BIONIC_MALLOC_H_SHA256"
materialize "$source_root/bionic/malloc.h" \
  "https://android.googlesource.com/$BIONIC_PROJECT/+/$BIONIC_REVISION/libc/platform/bionic/malloc.h" \
  "$BIONIC_PLATFORM_MALLOC_H_SHA256"

helpers="$root/_aosp/frameworks-base-android-util-log/core/jni"
nativehelper="$root/_build/nativehelper-foundation/source/libnativehelper"
allocator="$root/tools/bionic-libc-allocator-facade/include"
for pair in \
  "$helpers/core_jni_helpers.h:$CORE_JNI_HELPERS_SHA256" \
  "$helpers/jni_wrappers.h:$JNI_WRAPPERS_SHA256" \
  "$helpers/include/android_runtime/AndroidRuntime.h:$ANDROID_RUNTIME_H_SHA256" \
  "$nativehelper/include/nativehelper/JNIHelp.h:$JNI_HELP_H_SHA256" \
  "$nativehelper/include_platform/nativehelper/JNIPlatformHelp.h:$JNI_PLATFORM_HELP_H_SHA256"; do
  file="${pair%%:*}"
  expected="${pair##*:}"
  [[ -f "$file" ]] || { echo "activity-thread: missing locked support header: $file" >&2; exit 1; }
  [[ "$(sha256 "$file")" == "$expected" ]] || {
    echo "activity-thread: support header hash mismatch: $file" >&2
    exit 1
  }
done
[[ -f "$allocator/darwin_art_bionic_allocator.h" ]] || {
  echo "activity-thread: missing allocator provider ABI header: $allocator/darwin_art_bionic_allocator.h" >&2
  exit 1
}

flags=(
  -std=c++20 -arch arm64 -fPIC -Wno-writable-strings
  -D__ANDROID_API__=36 '-D__INTRODUCED_IN(n)='
  '-D__BIONIC_AVAILABILITY_GUARD(n)=1'
  '-D__nodiscard=[[nodiscard]]' '-D__mallocfunc=__attribute__((malloc))'
  '-D__RENAME(n)='
  '-D__clang_error_if(cond,msg)=__attribute__((diagnose_if(cond,msg,"error")))'
  -I"$source_root"
  -I"$helpers" -I"$helpers/include"
  -I"$nativehelper/include_jni" -I"$nativehelper/include"
  -I"$nativehelper/include_platform" -I"$nativehelper/include_platform_header_only"
  -I"$nativehelper/header_only_include"
  -I"$root/_aosp/system/logging/liblog/include"
  -I"$root/_aosp/system/core/libutils/include"
  -I"$root/_aosp/system/core/libutils/binder/include"
  -I"$root/_aosp/system/core/libsystem/include"
  -I"$root/_aosp/system/core/libcutils/include"
  -I"$root/_aosp/system/libbase/include"
  -I"$allocator"
  -include "$root/tools/activity-thread/bindings.h"
)

activity_object="$out/android_app_Activity.o"
activity_thread_object="$out/android_app_ActivityThread.o"
xcrun clang++ "${flags[@]}" -c \
  "$source_root/core/jni/android_app_Activity.cpp" -o "$activity_object"
xcrun clang++ "${flags[@]}" -c \
  "$source_root/core/jni/android_app_ActivityThread.cpp" -o "$activity_thread_object"
xcrun libtool -static -o "$out/libactivity-thread-darwin.a" \
  "$activity_object" "$activity_thread_object"

echo "activity-thread: AOSP Activity/ActivityThread JNI archive ready: $out/libactivity-thread-darwin.a"
