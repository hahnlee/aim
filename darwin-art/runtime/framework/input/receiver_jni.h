#pragma once

#include <jni.h>

#include <memory>

#include "input_routing.h"

namespace darwin_art::input {

class InputChannelResources;

// Validates the exact published receiver/channel/transport identity before
// changing writable interest. This is implemented by the receiver owner so
// channel routing never needs to inspect private receiver state.
bool RefreshInputReceiverWritable(const InputRoutingHandle& routing);

// Receiver JNI owns InputEventReceiver callbacks and lifecycle. Channel
// routing remains the owner of transport scheduling; these hooks are the
// complete receiver-facing writable/scheduler/wake boundary and retain no
// caller state.
struct ReceiverChannelOps final {
  bool (*refresh_writable)(const InputRoutingHandle&) = nullptr;
  bool (*ensure_scheduler)(const std::shared_ptr<InputChannelResources>&,
                           void* looper) = nullptr;
  void (*wake_pending)() = nullptr;
};

// Registers the Android InputEventReceiver native methods and installs an
// immutable copy of the channel routing hooks for owner-Looper callbacks.
bool RegisterInputReceiverNatives(JNIEnv* env, JavaVM* vm,
                                  ReceiverChannelOps ops);

}  // namespace darwin_art::input
