# Caller owns project_root, native_root, and task_stage. Original sources must
# already have passed the pinned materializer. This file owns compiler inputs,
# not resource acquisition or Android map policy.
mkdir -p "$task_stage/include" "$task_stage/src"
# Match toolbox_input_labels and Bionic's kernel_input_headers filegroup.
# Generate from the pinned upstream tool and headers, never a copied label list.
python3 "$project_root/_aosp/android16-key-character-map/system/core/toolbox/generate-input.h-labels.py" \
  "$project_root/_aosp/android16-key-character-map/bionic/libc/kernel/uapi/linux/input.h" \
  "$project_root/_aosp/android16-key-character-map/bionic/libc/kernel/uapi/linux/input-event-codes.h" \
  > "$task_stage/include/input.h-labels.h"
"$project_root/_downloads/android16-surfaceflinger-core/tools/aidl" \
  --lang=cpp --min_sdk_version=29 --omit_invocation \
  -I"$native_root/libs/input" -h "$task_stage/include" -o "$task_stage/src" \
  "$native_root/libs/input/android/os/PointerIconType.aidl" \
  "$native_root/libs/input/android/os/MotionEventFlag.aidl" \
  "$native_root/libs/input/android/os/IInputConstants.aidl"
shadow="$project_root/_build/surfaceflinger-core/work/frameworks-native"
generated="$project_root/_build/surfaceflinger-core/work/generated"
libhidl="$project_root/_aosp/android16-surfaceflinger-core/system-libhidl"
libfmq="$project_root/_aosp/android16-surfaceflinger-core/system-libfmq"
sdk_root="$(xcrun --sdk macosx --show-sdk-path)"
source "$project_root/tools/lib/surfaceflinger-compile-flags.sh"
kcm_flags=(
  -I"$native_root/include" -I"$task_stage/include" "${flags[@]}"
  -include "$task_stage/include/android/os/IInputConstants.h"
  -include "$task_stage/include/android/os/MotionEventFlag.h"
  -I"$project_root/_aosp/android16-key-character-map/bionic/libc/kernel/uapi"
  -I"$project_root/_aosp/android16-surfaceflinger-core/binder-uapi/libc/kernel/uapi"
  -I"$project_root/_aosp/android16-surfaceflinger-core/binder-uapi/libc/kernel/uapi/asm-arm64"
  -I"$project_root/_aosp/android16-surfaceflinger-core/binder-uapi/libc/kernel/android/uapi"
  -I"$project_root/_aosp/external/fmtlib/include"
  -I"$project_root/_build/nativehelper-foundation/source/libnativehelper/include_jni"
  -I"$project_root/_build/nativehelper-foundation/source/libnativehelper/include"
  -I"$project_root/_build/nativehelper-foundation/source/libnativehelper/header_only_include"
)
