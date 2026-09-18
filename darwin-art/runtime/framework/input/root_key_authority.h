#pragma once

#include <cstdint>
#include <memory>
#include <mutex>

#include "../../../compat/binder/endpoint_lifetime.h"
#include "../../../compat/window/desktop_root_events.h"
#include "input_routing_domain.h"

namespace darwin_art::looper { class ReusableLooperTask; }

namespace darwin_art::input {

// An immutable WMS decision copied before entering any input/domain lock.  It
// contains policy facts only; it owns no JNI, Binder, FD, or routing resource.
struct RootKeyDecisionRecord final {
  uint64_t incarnation = 0;
  uint64_t fact_serial = 0;
  uint64_t sequence = 0;
  uint64_t epoch = 0;
  bool selection_present = false;
};
using DecisionRecord = RootKeyDecisionRecord;
using RootKeyDecisionRecordHandle = std::shared_ptr<const RootKeyDecisionRecord>;
using DecisionRecordHandle = RootKeyDecisionRecordHandle;

// Read only under the domain -> authority guard. Pending keys bind the
// activation interval before a decision exists, then one immutable record.
struct RootKeyIntent final {
  darwin_art::window::DesktopRootEvents::LocalStamp stamp{};
  DecisionRecordHandle decision;
  bool closed = true;
  bool server_live = false;
};

// Positive Java-facing sequence/epoch values are bounded before publication;
// incarnation and fact serial remain unsigned identities and may use high bits.
RootKeyDecisionRecordHandle CreateRootKeyDecisionRecord(
    uint64_t incarnation, uint64_t fact_serial, uint64_t sequence,
    uint64_t epoch, bool selection_present) noexcept;
inline DecisionRecordHandle CreateDecisionRecord(
    uint64_t incarnation, uint64_t fact_serial, uint64_t sequence,
    uint64_t epoch, bool selection_present) noexcept {
  return CreateRootKeyDecisionRecord(incarnation, fact_serial, sequence,
                                     epoch, selection_present);
}

class RootKeyAuthority;
using RootKeyAuthorityHandle = std::shared_ptr<RootKeyAuthority>;

// This carry object keeps the authority facade and routing owner weak so a
// queued key cannot create an authority↔routing lifetime cycle. The exact
// endpoint lifetime is strong identity state and may own transport resources;
// callers release it only after leaving authority/domain locks.
class RootKeyAuthorityTicket final {
 public:
  RootKeyAuthorityTicket() noexcept = default;
  bool valid() const noexcept { return !authority_.expired() && decision_ != nullptr; }
  explicit operator bool() const noexcept { return valid(); }
  RootKeyAuthorityHandle Authority() const noexcept;
  DecisionRecordHandle Decision() const noexcept { return decision_; }
  InputRoutingHandle Routing() const noexcept { return routing_.lock(); }
  std::shared_ptr<darwin_art::binder::EndpointLifetime> Wire() const noexcept {
    return wire_;
  }
  uint64_t RootActivationRevision() const noexcept { return root_activation_revision_; }
  uint64_t RootPublicationSerial() const noexcept { return root_publication_serial_; }
  uint64_t RootStampRevision() const noexcept { return root_stamp_revision_; }
  uint64_t DecisionEpoch() const noexcept { return decision_epoch_; }
  uint64_t WireGeneration() const noexcept { return wire_generation_; }
  uint64_t OwnerRevision() const noexcept { return owner_revision_; }

 private:
  friend class RootKeyAuthority;
  friend class RootKeyAuthorityGuard;
  std::weak_ptr<RootKeyAuthority> authority_;
  DecisionRecordHandle decision_;
  std::weak_ptr<InputRoutingState> routing_;
  std::shared_ptr<darwin_art::binder::EndpointLifetime> wire_;
  uint64_t root_activation_revision_ = 0;
  uint64_t root_publication_serial_ = 0;
  uint64_t root_stamp_revision_ = 0;
  uint64_t decision_epoch_ = 0;
  uint64_t wire_generation_ = 0;
  uint64_t owner_revision_ = 0;
};
using RootKeyAuthoritySnapshot = RootKeyAuthorityTicket;

// The guard carries no strong authority reference.  The caller must retain a
// RootKeyAuthorityHandle for the whole domain transaction and construct this
// guard after acquiring that transaction (domain -> authority lock order).
class RootKeyAuthorityGuard final {
 public:
  RootKeyAuthorityGuard(const RootKeyAuthorityGuard&) = delete;
  RootKeyAuthorityGuard& operator=(const RootKeyAuthorityGuard&) = delete;
  RootKeyAuthorityGuard(RootKeyAuthorityGuard&&) noexcept;
  RootKeyAuthorityGuard& operator=(RootKeyAuthorityGuard&&) noexcept;
  ~RootKeyAuthorityGuard();
  RootKeyAuthorityTicket TrySnapshot() const noexcept;
  RootKeyIntent SnapshotKeyIntent() const noexcept;
  bool ValidateTicket(const RootKeyAuthorityTicket& ticket) const noexcept;

