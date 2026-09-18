#pragma once

#include <jni.h>

namespace darwin_art {

struct FrameworkInputEventDispatchResult {
  // Once invoked is true, Android owns the event and will recycle it from
  // InputEventReceiver, including when the call throws or its ACK is deferred.
  bool invoked = false;
  // True only when dispatchInputEvent was invoked without a pending exception.
  bool delivered = false;
  // ACK state is nonblocking: false means no finish ACK was observed yet.
  bool acknowledged = false;
  // Valid only when acknowledged is true.
  bool handled = false;
  jint framework_sequence = 0;
  bool rejected = false;
};

}  // namespace darwin_art
