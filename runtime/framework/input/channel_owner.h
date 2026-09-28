#pragma once

#include <cstdint>

#include <jni.h>

#include "darwin_framework_input_hint.h"
#include "input_channel_parcel.h"
#include "input_routing.h"

namespace aim::input {

// Registers Android InputChannel/InputEventReceiver
// natives and stores an immutable copy of the Parcel bridge for their JNI
// callbacks. The bridge is not retained as a caller-owned pointer.
bool RegisterInputNatives(JNIEnv* env, InputChannelParcelBridge bridge);
// Submit only an already-routed immutable admission; never select another
// channel while refreshing transport/wake/continuation ownership.
aim::AimInputEnqueueResult SubmitFrameworkInputAdmission(
    InputRoutingAdmission);

}  // namespace aim::input
