#pragma once

#include <jni.h>
#include <memory>

namespace darwin_art::input {
struct InputReceiver;

struct ReceiverFocusNotificationResult {
  // Once invoked, this notification must not be replayed, even if Java throws
  // or disposes its native receiver reentrantly.
  bool invoked = false;
  bool delivered = false;
};

// JNI delivery only, on the receiver's owner Looper after authoritative focus
// admission. This does not select a window, change routing, resolve a ViewRoot,
// fabricate geometry, or retain another global reference.
ReceiverFocusNotificationResult NotifyInputReceiverFocus(
    JNIEnv* env, const std::shared_ptr<InputReceiver>& receiver, bool focused);
}  // namespace darwin_art::input
