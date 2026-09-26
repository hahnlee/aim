#pragma once

#include <jni.h>
#include "receiver_endpoint_binding.h"
#include "receiver_admission.h"
#include "receiver_registry.h"
#include "input_routing.h"
#include "receiver_finish_owner.h"
#include "key_fallback.h"

#include <atomic>
#include <cstdint>
#include <memory>

namespace darwin_art::input {
class InputChannelResources;
struct PendingReceiverRetirement;
class ReceiverRetirementDriver;

// Private receiver state; registry's public interface keeps this opaque.
// Lifecycle coordination installs retirement handles before publication;
// independent pending retention, not these members, owns deferred settlement.
struct InputReceiver {
  jobject weak_receiver = nullptr;
  jobject view_root = nullptr;
  // Strong global reference to the exact original InputChannel Binder token.
  // ChannelIdentityCatalog intentionally keeps only a weak token reference;
  // this pin lasts exactly until receiver resource cleanup.
  jobject original_channel_token = nullptr;
  std::shared_ptr<InputChannelResources> channel;
  uint32_t next_sequence = 1;
  mutable ReceiverFinishOwner finishes;
  // Unhandled-key fallbacks for keys dispatched to this receiver.
  mutable KeyFallbackOwner key_fallbacks;
  bool touch_mode = false;
  void* looper = nullptr;
  ReceiverId registry_id = 0;
  std::atomic<bool> disposed{false};
  std::atomic<bool> refs_cleaned{false};
  ReceiverAdmission admission;
  ReceiverEndpointBinding binding;
  std::shared_ptr<PendingReceiverRetirement> pending_retirement;
  std::shared_ptr<ReceiverRetirementDriver> retirement_driver;
  // Original exact publication identity, not a reconstructed numeric-ID token.
  // The ledger/lifecycle owns it; admitted callbacks pin it only while in use.
  std::weak_ptr<const InputRoutingRecipient> routing_recipient;
};

// These functions own resources only: no routing mutation or wake allocation.
void CleanupReceiverRefs(JNIEnv* env, InputReceiver* receiver);
void ReleaseReceiverAdmission(JNIEnv* env, InputReceiver* receiver);
void RetireReceiverResources(JNIEnv* env,
                            const std::shared_ptr<InputReceiver>& receiver);

class ReceiverCallbackAdmission final {
 public:
  ReceiverCallbackAdmission(JNIEnv* env, std::shared_ptr<InputReceiver> receiver);
  ~ReceiverCallbackAdmission();
  bool Admitted() const { return admitted_; }
  ReceiverCallbackAdmission(const ReceiverCallbackAdmission&) = delete;
  ReceiverCallbackAdmission& operator=(const ReceiverCallbackAdmission&) = delete;
 private:
  JNIEnv* env_;
  const std::shared_ptr<InputReceiver> receiver_;
  bool admitted_;
};

}  // namespace darwin_art::input
