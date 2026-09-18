#pragma once

#include "receiver_routing_lifecycle.h"
#include "receiver_admission.h"
#include "receiver_endpoint_binding.h"

namespace darwin_art::input {

enum class ReceiverRetirementBarrierStatus : uint8_t {
  kLifecycle,
  kAdmission,
  kBinding,
  kRouting,
  kTx,
  kReady,
  kBusy,
  kInvalid,
};
struct ReceiverRetirementBarrierQuery {
  ReceiverRetirementBarrierStatus status = ReceiverRetirementBarrierStatus::kLifecycle;
  InputRoutingRetirementQuery routing;
  InputTransportTxFenceStatus tx = InputTransportTxFenceStatus::kInvalid;
  bool fence_captured = false;
};

// Resource-only settlement proof for an Android InputEventReceiver epoch.
// Prepare/retain outside the receiver before visibility. Does not retain the
// receiver, JNI globals or channel wrapper; does not dispatch CANCEL, pump TX,
// transfer registration authority or close descriptors. Those belong to the
// independently retained retirement coordinator and transport progress owner.
class ReceiverRetirementBarrier final {
 public:
  // Preparation throws invalid_argument for a null/mismatched exact transport.
  // No such invalid object is exposed to the registry or asynchronous callers.
  ReceiverRetirementBarrier(const ReceiverRoutingLifecycle& lifecycle,
                           ReceiverAdmission::RetirementHandle admission,
                           ReceiverEndpointBinding::RetirementHandle binding,
                           std::shared_ptr<InputTransport> original_transport);
  ReceiverRetirementBarrier Retain() const;
  ReceiverRetirementBarrier(const ReceiverRetirementBarrier&) = delete;
  ReceiverRetirementBarrier& operator=(const ReceiverRetirementBarrier&) = delete;
  // Finite accepted TX prefix is captured only after all old producers have
  // ended, including admitted sends masked by terminal routing. Successor
  // traffic after capture cannot extend this epoch's settlement obligation.
  ReceiverRetirementBarrierQuery Poll();
 private:
  struct Control;
  explicit ReceiverRetirementBarrier(std::shared_ptr<Control> control)
      : control_(std::move(control)) {}
  const std::shared_ptr<Control> control_;
};

}  // namespace darwin_art::input
