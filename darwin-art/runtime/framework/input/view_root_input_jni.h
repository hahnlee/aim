#pragma once

#include <cstdint>
#include <memory>

#include <jni.h>

#include "framework_dispatch.h"
#include "input_routing.h"
#include "input_event_origin.h"

namespace darwin_art::input {

struct InputReceiver;

struct OriginalInputReceiverDispatchContext;
using OriginalInputReceiverEventDispatch =
    FrameworkInputEventDispatchResult (*)(
        JNIEnv*, const OriginalInputReceiverDispatchContext&, jobject);

// The channel owner supplies the admitted original receiver. Keeping this
// shared lease in the dispatch context preserves the original weak Java
// receiver/ViewRoot JNI resources while packet construction and callbacks
// run; it is never reconstructed from ViewRootImpl.mInputEventReceiver.
struct OriginalInputReceiverDispatchContext final {
  std::shared_ptr<InputReceiver> receiver;
  // Strong pin of the exact publication token admitted by the transport
  // callback. The dispatch path never reacquires a possibly changed weak
  // token after packet/JNI work has started.
  InputRoutingRecipientHandle recipient;
  OriginalInputReceiverEventDispatch dispatch = nullptr;
  InputEventOrigin origin{};
  // Borrowed only during synchronous packet construction/dispatch. Never
  // retained in InputEventOrigin or the asynchronous finish ledger.
  InputRoutingPacketLease* local_packet_lease = nullptr;
};

struct InputWindowGeometry {
  int32_t left = 0;
  int32_t top = 0;
  int32_t right = 0;
  int32_t bottom = 0;
};

// Returns a caller-owned local ViewRoot only for the pinned framework's
// WindowInputEventReceiver. A cleared weak referent or a generic receiver has
// no ViewRoot. JNI failures remain pending; this emits no focus/mode events.
enum class ReceiverViewRootKind { kAbsent, kGeneral, kWindow, kError };
struct ReceiverViewRootResolution {
  ReceiverViewRootKind kind = ReceiverViewRootKind::kError;
  jobject local_root = nullptr;
};
ReceiverViewRootResolution ResolveWeakWindowReceiverViewRoot(JNIEnv*, jobject weak_receiver);

// Dispatch to the exact receiver admitted by the owner-Looper callback. This
// path uses the context receiver's weak_receiver and does not inspect the
// mutable ViewRoot receiver pointer.
FrameworkInputEventDispatchResult DispatchFrameworkInputEventResultForReceiver(
    JNIEnv* env, const OriginalInputReceiverDispatchContext& context,
    jobject event);

// Reads ViewRootImpl.mWinFrame without consuming a Java exception.  A false
// result means that a JNI lookup/read failed or that the input was invalid;
// any exception raised by JNI remains pending on env for the caller.
bool ReadViewRootGeometry(JNIEnv* env, jobject view_root,
                          InputWindowGeometry* geometry);

}  // namespace darwin_art::input
