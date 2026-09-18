#include "receiver_routing_lifecycle.h"

#include <mutex>
#include <stdexcept>
#include <utility>

namespace darwin_art::input {

struct ReceiverRoutingLifecycle::Control {
  enum class Operation { kIdle, kPublishing, kRetiring };
  explicit Control(InputRoutingRecipientHandle token)
      : routing(token == nullptr ? nullptr : token->Routing()),
        recipient(std::move(token)) {
    if (routing == nullptr || recipient->id == 0)
      throw std::invalid_argument("receiver routing requires a prepared recipient");
  }
  // Pin routing independently: recipient itself contains only a weak reference.
  const InputRoutingHandle routing;
  const InputRoutingRecipientHandle recipient;
  std::mutex mutex;
  Operation operation = Operation::kIdle;
  bool attempted = false;
  ReceiverRoutingLifecycleSnapshot state;
};

ReceiverRoutingLifecycle::ReceiverRoutingLifecycle(InputRoutingRecipientHandle recipient)
    : control_(std::make_shared<Control>(std::move(recipient))) {}

ReceiverRoutingLifecycle ReceiverRoutingLifecycle::Retain() const {
  return ReceiverRoutingLifecycle(control_);
}

InputRoutingEndpointHandle ReceiverRoutingLifecycle::OriginalEndpoint() const {
  return control_->recipient->OriginalEndpoint();
}

ReceiverRoutingPublishResult ReceiverRoutingLifecycle::Publish(ReceiverId expected_current_id) {
  const auto control = control_;
  {
    std::lock_guard lock(control->mutex);
    if (control->state.close_requested) return {ReceiverRoutingPublishStatus::kClosed, {}};
    if (control->operation != Control::Operation::kIdle)
      return {ReceiverRoutingPublishStatus::kBusy, {}};
    if (control->attempted) return {ReceiverRoutingPublishStatus::kAlreadyAttempted, control->state.publication};
    control->operation = Control::Operation::kPublishing;
  }
  InputRoutingPublicationResult publication;
  try {
    publication = PublishInputRoutingRecipient(control->recipient, expected_current_id);
  } catch (...) {
    bool close;
    {
      std::lock_guard lock(control->mutex);
      control->operation = Control::Operation::kIdle;
      close = control->state.close_requested;
    }
    if (close) (void)CloseControl(control);
    throw;
  }
  bool close;
  {
    std::lock_guard lock(control->mutex);
    control->state.publication = publication;
    control->attempted = true;
    control->operation = Control::Operation::kIdle;
    close = control->state.close_requested;
  }
  if (close) (void)CloseControl(control);
  return {ReceiverRoutingPublishStatus::kCompleted, publication};
}

ReceiverRoutingCloseStatus ReceiverRoutingLifecycle::CloseControl(const std::shared_ptr<Control>& control) {
  {
    std::lock_guard lock(control->mutex);
    control->state.close_requested = true;
    if (control->state.logical_closed) return ReceiverRoutingCloseStatus::kClosed;
    if (control->operation != Control::Operation::kIdle)
      return ReceiverRoutingCloseStatus::kDeferred;
    control->operation = Control::Operation::kRetiring;
  }
  InputRoutingRecipientRetirementResult retirement;
  try {
    retirement = RetireInputRoutingRecipient(control->recipient);
  } catch (...) {
    std::lock_guard lock(control->mutex);
    control->operation = Control::Operation::kIdle;
    throw;
  }
  {
    std::lock_guard lock(control->mutex);
    control->state.retirement = retirement;
    control->state.logical_closed = true;
    control->operation = Control::Operation::kIdle;
  }
  return ReceiverRoutingCloseStatus::kClosed;
}

ReceiverRoutingCloseStatus ReceiverRoutingLifecycle::Close() {
  const auto control = control_;
  return CloseControl(control);
}

ReceiverRoutingLifecycleSnapshot ReceiverRoutingLifecycle::Snapshot() const {
  const auto control = control_;
  std::lock_guard lock(control->mutex);
  auto state = control->state;
  state.operation_pending = control->operation != Control::Operation::kIdle;
  return state;
}

}  // namespace darwin_art::input
