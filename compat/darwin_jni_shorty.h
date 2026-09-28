#pragma once

#include <string>
#include <cstdint>

#include "jni.h"

namespace android {

JNIEnv* CurrentArtEnv();
// Returns the untagged Generic-JNI SaveRefsAndArgs frame for the current ART
// thread when a native callback is entered through that AOSP transition.
bool CurrentGenericJniFrame(uint64_t* managed_sp);
// Returns the current interpreter ShadowFrame identity when ART is executing
// through its shadow-frame interpreter on the current thread.
bool CurrentInterpreterFrame(uint64_t* shadow_frame);
bool DescriptorToShorty(const char* descriptor, std::string* shorty);

}  // namespace android
