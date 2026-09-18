#pragma once

#include <jni.h>

#include "input_routing.h"
#include "input_event_origin.h"

namespace darwin_art::input {

struct InputReceiver;

// The channel owner keeps DarwinInputChannelState private.  These helpers are
// the narrow receiver-facing bridge for Android's ViewRoot JNI owner: a
// receiver lease/admission keeps the pointed-to state alive while each helper
// runs, and no channel state or transport object crosses this boundary.
InputRoutingHandle ReceiverRoutingHandle(const InputReceiver* receiver);
// Atomically select a non-live Java sequence and reserve its original event
// before invocation. Failure preserves the cursor and caller output.
bool AllocateReceiverFinish(InputReceiver* receiver, jint* sequence,
                            InputEventOrigin origin,
                            InputRoutingRecipientHandle original);
bool CancelReceiverFinish(const InputReceiver* receiver, jint sequence);
bool CloseReceiverFinishObservation(const InputReceiver* receiver, jint sequence,
                                    bool* handled);
void RequestReceiverRoutingProgress(const InputReceiver* receiver);

}  // namespace darwin_art::input
