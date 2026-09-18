#include "root_key_authority.h"
#include "../../../compat/looper/android_looper_owner.h"

#include <limits>
#include <unordered_map>
#include <utility>
#include <vector>

namespace darwin_art::input {
namespace {

constexpr uint64_t kJavaPositiveMax =
    static_cast<uint64_t>(std::numeric_limits<int64_t>::max());
constexpr uint64_t kCounterMax = std::numeric_limits<uint64_t>::max();

struct ProgressSignal final {
  std::weak_ptr<darwin_art::looper::ReusableLooperTask> task;
  ~ProgressSignal() {
    // Request is allocation-free and cannot dispatch policy inline. The only
    // retained task context is weak metadata; no provider/JNI release tail.
    if (const auto pin = task.lock()) (void)pin->Request();
  }
};

bool ValidDecision(const DecisionRecordHandle& record) noexcept {
  if (!record || record->incarnation == 0 || record->sequence == 0 ||
      record->sequence > kJavaPositiveMax ||
      record->epoch > kJavaPositiveMax ||
      (record->selection_present && record->epoch == 0))
    return false;
  return !record->selection_present || record->fact_serial != 0;
}

template <typename T, typename U>
bool SameWeakOwner(const std::weak_ptr<T>& left,
                  const std::weak_ptr<U>& right) noexcept {
  return !left.owner_before(right) && !right.owner_before(left);
}

}  // namespace

struct RootKeyAuthority::Control final {
  explicit Control(uint64_t incarnation) noexcept {
    stamp.incarnation = incarnation;
  }
  mutable std::mutex mutex;
  std::weak_ptr<darwin_art::window::DesktopRootEvents> root;
  bool closed = false;
  bool initialized = false;
  bool stamp_seen = false;
  darwin_art::window::DesktopRootEvents::LocalStamp stamp{};
  uint64_t owner_revision = 1;
  DecisionRecordHandle decision;
  std::weak_ptr<InputRoutingState> routing;
  bool resolved = false;
  std::shared_ptr<darwin_art::binder::EndpointLifetime> server;
  uint64_t server_generation = 0;
  std::weak_ptr<darwin_art::looper::ReusableLooperTask> progress;
};

namespace {

struct CatalogEntry final {
  std::weak_ptr<darwin_art::window::DesktopRootEvents> root;
  std::shared_ptr<RootKeyAuthority::Control> control;
  std::weak_ptr<RootKeyAuthority> facade;
  bool constructing = false;
};

std::mutex g_catalog_mutex;
std::unordered_map<darwin_art::window::DesktopRootEvents*, CatalogEntry>
    g_catalog;

bool BumpOwnerRevisionLocked(RootKeyAuthority::Control& control) noexcept {
  if (control.owner_revision == kCounterMax) {
    control.closed = true;
    control.decision.reset();
    control.routing.reset();
    control.resolved = false;
    // The server is inert; leave its last strong reference to be destroyed
    // after this lock unwinds rather than invoking a resource destructor here.
    control.server_generation = 0;
    return false;
  }
  ++control.owner_revision;
  return true;
}

}  // namespace

DecisionRecordHandle CreateRootKeyDecisionRecord(
    uint64_t incarnation, uint64_t fact_serial, uint64_t sequence,
    uint64_t epoch, bool selection_present) noexcept {
  if (incarnation == 0 || sequence == 0 || sequence > kJavaPositiveMax ||
      epoch > kJavaPositiveMax || (selection_present && epoch == 0) ||
      (selection_present && fact_serial == 0))
    return {};
  try {
    return std::shared_ptr<const RootKeyDecisionRecord>(
        new RootKeyDecisionRecord{incarnation, fact_serial, sequence, epoch,
                                  selection_present});
  } catch (...) {
    return {};
  }
}

RootKeyAuthorityHandle AcquireRootKeyAuthority(
    const std::shared_ptr<darwin_art::window::DesktopRootEvents>& root,
    RootKeyAuthorityAcquireStatus* status) noexcept {
  auto set_status = [status](RootKeyAuthorityAcquireStatus value) noexcept {
    if (status != nullptr) *status = value;
  };
  set_status(RootKeyAuthorityAcquireStatus::kInvalidRoot);
  if (!root) return {};

  const auto key = root.get();
  RootKeyAuthorityHandle facade;
  std::shared_ptr<RootKeyAuthority::Control> control;
  // All erased weak/strong owners, including their control blocks, unwind
  // outside the catalog lock even if a later allocation fails.
  std::vector<CatalogEntry> retired_entries;
  {
    std::lock_guard<std::mutex> lock(g_catalog_mutex);
    try {
      retired_entries.reserve(g_catalog.size());
      for (auto scan = g_catalog.begin(); scan != g_catalog.end();) {
        if (scan->second.root.expired()) {
          retired_entries.push_back(std::move(scan->second));
          scan = g_catalog.erase(scan);
        } else {
          ++scan;
        }
      }
    } catch (const std::bad_alloc&) {
      set_status(RootKeyAuthorityAcquireStatus::kAllocationFailed);
      return {};
    }
    auto it = g_catalog.find(key);
    if (it != g_catalog.end()) {
      const std::weak_ptr<darwin_art::window::DesktopRootEvents> requested = root;
      if (!SameWeakOwner(it->second.root, requested)) {
        set_status(RootKeyAuthorityAcquireStatus::kInvalidRoot);
        return {};
      }
      {
        std::lock_guard<std::mutex> control_lock(it->second.control->mutex);
        if (it->second.control->closed) {
          set_status(RootKeyAuthorityAcquireStatus::kClosed);
          return {};
        }
        control = it->second.control;
        if (it->second.constructing) {
          // Subscribe invokes a synchronous, reentrant initial hook. Never
          // expose the unpublished facade or wait on its own construction.
          set_status(RootKeyAuthorityAcquireStatus::kUnavailable);
          return {};
        }
        facade = it->second.facade.lock();
        if (facade) {
          if (!control->initialized) {
            set_status(RootKeyAuthorityAcquireStatus::kUnavailable);
            return {};
          }
          set_status(RootKeyAuthorityAcquireStatus::kAcquired);
          return facade;
        }
      }
    }

    if (!control) {
      try {
        control = std::make_shared<RootKeyAuthority::Control>(root->incarnation());
      } catch (...) {
        set_status(RootKeyAuthorityAcquireStatus::kAllocationFailed);
        return {};
      }
      control->root = root;
    }
    try {
      facade = RootKeyAuthorityHandle(new RootKeyAuthority(root, control));
    } catch (...) {
      set_status(RootKeyAuthorityAcquireStatus::kAllocationFailed);
      return {};
    }
    try {
      auto slot = g_catalog.find(key);
      if (slot == g_catalog.end()) {
        g_catalog.emplace(key, CatalogEntry{root, control, facade, true});
      } else {
        slot->second.facade = facade;
        slot->second.constructing = true;
      }
    } catch (...) {
      set_status(RootKeyAuthorityAcquireStatus::kAllocationFailed);
      return {};
    }
    set_status(RootKeyAuthorityAcquireStatus::kUnavailable);
  }
  // No observer callback may run under g_catalog_mutex.  The weak facade is
  // reserved in the catalog, so reentrant Acquire returns unavailable.
  const bool initialized = facade->Initialize();
  {
    std::lock_guard<std::mutex> lock(g_catalog_mutex);
    auto it = g_catalog.find(key);
    const std::weak_ptr<RootKeyAuthority> expected = facade;
    if (it == g_catalog.end() || it->second.control != control ||
        !it->second.constructing ||
        !SameWeakOwner(it->second.facade, expected)) {
      set_status(RootKeyAuthorityAcquireStatus::kUnavailable);
      return {};
    }
    std::lock_guard<std::mutex> control_lock(control->mutex);
    it->second.constructing = false;
    if (!initialized || control->closed) {
      // Preserve closure/highwater/frozen identity even after publication or
      // subscription failure. Facade destruction alone must not seal Control.
      it->second.facade.reset();
      set_status(control->closed ? RootKeyAuthorityAcquireStatus::kClosed
                                 : RootKeyAuthorityAcquireStatus::kUnavailable);
      return {};
    }
  }
  set_status(RootKeyAuthorityAcquireStatus::kAcquired);
  return facade;
}

RootKeyAuthority::RootKeyAuthority(
    const std::shared_ptr<darwin_art::window::DesktopRootEvents>& root,
    std::shared_ptr<Control> control) noexcept
    : control_(std::move(control)), root_(root) {}

RootKeyAuthority::~RootKeyAuthority() = default;

bool RootKeyAuthority::Initialize() noexcept {
  const auto root = root_;
  if (!root) return false;
  {
    auto domain = LockInputRoutingDomain();
    std::lock_guard<std::mutex> lock(control_->mutex);
    if (control_->closed) return false;
    control_->initialized = false;
  }
  try {
    if (!root->SubscribeStampObserver(shared_from_this())) return false;
  } catch (...) {
    return false;
  }
  auto domain = LockInputRoutingDomain();
  std::lock_guard<std::mutex> lock(control_->mutex);
  if (control_->closed) return true;
  control_->initialized = true;
  return true;
}

uint64_t RootKeyAuthority::Incarnation() const noexcept {
  std::lock_guard<std::mutex> lock(control_->mutex);
  return control_->stamp.incarnation;
}

bool RootKeyAuthority::Closed() const noexcept {
  std::lock_guard<std::mutex> lock(control_->mutex);
  return control_->closed;
}

std::weak_ptr<darwin_art::looper::ReusableLooperTask>
RootKeyAuthority::ProgressTask() const noexcept {
  std::lock_guard<std::mutex> lock(control_->mutex);
  return control_->progress;
}

bool RootKeyAuthority::SubscribeProgressTask(
    const std::shared_ptr<darwin_art::looper::ReusableLooperTask>& task) noexcept {
  if (!task) return false;
  const std::weak_ptr<darwin_art::looper::ReusableLooperTask> candidate = task;
  std::lock_guard<std::mutex> lock(control_->mutex);
  if (control_->closed || (!control_->progress.expired() &&
      !SameWeakOwner(control_->progress, candidate))) return false;
  control_->progress = candidate;
  return true;
}

void RootKeyAuthority::RequestProgress() const noexcept {
  const ProgressSignal signal{ProgressTask()};
}

bool RootKeyAuthority::AttachServer(
    const std::shared_ptr<darwin_art::binder::EndpointLifetime>& owner,
    uint64_t generation) noexcept {
  if (!owner || generation == 0 || !owner->Matches(generation)) return false;
  const ProgressSignal signal{ProgressTask()};
  auto domain = LockInputRoutingDomain();
  std::lock_guard<std::mutex> lock(control_->mutex);
  if (!control_->initialized || control_->closed || !owner->Matches(generation))
    return false;
  if (control_->server) {
    return control_->server.get() == owner.get() &&
           control_->server_generation == generation;
  }
  if (!BumpOwnerRevisionLocked(*control_)) return false;
  control_->server = owner;
  control_->server_generation = generation;
  return true;
}

bool RootKeyAuthority::PublishDecision(
    const DecisionRecordHandle& record) noexcept {
  if (!ValidDecision(record)) return false;
  const ProgressSignal signal{ProgressTask()};
  DecisionRecordHandle retired;
  auto domain = LockInputRoutingDomain();
  std::lock_guard<std::mutex> lock(control_->mutex);
  if (!control_->initialized || control_->closed ||
      !control_->server ||
      !control_->server->Matches(control_->server_generation) ||
      record->incarnation != control_->stamp.incarnation)
    return false;
  if (control_->decision) {
    if (record->sequence < control_->decision->sequence) return false;
    if (record->sequence == control_->decision->sequence)
      return record.get() == control_->decision.get();
  }
  if (!BumpOwnerRevisionLocked(*control_)) return false;
  retired = std::move(control_->decision);
  control_->decision = record;
  control_->routing.reset();
  control_->resolved = false;
  return true;
}

bool RootKeyAuthority::ResolveDecision(
    const DecisionRecordHandle& record,
    const InputRoutingHandle& routing) noexcept {
  if (!record || !routing) return false;
  const ProgressSignal signal{ProgressTask()};
  auto domain = LockInputRoutingDomain();
  std::lock_guard<std::mutex> lock(control_->mutex);
  if (!control_->initialized || control_->closed || !control_->decision ||
      !control_->server ||
      !control_->server->Matches(control_->server_generation) ||
      control_->decision.get() != record.get() || !record->selection_present)
    return false;
  if (control_->resolved) {
    // Once the weak route expired, never let a same-record retry install a new
    // routing owner.  A new decision is required to establish a new identity.
    const std::weak_ptr<InputRoutingState> requested = routing;
    return !control_->routing.expired() &&
           SameWeakOwner(control_->routing, requested);
  }
  if (!BumpOwnerRevisionLocked(*control_)) return false;
  control_->routing = routing;
  control_->resolved = true;
  return true;
}

void RootKeyAuthority::OnLocalStamp(
    darwin_art::window::DesktopRootEvents::LocalStamp stamp) noexcept {
  const ProgressSignal signal{ProgressTask()};
  DecisionRecordHandle retired;
  std::shared_ptr<darwin_art::binder::EndpointLifetime> retired_server;
  auto domain = LockInputRoutingDomain();
  std::lock_guard<std::mutex> lock(control_->mutex);
  if (stamp.incarnation != control_->stamp.incarnation || control_->closed)
    return;
  if (!stamp.closed && control_->stamp_seen &&
      stamp.stamp_revision <= control_->stamp.stamp_revision)
    return;
  if (!BumpOwnerRevisionLocked(*control_)) return;
  control_->stamp = stamp;
  control_->stamp_seen = true;
  if (stamp.closed) {
    control_->closed = true;
    retired = std::move(control_->decision);
    control_->routing.reset();
    control_->resolved = false;
    retired_server = std::move(control_->server);
    control_->server_generation = 0;
  }
}

bool RootKeyAuthority::Close() noexcept {
  const ProgressSignal signal{ProgressTask()};
  DecisionRecordHandle retired;
  std::shared_ptr<darwin_art::binder::EndpointLifetime> retired_server;
  auto domain = LockInputRoutingDomain();
  std::lock_guard<std::mutex> lock(control_->mutex);
  if (control_->closed) return false;
  control_->closed = true;
  retired = std::move(control_->decision);
  control_->routing.reset();
  control_->resolved = false;
  retired_server = std::move(control_->server);
  control_->server_generation = 0;
  return true;
}

RootKeyAuthorityTicket RootKeyAuthority::TrySnapshotLocked() noexcept {
  RootKeyAuthorityTicket ticket;
  if (!control_->initialized || control_->closed || !control_->stamp_seen ||
      !control_->stamp.key || !control_->decision || !control_->resolved ||
      !control_->decision->selection_present ||
      control_->decision->fact_serial == 0 ||
      control_->stamp.latest_emitted_serial != control_->decision->fact_serial ||
      !control_->server ||
      !control_->server->Matches(control_->server_generation))
    return ticket;
  if (control_->root.expired() || control_->routing.expired()) return ticket;
  ticket.authority_ = weak_from_this();
  ticket.decision_ = control_->decision;
  ticket.routing_ = control_->routing;
  ticket.wire_ = control_->server;
  ticket.root_activation_revision_ = control_->stamp.state_revision;
  ticket.root_publication_serial_ = control_->stamp.latest_emitted_serial;
  ticket.root_stamp_revision_ = control_->stamp.stamp_revision;
  ticket.decision_epoch_ = control_->decision->epoch;
  ticket.wire_generation_ = control_->server_generation;
  ticket.owner_revision_ = control_->owner_revision;
  return ticket;
}

bool RootKeyAuthority::ValidateTicketLocked(
    const RootKeyAuthorityTicket& ticket) const noexcept {
  const auto self = weak_from_this();
  if (!ticket.valid() || !SameWeakOwner(ticket.authority_, self) ||
      ticket.decision_.get() != control_->decision.get() ||
      !control_->resolved || control_->closed || !control_->stamp.key ||
      control_->root.expired() || control_->routing.expired() ||
      !control_->decision->selection_present ||
      control_->stamp.latest_emitted_serial != control_->decision->fact_serial ||
      ticket.owner_revision_ != control_->owner_revision ||
      ticket.root_activation_revision_ != control_->stamp.state_revision ||
      ticket.root_publication_serial_ != control_->stamp.latest_emitted_serial ||
      ticket.root_stamp_revision_ != control_->stamp.stamp_revision ||
      ticket.decision_epoch_ != control_->decision->epoch ||
      !control_->server || ticket.wire_.get() != control_->server.get() ||
      ticket.wire_generation_ != control_->server_generation ||
      !control_->server->Matches(control_->server_generation))
    return false;
  return SameWeakOwner(control_->routing, ticket.routing_);
}

RootKeyAuthorityHandle RootKeyAuthorityTicket::Authority() const noexcept {
  return authority_.lock();
}

RootKeyAuthorityGuard::RootKeyAuthorityGuard(
    InputRoutingDomainTransaction& domain, RootKeyAuthority& authority) noexcept
    : authority_(&authority), lock_(authority.control_->mutex) {
  (void)domain;
}

RootKeyAuthorityGuard::RootKeyAuthorityGuard(
    RootKeyAuthorityGuard&& other) noexcept = default;
RootKeyAuthorityGuard& RootKeyAuthorityGuard::operator=(
    RootKeyAuthorityGuard&& other) noexcept = default;
RootKeyAuthorityGuard::~RootKeyAuthorityGuard() = default;

RootKeyAuthorityTicket RootKeyAuthorityGuard::TrySnapshot() const noexcept {
  if (authority_ == nullptr || !lock_.owns_lock()) return {};
  return authority_->TrySnapshotLocked();
}

RootKeyIntent RootKeyAuthorityGuard::SnapshotKeyIntent() const noexcept {
  if (!authority_ || !lock_.owns_lock()) return {};
  const auto& control = *authority_->control_;
  RootKeyIntent intent;
  intent.stamp = control.stamp;
  intent.decision = control.decision;
  intent.closed = !control.initialized || control.closed || control.root.expired();
  intent.server_live = control.server &&
      control.server->Matches(control.server_generation);
  return intent;
}

bool RootKeyAuthorityGuard::ValidateTicket(
    const RootKeyAuthorityTicket& ticket) const noexcept {
  return authority_ != nullptr && lock_.owns_lock() &&
         authority_->ValidateTicketLocked(ticket);
}

RootKeyAuthorityGuard LockRootKeyAuthorityForRouting(
    InputRoutingDomainTransaction& domain, RootKeyAuthority& authority) noexcept {
  return RootKeyAuthorityGuard(domain, authority);
}

}  // namespace darwin_art::input
