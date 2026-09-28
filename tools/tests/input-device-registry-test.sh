#!/bin/bash
set -euo pipefail

test_root="$(cd "$(dirname "$0")/../.." && pwd)"
source_file="$test_root/runtime/framework/input/InputDeviceRegistry.java"
native_input_header="$test_root/_aosp/frameworks/native/include/android/input.h"

# Pinned Android's native input contract defines alphabetic keyboard as 2;
# KeyCharacterMap.FULL (4) is a different enum and must not reach the builder.
grep -Fq 'import android.view.InputDevice;' "$source_file"
grep -Fq 'Integer.valueOf(InputDevice.KEYBOARD_TYPE_ALPHABETIC)' "$source_file"
! grep -Fq 'setKeyboardType", int.class, Integer.valueOf(4)' "$source_file"
grep -Eq 'AINPUT_KEYBOARD_TYPE_ALPHABETIC[[:space:]]*=[[:space:]]*2' \
  "$native_input_header"

echo "input-device registry: alphabetic keyboard type uses pinned InputDevice constant PASS"
