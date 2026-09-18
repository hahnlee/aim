#include "transport_registration_authority.h"

#include <algorithm>
#include <deque>
#include <mutex>
#include <new>
#include <unordered_map>
#include <utility>

namespace darwin_art::input {

struct TransportRegistrationAuthority::Entry {
  int fd = -1;
  TransportRegistrationRole role = TransportRegistrationRole::kReceiver;
  std::uint64_t identity = 0;
  void* looper = nullptr;
  bool ready = false;
  bool admitted = false;
  bool released = false;
  bool abandoned = false;
  std::uint64_t revision = 0;
  std::uint64_t notified_revision = 0;
  void (*notify)(void*) noexcept = nullptr;
  std::weak_ptr<void> context;
  void (*receiver_waiting)(void*) noexcept = nullptr;
  std::weak_ptr<void> receiver_waiting_context;
};
struct TransportRegistrationAuthority::Control {
  struct Lane {
    std::shared_ptr<Entry> owner;
    std::deque<std::shared_ptr<Entry>> waiting;
  };
  std::mutex mutex;
  std::unordered_map<int, Lane> lanes;
};

TransportRegistrationAuthority::TransportRegistrationAuthority()
    : control_(std::make_shared<Control>()) {}

TransportRegistrationResult TransportRegistrationAuthority::Reserve(
    int fd, TransportRegistrationRole role, std::uint64_t identity,
    void* looper, Claim* claim) const {
  if (fd < 0 || identity == 0 || looper == nullptr || claim == nullptr)
    return TransportRegistrationResult::kInvalid;
  if (claim->entry_ != nullptr) return TransportRegistrationResult::kConflict;
  const auto control = control_;
  std::shared_ptr<Entry> entry;
  std::shared_ptr<Entry> yield_owner;
  TransportRegistrationResult result = TransportRegistrationResult::kDeferred;
  try {
    entry = std::make_shared<Entry>();
    entry->fd = fd;
    entry->role = role;
    entry->identity = identity;
    entry->looper = looper;
    std::lock_guard<std::mutex> lock(control->mutex);
    auto& lane = control->lanes[fd];
    const auto matches = [&](const std::shared_ptr<Entry>& old) {
      return old != nullptr && old->identity == identity;
    };
    if (matches(lane.owner) ||
        std::any_of(lane.waiting.begin(), lane.waiting.end(), matches))
      return TransportRegistrationResult::kConflict;
    if (lane.owner == nullptr) {
      entry->ready = true;
      ++entry->revision;
      lane.owner = entry;
    } else if (role == TransportRegistrationRole::kReceiver &&
               lane.owner->role == TransportRegistrationRole::kRetiredOutput &&
               !lane.owner->admitted) {
      // Commit waiter allocation before changing ownership. A stale retired
      // readiness notification cannot subsequently pass BeginRegistration.
      lane.waiting.push_back(lane.owner);
      lane.owner->ready = false;
      entry->ready = true;
      ++entry->revision;
      lane.owner = entry;
    } else {
      lane.waiting.push_back(entry);
      if (role == TransportRegistrationRole::kReceiver &&
          lane.owner->role == TransportRegistrationRole::kRetiredOutput &&
          lane.owner->admitted)
        yield_owner = lane.owner;
    }
    result = entry->ready ? TransportRegistrationResult::kAcquired
                         : TransportRegistrationResult::kDeferred;
  } catch (const std::bad_alloc&) {
    return TransportRegistrationResult::kOutOfMemory;
  }
  claim->control_ = control;
  claim->entry_ = entry;
  // Caller intent is fully published before a reentrant yield notification.
  if (yield_owner != nullptr) NotifyReceiverWaiting(control, yield_owner);
  return result;
}

bool TransportRegistrationAuthority::HasLiveIntent(int fd) const {
  const auto control = control_;
  std::lock_guard<std::mutex> lock(control->mutex);
  const auto found = control->lanes.find(fd);
  if (found == control->lanes.end()) return false;
  const auto live = [](const std::shared_ptr<Entry>& entry) {
    return entry != nullptr && entry->role == TransportRegistrationRole::kReceiver;
  };
  return live(found->second.owner) || std::any_of(
      found->second.waiting.begin(), found->second.waiting.end(), live);
}

bool TransportRegistrationAuthority::HasAdmittedRegistration(int fd) const {
  const auto control = control_;
  std::lock_guard<std::mutex> lock(control->mutex);
  const auto found = control->lanes.find(fd);
  return found != control->lanes.end() && found->second.owner != nullptr &&
         found->second.owner->admitted;
}

TransportRegistrationAuthority::Claim::~Claim() { Abandon(); }
TransportRegistrationAuthority::Claim::Claim(Claim&& other) noexcept
    : control_(std::move(other.control_)), entry_(std::move(other.entry_)) {}
TransportRegistrationAuthority::Claim&
TransportRegistrationAuthority::Claim::operator=(Claim&& other) noexcept {
  Claim previous;
  if (this != &other) {
    previous.control_ = std::move(control_);
    previous.entry_ = std::move(entry_);
    control_ = std::move(other.control_);
    entry_ = std::move(other.entry_);
  }
  return *this;
}
void TransportRegistrationAuthority::Claim::Abandon() noexcept {
  const auto control = std::move(control_);
  const auto entry = std::move(entry_);
  if (control != nullptr && entry != nullptr) (void)Release(control, entry, false);
}
TransportRegistrationResult
TransportRegistrationAuthority::Claim::BeginRegistration() {
  const auto control = control_;
  const auto entry = entry_;
  if (control == nullptr || entry == nullptr)
    return TransportRegistrationResult::kInvalid;
  std::lock_guard<std::mutex> lock(control->mutex);
  if (entry->released || entry->admitted) return TransportRegistrationResult::kConflict;
  if (!entry->ready) return TransportRegistrationResult::kDeferred;
  entry->admitted = true;
  return TransportRegistrationResult::kAcquired;
}
bool TransportRegistrationAuthority::Claim::ReleaseAfterQuiescence() {
  const auto control = std::move(control_);
  const auto entry = std::move(entry_);
  if (control == nullptr || entry == nullptr) return false;
  const bool released = Release(control, entry, true);
  if (!released) {
    control_ = control;
    entry_ = entry;
  }
  return released;
}

bool TransportRegistrationAuthority::Release(const std::shared_ptr<Control>& control,
    const std::shared_ptr<Entry>& entry, bool quiescent) {
  std::shared_ptr<Entry> promoted;
  {
    std::lock_guard<std::mutex> lock(control->mutex);
    if (entry->released) return true;
    if (entry->admitted && !quiescent) {
      entry->abandoned = true; // Quarantine: never replace a possibly live FD.
      return false;
    }
    const auto found = control->lanes.find(entry->fd);
    if (found == control->lanes.end()) return false;
    auto& lane = found->second;
    if (lane.owner == entry) {
      lane.owner.reset();
      if (!lane.waiting.empty()) {
        auto next = std::find_if(lane.waiting.begin(), lane.waiting.end(),
            [](const std::shared_ptr<Entry>& candidate) {
              return candidate->role == TransportRegistrationRole::kReceiver;
            });
        if (next == lane.waiting.end()) next = lane.waiting.begin();
        promoted = *next;
        lane.waiting.erase(next);
        promoted->ready = true;
        ++promoted->revision;
        lane.owner = promoted;
      }
    } else {
      const auto waiter = std::find(lane.waiting.begin(), lane.waiting.end(), entry);
      if (waiter == lane.waiting.end()) return false;
      lane.waiting.erase(waiter);
    }
    entry->released = true;
    entry->ready = false;
    if (lane.owner == nullptr && lane.waiting.empty()) control->lanes.erase(found);
  }
  if (promoted != nullptr) Notify(control, promoted);
  return true;
}

bool TransportRegistrationAuthority::Claim::SubscribeAvailable(
    void (*notify)(void*) noexcept, std::weak_ptr<void> context) {
  const auto control = control_;
  const auto entry = entry_;
  const auto owner = context.lock();
  if (control == nullptr || entry == nullptr || notify == nullptr || owner == nullptr)
    return false;
  {
    std::lock_guard<std::mutex> lock(control->mutex);
    if (entry->released || (entry->notify != nullptr && !entry->context.expired()))
      return false;
    entry->notify = notify;
    entry->context = std::move(context);
  }
  Notify(control, entry);
  return true;
}
void TransportRegistrationAuthority::Notify(const std::shared_ptr<Control>& control,
    const std::shared_ptr<Entry>& entry) {
  std::shared_ptr<void> owner;
  void (*notify)(void*) noexcept = nullptr;
  {
    std::lock_guard<std::mutex> lock(control->mutex);
    if (!entry->ready || entry->released || entry->notify == nullptr ||
        entry->notified_revision == entry->revision) return;
    owner = entry->context.lock();
    if (owner == nullptr) return;
    entry->notified_revision = entry->revision;
    notify = entry->notify;
  }
  notify(owner.get());
}

bool TransportRegistrationAuthority::Claim::SubscribeReceiverWaiting(
    void (*notify)(void*) noexcept, std::weak_ptr<void> context) {
  const auto control = control_;
  const auto entry = entry_;
  const auto owner = context.lock();
  if (control == nullptr || entry == nullptr || notify == nullptr || owner == nullptr)
    return false;
  std::weak_ptr<void> previous;
  {
    std::lock_guard lock(control->mutex);
    if (entry->released || entry->role != TransportRegistrationRole::kRetiredOutput)
      return false;
    previous = std::move(entry->receiver_waiting_context);
    entry->receiver_waiting = notify;
    entry->receiver_waiting_context = std::move(context);
  }
  NotifyReceiverWaiting(control, entry);
  return true;
}

void TransportRegistrationAuthority::NotifyReceiverWaiting(
    const std::shared_ptr<Control>& control, const std::shared_ptr<Entry>& entry) {
  std::shared_ptr<void> owner;
  void (*notify)(void*) noexcept = nullptr;
  {
    std::lock_guard lock(control->mutex);
    if (entry->released || !entry->admitted ||
        entry->role != TransportRegistrationRole::kRetiredOutput ||
        entry->receiver_waiting == nullptr)
      return;
    const auto lane = control->lanes.find(entry->fd);
    if (lane == control->lanes.end() || lane->second.owner != entry ||
        !std::any_of(lane->second.waiting.begin(), lane->second.waiting.end(),
          [](const std::shared_ptr<Entry>& candidate) {
            return candidate->role == TransportRegistrationRole::kReceiver;
          }))
      return;
    owner = entry->receiver_waiting_context.lock();
    if (owner == nullptr) return;
    notify = entry->receiver_waiting;
  }
  notify(owner.get());
}

}  // namespace darwin_art::input
