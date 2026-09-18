#include "receiver_admission.h"

namespace darwin_art::input {

struct ReceiverAdmission::Control {
  std::mutex mutex;
  std::uint32_t active = 0;
  bool retired = false;
  bool cleanup_claimed = false;
  bool cleanup_completed = false;
  bool notified = false;
  void (*notify)(void*) noexcept = nullptr;
  std::weak_ptr<void> context;
};

ReceiverAdmission::ReceiverAdmission() : control_(std::make_shared<Control>()) {}

bool ReceiverAdmission::IsQuiescent() const {
  const auto control = control_;
  std::lock_guard<std::mutex> lock(control->mutex);
  return control->retired && control->active == 0;
}

bool ReceiverAdmission::Admit() {
  const auto control = control_;
  std::lock_guard<std::mutex> lock(control->mutex);
  if (control->retired) return false;
  ++control->active;
  return true;
}

bool ReceiverAdmission::Retire() {
  const auto control = control_;
  std::lock_guard<std::mutex> lock(control->mutex);
  control->retired = true;
  if (control->active != 0 || control->cleanup_claimed) return false;
  control->cleanup_claimed = true;
  return true;
}

bool ReceiverAdmission::Release() {
  const auto control = control_;
  std::lock_guard<std::mutex> lock(control->mutex);
  if (control->active == 0) return false;
  --control->active;
  if (!control->retired || control->active != 0 || control->cleanup_claimed)
    return false;
  control->cleanup_claimed = true;
  return true;
}

ReceiverAdmission::RetirementHandle ReceiverAdmission::RetainRetirement() const {
  return RetirementHandle(control_);
}

bool ReceiverAdmission::CompleteCleanup() {
  const auto control = control_;
  {
    std::lock_guard<std::mutex> lock(control->mutex);
    if (!control->retired || control->active != 0 ||
        !control->cleanup_claimed || control->cleanup_completed) return false;
    control->cleanup_completed = true;
  }
  NotifyQuiescence(control);
  return true;
}

bool ReceiverAdmission::RetirementHandle::IsQuiescent() const {
  const auto control = control_;
  if (control == nullptr) return false;
  std::lock_guard<std::mutex> lock(control->mutex);
  return control->retired && control->active == 0 && control->cleanup_completed;
}

bool ReceiverAdmission::RetirementHandle::SetQuiescenceNotification(
    void (*notify)(void*) noexcept, std::weak_ptr<void> context) {
  const auto control = control_;
  const auto owner = context.lock();
  if (control == nullptr || notify == nullptr || owner == nullptr) return false;
  {
    std::lock_guard<std::mutex> lock(control->mutex);
    if (control->notify != nullptr && !control->context.expired()) return false;
    control->notify = notify;
    control->context = std::move(context);
  }
  ReceiverAdmission::NotifyQuiescence(control);
  return true;
}

void ReceiverAdmission::NotifyQuiescence(const std::shared_ptr<Control>& control) {
  std::shared_ptr<void> owner;
  void (*notify)(void*) noexcept = nullptr;
  {
    std::lock_guard<std::mutex> lock(control->mutex);
    if (!control->cleanup_completed || control->notified ||
        control->notify == nullptr) return;
    owner = control->context.lock();
    if (owner == nullptr) return;
    notify = control->notify;
    control->notified = true;
  }
  notify(owner.get());
}

}  // namespace darwin_art::input
