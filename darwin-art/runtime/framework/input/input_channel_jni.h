#pragma once

#include "input_channel_parcel.h"
#include "channel_resources.h"

namespace darwin_art::input {
// Java channel wrappers and Parcel lifetime stay private here. The receiver
// owner may retain one explicit strong global reference to the exact original
// Binder token for its lifetime; it must release that reference through its
// live-env cleanup path.
std::shared_ptr<InputChannelResources> AcquireInputChannelResources(jlong pointer);
std::shared_ptr<InputChannelResources> AcquireInputChannelResources(JNIEnv*, jobject);
// WMS publication requires the server-side wrapper capability. The wrapper and
// Binder/Parcel layout remain private; consumers retain only JNI-free resources.
std::shared_ptr<InputChannelResources> AcquireServerInputChannelResources(JNIEnv*, jobject);
// Returns an owned strong global reference to the exact token held by the
// supplied Java InputChannel, or null on invalid/disposed input or JNI error.
// The caller owns deletion and must have a live JNIEnv for that cleanup.
jobject PinInputChannelToken(
    JNIEnv*, jobject, const std::shared_ptr<InputChannelResources>& expected);
bool RegisterInputChannelJni(JNIEnv*, InputChannelParcelBridge);
}  // namespace darwin_art::input
