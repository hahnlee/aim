#pragma once

#include "input_routing.h"
#include "input_transport_pump.h"
#include <jni.h>
#include <memory>

namespace darwin_art::input {
struct InputReceiver;
// Called synchronously by the admitted original receiver's owner Looper.
// Routing admission and JNI notification are separate: no routing/provider
// lock is held while the framework callback runs.
FocusControlCallbackResult ConsumeReceiverFocusControl(
    JNIEnv* env, const std::shared_ptr<InputReceiver>& receiver,
    const InputRoutingRecipientHandle& expected, FocusControl control) noexcept;
}  // namespace darwin_art::input
