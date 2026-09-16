#pragma once

#include <cstdint>
#include <string>
#include <vector>

#include <jni.h>

namespace darwin_art::jni {

// A copy of the AArch64 Android va_list state. Android keeps eight-byte GP
// slots, sixteen-byte FP register slots, and an eight-byte aligned stack tail.
// The decoder copies this structure before consuming it, so the caller's
// va_list remains unchanged.
struct AndroidArm64VaList {
  uint8_t* stack;
  uint8_t* gr_top;
  uint8_t* vr_top;
  int32_t gr_offs;
  int32_t vr_offs;
};

static_assert(sizeof(AndroidArm64VaList) == 32);

// Decode an Android JNI method descriptor and its Android AArch64 variadic
// arguments into jvalue records. ART remains the authority for class names and
// method validity; callers supply its successfully resolved descriptor and a
// readable native va_list/save area. Descriptor structure is checked before
// argument memory is read. On rejection, output is left unchanged. This is an
// ABI converter, not a validator of arbitrary native pointers.
bool DecodeAndroidArguments(const std::string& descriptor,
                            const void* raw_args,
                            std::vector<jvalue>* output);

}  // namespace darwin_art::jni