 private:
  friend class RootKeyAuthority;
  friend RootKeyAuthorityGuard LockRootKeyAuthorityForRouting(
      InputRoutingDomainTransaction&, RootKeyAuthority&) noexcept;
  RootKeyAuthorityGuard(InputRoutingDomainTransaction& domain,
                        RootKeyAuthority& authority) noexcept;
  RootKeyAuthority* authority_ = nullptr;
  std::unique_lock<std::mutex> lock_;
};

enum class RootKeyAuthorityAcquireStatus : uint8_t {
  kAcquired,
  kUnavailable,       // another facade is constructing its root subscription
  kClosed,
  kAllocationFailed,
  kInvalidRoot,
};

// The exact root is the catalog key.  The catalog owns only an inert control
// record and weak references; it never retains an NSWindow or observer facade.
RootKeyAuthorityHandle AcquireRootKeyAuthority(
    const std::shared_ptr<darwin_art::window::DesktopRootEvents>& root,
    RootKeyAuthorityAcquireStatus* status) noexcept;
inline RootKeyAuthorityHandle AcquireRootKeyAuthority(
    const std::shared_ptr<darwin_art::window::DesktopRootEvents>& root) noexcept {
  return AcquireRootKeyAuthority(root, nullptr);
}

RootKeyAuthorityGuard LockRootKeyAuthorityForRouting(
    InputRoutingDomainTransaction& domain, RootKeyAuthority& authority) noexcept;

class RootKeyAuthority final
    : public darwin_art::window::DesktopRootEvents::StampObserver,
      public std::enable_shared_from_this<RootKeyAuthority> {
 public:
  struct Control;
  ~RootKeyAuthority() override;
  RootKeyAuthority(const RootKeyAuthority&) = delete;
  RootKeyAuthority& operator=(const RootKeyAuthority&) = delete;

  uint64_t Incarnation() const noexcept;
  bool Closed() const noexcept;
  // Fixed asynchronous signal only: the prepared task context must contain
  // weak ingress state and no arbitrary quiescence callback/resource owner.
  // Repeated same-task publication is idempotent; conflicting live tasks fail.
  bool SubscribeProgressTask(
      const std::shared_ptr<darwin_art::looper::ReusableLooperTask>&) noexcept;
  void RequestProgress() const noexcept;

  // Attaches one exact server lifetime.  Repeating the same owner/generation
  // is idempotent; a different owner or generation is rejected.
  bool AttachServer(
      const std::shared_ptr<darwin_art::binder::EndpointLifetime>& owner,
      uint64_t generation) noexcept;

  // Publication is monotonic by sequence. A newer record immediately clears
  // any prior resolved routing, including when the new record is unresolved.
  bool PublishDecision(const DecisionRecordHandle& record) noexcept;
  // Resolution is allowed only for the current record and exactly one route;
  // retrying the same record/route is idempotent, while A -> B is rejected.
  bool ResolveDecision(const DecisionRecordHandle& record,
                       const InputRoutingHandle& routing) noexcept;

  void OnLocalStamp(
      darwin_art::window::DesktopRootEvents::LocalStamp stamp) noexcept override;
  bool Close() noexcept;

 private:
  friend class RootKeyAuthorityGuard;
  friend class RootKeyAuthorityTestPeer;
  friend RootKeyAuthorityHandle AcquireRootKeyAuthority(
      const std::shared_ptr<darwin_art::window::DesktopRootEvents>&,
      RootKeyAuthorityAcquireStatus*) noexcept;

  explicit RootKeyAuthority(
      const std::shared_ptr<darwin_art::window::DesktopRootEvents>& root,
      std::shared_ptr<Control> control) noexcept;
  bool Initialize() noexcept;
  RootKeyAuthorityTicket TrySnapshotLocked() noexcept;
  bool ValidateTicketLocked(const RootKeyAuthorityTicket& ticket) const noexcept;
  std::weak_ptr<darwin_art::looper::ReusableLooperTask> ProgressTask() const noexcept;

  std::shared_ptr<Control> control_;
  // The catalog retains only a weak facade. Keeping the original provider
  // here therefore creates no catalog/resource cycle and guarantees the hook
  // remains attached to a live root throughout a pinned admission.
  std::shared_ptr<darwin_art::window::DesktopRootEvents> root_;
};

}  // namespace darwin_art::input
