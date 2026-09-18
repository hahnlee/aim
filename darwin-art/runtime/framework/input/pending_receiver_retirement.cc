#include "pending_receiver_retirement.h"

#include "receiver_retirement_driver.h"

#include <mutex>
#include <stdexcept>
#include <utility>

namespace darwin_art::input {

struct PendingReceiverRetirement {
  PendingReceiverRetirement(const ReceiverRoutingLifecycle& owner,
                           ReceiverAdmission::RetirementHandle admission,
                           ReceiverEndpointBinding::RetirementHandle binding,
                           std::shared_ptr<InputTransport> transport)
      : lifecycle(owner.Retain()),
        barrier(lifecycle, admission, binding, std::move(transport)),
        admission(std::move(admission)), binding(std::move(binding)) {}
  ReceiverRoutingLifecycle lifecycle;
  ReceiverRetirementBarrier barrier;
  ReceiverAdmission::RetirementHandle admission;
  ReceiverEndpointBinding::RetirementHandle binding;
  ReceiverRetirementDriverHandle driver;
  // Protected only by the retention-list mutex. No callback/provider owns it.
  bool enlisted_once = false;
  bool retained = false;
  PendingReceiverRetirementHandle next;
};

namespace {
struct RetentionList {
  std::mutex mutex;
  PendingReceiverRetirementHandle head;
  uint64_t revision = 0;
};
RetentionList& ProcessRetentionList() {
  // Initialized during Prepare, never implicitly destroyed after VM/provider
  // teardown. APK process exit is OS-owned; reusable VM shutdown must explicitly
  // drain these records through its retirement coordinator before teardown.
  static auto* list = new RetentionList;
  return *list;
}
}

PendingReceiverRetirementHandle PreparePendingReceiverRetirement(
    const ReceiverRoutingLifecycle& lifecycle,
    ReceiverAdmission::RetirementHandle admission,
    ReceiverEndpointBinding::RetirementHandle binding,
    std::shared_ptr<InputTransport> original_transport) {
  (void)ProcessRetentionList();
  return std::make_shared<PendingReceiverRetirement>(
      lifecycle, std::move(admission), std::move(binding), std::move(original_transport));
}

bool AttachPendingReceiverRetirementDriver(
    const PendingReceiverRetirementHandle& record,
    ReceiverRetirementDriverHandle driver) {
  if (record == nullptr || driver == nullptr) return false;
  auto& list = ProcessRetentionList();
  std::lock_guard lock(list.mutex);
  if (record->enlisted_once || record->driver != nullptr) return false;
  record->driver = std::move(driver);
  return true;
}

bool SubscribePendingReceiverRetirementQuiescence(
    const PendingReceiverRetirementHandle& record,
    PendingReceiverRetirementNotify notify, std::weak_ptr<void> context) {
  if (record == nullptr || notify == nullptr || context.expired()) return false;
  // Both handles are prepared before enlistment. Each owner invokes the weak
  // callback outside its own mutex; an immediate completion is safe here.
  return record->admission.SetQuiescenceNotification(notify, context) &&
         record->binding.SetQuiescenceNotification(notify, std::move(context));
}

bool RequestPendingReceiverRetirementProgress(
    const PendingReceiverRetirementHandle& record) {
  if (record == nullptr) return false;
  ReceiverRetirementDriverHandle driver;
  {
    std::lock_guard lock(ProcessRetentionList().mutex);
    if (!record->retained) return false;
    driver = record->driver;
  }
  return driver != nullptr && driver->Request();
}

bool EnlistPendingReceiverRetirement(const PendingReceiverRetirementHandle& record) {
  if (record == nullptr) return false;
  const auto pin = record;
  auto& list = ProcessRetentionList();
  std::lock_guard lock(list.mutex);
  if (pin->enlisted_once) return false;
  pin->enlisted_once = true;
  pin->retained = true;
  pin->next = std::move(list.head);
  list.head = pin;
  ++list.revision;
  return true;
}

ReceiverRoutingCloseStatus ClosePendingReceiverRetirement(
    const PendingReceiverRetirementHandle& record) {
  const auto pin = record;
  if (pin == nullptr) throw std::invalid_argument("null receiver retirement record");
  return pin->lifecycle.Close();
}

ReceiverRoutingPublishResult PublishPendingReceiverRetirement(
    const PendingReceiverRetirementHandle& record, ReceiverId expected_current_id) {
  const auto pin = record;
  if (pin == nullptr || !IsPendingReceiverRetirementRetained(pin))
    throw std::logic_error("receiver publication requires retained retirement ownership");
  return pin->lifecycle.Publish(expected_current_id);
}

ReceiverRetirementBarrierQuery PollPendingReceiverRetirement(
    const PendingReceiverRetirementHandle& record) {
  const auto pin = record;
  if (pin == nullptr) return {.status = ReceiverRetirementBarrierStatus::kInvalid};
  return pin->barrier.Poll();
}

bool IsPendingReceiverRetirementRetained(const PendingReceiverRetirementHandle& record) {
  const auto pin = record;
  if (pin == nullptr) return false;
  std::lock_guard lock(ProcessRetentionList().mutex);
  return pin->retained;
}

PendingReceiverRetirementScan VisitPendingReceiverRetirements(
    size_t budget, void (*visitor)(void*, const PendingReceiverRetirementHandle&),
    void* context, const PendingReceiverRetirementScan* resume) {
  if (visitor == nullptr) throw std::invalid_argument("null retirement visitor");
  auto& list = ProcessRetentionList();
  PendingReceiverRetirementHandle current;
  uint64_t revision;
  {
    std::lock_guard lock(list.mutex);
    revision = list.revision;
    current = resume != nullptr && !resume->membership_changed &&
                      resume->membership_revision == revision
                  ? resume->continuation : list.head;
  }
  PendingReceiverRetirementScan result;
  result.membership_revision = revision;
  while (current != nullptr && result.visited < budget) {
    PendingReceiverRetirementHandle next;
    {
      std::lock_guard lock(list.mutex);
      next = current->next;
    }
    visitor(context, current);
    ++result.visited;
    current = std::move(next);  // Final release outside the retention mutex.
  }
  result.budget_exhausted = current != nullptr;
  result.continuation = current;
  {
    std::lock_guard lock(list.mutex);
    result.membership_changed = revision != list.revision;
  }
  return result;
}

bool ReleaseSettledReceiverRetirement(const PendingReceiverRetirementHandle& record) {
  const auto pin = record;
  if (pin == nullptr || pin->barrier.Poll().status != ReceiverRetirementBarrierStatus::kReady)
    return false;
  auto& list = ProcessRetentionList();
  ReceiverRetirementDriverHandle retired_driver;
  {
    std::lock_guard lock(list.mutex);
    if (!pin->retained) return false;
    auto* link = &list.head;
    while (*link != nullptr && *link != pin) link = &(*link)->next;
    if (*link == nullptr) return false;
    // pin retains the removed node; its next moves into the list rather than
    // releasing a last owner. Any final destruction happens after unlocking.
    *link = std::move(pin->next);
    pin->retained = false;
    retired_driver = std::move(pin->driver);
    ++list.revision;
  }
  retired_driver.reset();
  return true;
}

}  // namespace darwin_art::input
