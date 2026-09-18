#pragma once

#include "receiver_retirement_barrier.h"

namespace darwin_art::input {

struct PendingReceiverRetirement;
using PendingReceiverRetirementHandle = std::shared_ptr<PendingReceiverRetirement>;
class ReceiverRetirementDriver;
using ReceiverRetirementDriverHandle = std::shared_ptr<ReceiverRetirementDriver>;
using PendingReceiverRetirementNotify = void (*)(void*) noexcept;

// Prepare all allocating ownership BEFORE receiver visibility. The independent
// record contains no receiver, channel wrapper or JNI references.
PendingReceiverRetirementHandle PreparePendingReceiverRetirement(
    const ReceiverRoutingLifecycle& lifecycle,
    ReceiverAdmission::RetirementHandle admission,
    ReceiverEndpointBinding::RetirementHandle binding,
    std::shared_ptr<InputTransport> original_transport);
// One-shot typed owner attachment. Must happen after Prepare and before
// Enlist; the driver stores only a weak record back-reference, so this field
// does not form a cycle.
bool AttachPendingReceiverRetirementDriver(
    const PendingReceiverRetirementHandle& record,
    ReceiverRetirementDriverHandle driver);
// Installs weak completion hints on both resource gates before enlistment.
// The callback is invoked outside their locks and may be called immediately.
bool SubscribePendingReceiverRetirementQuiescence(
    const PendingReceiverRetirementHandle& record,
    PendingReceiverRetirementNotify notify, std::weak_ptr<void> context);
// Discovery/VM-shutdown path: request the attached driver without exposing
// the record's typed owner or holding the retention-list mutex across Request.
bool RequestPendingReceiverRetirementProgress(
    const PendingReceiverRetirementHandle& record);
// Single-use, allocation-free list publication. Enlist before registry exposure;
// the list retains deferred/failed closure even after the receiver disappears.
// The process-lifetime list is initialized in Prepare, not static-destroyed.
// Reusable VM shutdown must explicitly drain it before provider teardown.
bool EnlistPendingReceiverRetirement(const PendingReceiverRetirementHandle& record);
// Publication requires independently retained ownership, not a receiver member.
ReceiverRoutingPublishResult PublishPendingReceiverRetirement(
    const PendingReceiverRetirementHandle& record, ReceiverId expected_current_id = 0);
ReceiverRoutingCloseStatus ClosePendingReceiverRetirement(
    const PendingReceiverRetirementHandle& record);
ReceiverRetirementBarrierQuery PollPendingReceiverRetirement(
    const PendingReceiverRetirementHandle& record);
bool IsPendingReceiverRetirementRetained(const PendingReceiverRetirementHandle& record);
struct PendingReceiverRetirementScan {
  size_t visited = 0;
  bool budget_exhausted = false;
  bool membership_changed = false;
  uint64_t membership_revision = 0;
  PendingReceiverRetirementHandle continuation;
};
// Allocation-free after Prepare initializes the list. Pins next before calling
// visitor outside the list mutex. Concurrent removal may skip another node;
// membership_changed requires a new pass, never implies a complete snapshot.
// Visitors can see already-unlinked pins: revalidate retention before mutation.
// Pass the previous result to resume a bounded pass; changed membership restarts
// at the current head. Stable bounded passes therefore cannot starve the tail.
PendingReceiverRetirementScan VisitPendingReceiverRetirements(
    size_t budget, void (*visitor)(void*, const PendingReceiverRetirementHandle&),
    void* context, const PendingReceiverRetirementScan* resume = nullptr);
// Only releases registry retention after settlement proof. Caller must retain
// its independent lease and retire any progress-driver claims/callbacks before
// dropping it. This function is NOT FD authority transfer or driver quiescence.
bool ReleaseSettledReceiverRetirement(const PendingReceiverRetirementHandle& record);

}  // namespace darwin_art::input
