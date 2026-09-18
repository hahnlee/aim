#import <AppKit/AppKit.h>

#include "compat/window/desktop_root_events.h"
#include "compat/binder/wire_channel_lifetime.h"
#include "runtime/framework/input/input_routing_state_internal.h"
#include "runtime/framework/input/root_key_authority.h"

#include <cassert>
#include <cstdio>
#include <limits>
#include <memory>
#include <mutex>
#include <thread>
#include <vector>

namespace darwin_art::input {
// Domain's routing-selection helpers are outside this owner's scope. The test
// only needs the real domain transaction lock for the ordering guard.
InputRoutingSelectionSnapshot SnapshotInputRoutingSelection(
    const InputRoutingHandle&) {
  return {};
}
bool ValidateInputRoutingSelection(const InputRoutingHandle&,
                                   const InputRoutingSelectionSnapshot&,
                                   InputRoutingAdmission*) {
  return false;
}

}  // namespace darwin_art::input

@interface RootKeyAuthorityTestWindow : NSWindow
@property(nonatomic, assign) BOOL reportedKeyWindow;
@end

@implementation RootKeyAuthorityTestWindow
- (BOOL)isKeyWindow { return self.reportedKeyWindow; }
@end

namespace {

using darwin_art::binder::WireChannelLifetime;
using darwin_art::input::AcquireRootKeyAuthority;
using darwin_art::input::CreateRootKeyDecisionRecord;
using darwin_art::input::InputRoutingHandle;
using darwin_art::input::InputRoutingState;
using darwin_art::input::LockInputRoutingDomain;
using darwin_art::input::LockRootKeyAuthorityForRouting;
using darwin_art::input::RootKeyAuthority;
using darwin_art::input::RootKeyAuthorityAcquireStatus;
using darwin_art::input::RootKeyAuthorityTicket;
using darwin_art::window::DesktopRootEvents;

void NoopFact(void*, DarwinArtDesktopRootEvent) noexcept {}

bool Check(bool value, const char* message) {
  if (value) return true;
  std::fprintf(stderr, "root key authority failure: %s\n", message);
  return false;
}

std::shared_ptr<RootKeyAuthority> Acquire(
    const std::shared_ptr<DesktopRootEvents>& root) {
  RootKeyAuthorityAcquireStatus status =
      RootKeyAuthorityAcquireStatus::kInvalidRoot;
  auto authority = AcquireRootKeyAuthority(root, &status);
  return authority;
}

}  // namespace

int main() {
  @autoreleasepool {
    [NSApplication sharedApplication];
    auto* window = [[RootKeyAuthorityTestWindow alloc]
        initWithContentRect:NSMakeRect(0, 0, 320, 240)
                  styleMask:NSWindowStyleMaskBorderless
                    backing:NSBackingStoreBuffered
                      defer:YES];
    if (!Check(window != nil, "real AppKit window")) return 1;
    window.releasedWhenClosed = NO;
    window.reportedKeyWindow = YES;

    auto root = DesktopRootEvents::Create(window);
    if (!Check(root != nullptr, "root allocation")) return 1;
    const uint64_t incarnation = root->incarnation();
    if (!Check(incarnation != 0, "root incarnation")) return 1;

    auto authority = Acquire(root);
    if (!Check(authority != nullptr && !authority->Closed(),
               "exact-root authority acquisition")) return 1;
    auto preattach = CreateRootKeyDecisionRecord(incarnation, 1, 1, 1, true);
    if (!Check(preattach && !authority->PublishDecision(preattach),
               "pre-attachment decision rejected")) return 1;

    // Concurrent callers observe one canonical facade after installation.
    std::mutex results_mutex;
    std::vector<std::shared_ptr<RootKeyAuthority>> results;
    std::vector<std::thread> callers;
    for (int i = 0; i < 8; ++i) {
      callers.emplace_back([&] {
        auto candidate = Acquire(root);
        std::lock_guard<std::mutex> lock(results_mutex);
        results.push_back(std::move(candidate));
      });
    }
    for (auto& caller : callers) caller.join();
    if (!Check(results.size() == 8, "concurrent acquisition count")) return 1;
    for (const auto& candidate : results) {
      if (!Check(candidate && candidate.get() == authority.get(),
                 "concurrent acquisition canonical identity")) return 1;
    }

    auto wire = WireChannelLifetime::Create();
    if (!Check(wire != nullptr, "wire lifetime allocation")) return 1;
    const uint64_t generation = wire->Generation();
    if (!Check(authority->AttachServer(wire, generation) &&
                   authority->AttachServer(wire, generation),
               "exact wire attach/idempotent retry")) return 1;
    auto other_wire = WireChannelLifetime::Create();
    if (!Check(other_wire &&
                   !authority->AttachServer(other_wire, other_wire->Generation()),
               "distinct wire replacement rejected")) return 1;

    if (!Check(root->Bind(window, NoopFact, {}), "root bind")) return 1;
    const auto stamp = root->Snapshot();
    if (!Check(stamp.key && stamp.latest_emitted_serial != 0,
               "bound key stamp")) return 1;

    auto decision = CreateRootKeyDecisionRecord(
        incarnation, stamp.latest_emitted_serial, 1, 1, true);
    if (!Check(decision != nullptr && authority->PublishDecision(decision),
               "decision publication")) return 1;
    if (!Check(authority->PublishDecision(decision),
               "same-record publication retry")) return 1;
    auto equal_different = CreateRootKeyDecisionRecord(
        incarnation, stamp.latest_emitted_serial, 1, 1, true);
    if (!Check(equal_different && !authority->PublishDecision(equal_different),
               "equal-sequence different-record rejection")) return 1;
    if (!Check(!authority->PublishDecision(CreateRootKeyDecisionRecord(
                   incarnation, stamp.latest_emitted_serial, 0, 1, true)),
               "zero-sequence rejection")) return 1;
    if (!Check(!authority->PublishDecision(CreateRootKeyDecisionRecord(
                   incarnation, stamp.latest_emitted_serial, 2, 0, true)),
               "selected zero-epoch rejection")) return 1;
    if (!Check(static_cast<bool>(CreateRootKeyDecisionRecord(
                   incarnation, stamp.latest_emitted_serial, 3, 0, false)),
               "zero-epoch revoke is representable")) return 1;
    if (!Check(!CreateRootKeyDecisionRecord(
                   incarnation, stamp.latest_emitted_serial, 4,
                   std::numeric_limits<uint64_t>::max(), false),
               "high-bit revoke epoch rejection")) return 1;

    InputRoutingHandle route_a = std::make_shared<InputRoutingState>();
    InputRoutingHandle route_b = std::make_shared<InputRoutingState>();
    if (!Check(authority->ResolveDecision(decision, route_a) &&
                   authority->ResolveDecision(decision, route_a) &&
                   !authority->ResolveDecision(decision, route_b),
               "single exact route resolution")) return 1;

    RootKeyAuthorityTicket ticket;
    {
      auto domain = LockInputRoutingDomain();
      auto guard = LockRootKeyAuthorityForRouting(domain, *authority);
      ticket = guard.TrySnapshot();
      if (!Check(ticket && ticket.Decision().get() == decision.get() &&
                     ticket.Routing().get() == route_a.get() &&
                     ticket.WireGeneration() == generation &&
                     guard.ValidateTicket(ticket),
                 "domain-ordered immutable ticket snapshot")) return 1;
    }
    // The ticket carries weak authority/routing identity, but an inert wire
    // lifetime and immutable decision. Route destruction cannot retarget it.
    route_a.reset();
    {
      auto domain = LockInputRoutingDomain();
      auto guard = LockRootKeyAuthorityForRouting(domain, *authority);
      if (!Check(!guard.ValidateTicket(ticket),
                 "expired resolved route invalidates ticket")) return 1;
    }

    // A newer unresolved record immediately revokes the old resolved route;
    // resolving it later is the only way to establish a new exact route.
    route_a = std::make_shared<InputRoutingState>();
    auto newer = CreateRootKeyDecisionRecord(
        incarnation, stamp.latest_emitted_serial, 2, 2, true);
    if (!Check(newer && authority->PublishDecision(newer) &&
                   !authority->ResolveDecision(decision, route_a),
               "newer unresolved decision supersedes old route")) return 1;
    {
      auto domain = LockInputRoutingDomain();
      auto guard = LockRootKeyAuthorityForRouting(domain, *authority);
      if (!Check(!guard.TrySnapshot() && !guard.ValidateTicket(ticket),
                 "unresolved publication provides no grant")) return 1;
    }
    if (!Check(authority->ResolveDecision(newer, route_a),
               "new decision route resolution")) return 1;
    route_a.reset();
    auto route_c = std::make_shared<InputRoutingState>();
    if (!Check(!authority->ResolveDecision(newer, route_c),
               "same-record route replacement after expiry rejected")) return 1;

    auto live = CreateRootKeyDecisionRecord(
        incarnation, stamp.latest_emitted_serial, 3, 3, true);
    if (!Check(live && authority->PublishDecision(live) &&
                   authority->ResolveDecision(live, route_c),
               "live grant before local cutoff")) return 1;
    RootKeyAuthorityTicket live_ticket;
    {
      auto domain = LockInputRoutingDomain();
      auto guard = LockRootKeyAuthorityForRouting(domain, *authority);
      live_ticket = guard.TrySnapshot();
      if (!Check(live_ticket && guard.ValidateTicket(live_ticket),
                 "live local-cutoff ticket")) return 1;
    }
    if (!Check(root->Unbind(), "synchronous provider unbind")) return 1;
    {
      auto domain = LockInputRoutingDomain();
      auto guard = LockRootKeyAuthorityForRouting(domain, *authority);
      if (!Check(!guard.ValidateTicket(live_ticket) && !guard.TrySnapshot(),
                 "provider unbind cuts off live grant")) return 1;
    }
    if (!Check(root->Bind(window, NoopFact, {}), "provider rebind")) return 1;
    auto rebound = CreateRootKeyDecisionRecord(
        incarnation, root->Snapshot().latest_emitted_serial, 4, 4, true);
    if (!Check(rebound && authority->PublishDecision(rebound) &&
                   authority->ResolveDecision(rebound, route_c),
               "new fact restores live grant")) return 1;
    {
      auto domain = LockInputRoutingDomain();
      auto guard = LockRootKeyAuthorityForRouting(domain, *authority);
      live_ticket = guard.TrySnapshot();
      if (!Check(live_ticket && guard.ValidateTicket(live_ticket),
                 "live wire-cutoff ticket")) return 1;
    }

    if (!Check(wire->Seal(), "wire seal")) return 1;
    {
      auto domain = LockInputRoutingDomain();
      auto guard = LockRootKeyAuthorityForRouting(domain, *authority);
      if (!Check(!guard.TrySnapshot() && !guard.ValidateTicket(live_ticket),
                 "sealed wire cuts off live grant")) return 1;
    }
    if (!Check(root->Unbind(), "root unbind")) return 1;
    if (!Check(authority->Close(), "explicit authority close")) return 1;
    RootKeyAuthorityAcquireStatus closed_status =
        RootKeyAuthorityAcquireStatus::kAcquired;
    if (!Check(!AcquireRootKeyAuthority(root, &closed_status) &&
                   closed_status == RootKeyAuthorityAcquireStatus::kClosed,
               "closed exact-root catalog is sticky")) return 1;

    // A facade may disappear while the exact root remains open. Its inert
    // catalog Control preserves the server/high-water state; recreation must
    // resubscribe, fence old tickets, and reject the old fact after a new
    // root publication.
    auto recreated_root = DesktopRootEvents::Create(window);
    auto recreated = Acquire(recreated_root);
    auto recreated_wire = WireChannelLifetime::Create();
    if (!Check(recreated && recreated_wire &&
                   recreated->AttachServer(recreated_wire,
                                            recreated_wire->Generation()) &&
                   recreated_root->Bind(window, NoopFact, {}),
               "recreation setup")) return 1;
    const auto recreated_first_stamp = recreated_root->Snapshot();
    auto recreated_first = CreateRootKeyDecisionRecord(
        recreated_root->incarnation(),
        recreated_first_stamp.latest_emitted_serial, 1, 1, true);
    auto recreated_route = std::make_shared<InputRoutingState>();
    if (!Check(recreated_first && recreated->PublishDecision(recreated_first) &&
                   recreated->ResolveDecision(recreated_first, recreated_route),
               "recreation first decision")) return 1;
    RootKeyAuthorityTicket old_ticket;
    {
      auto transaction = LockInputRoutingDomain();
      auto authority_guard =
          LockRootKeyAuthorityForRouting(transaction, *recreated);
      old_ticket = authority_guard.TrySnapshot();
      if (!Check(static_cast<bool>(old_ticket), "recreation first ticket")) return 1;
    }
    std::weak_ptr<RootKeyAuthority> weak_recreated = recreated;
    recreated.reset();
    if (!Check(weak_recreated.expired() && !old_ticket.valid(),
               "old facade ticket is fenced after release")) return 1;
    if (!Check(recreated_root->Unbind() &&
                   recreated_root->Bind(window, NoopFact, {}),
               "root changes while facade absent")) return 1;
    recreated = Acquire(recreated_root);
    if (!Check(recreated && recreated->AttachServer(
                             recreated_wire, recreated_wire->Generation()),
               "recreate exact Control/server")) return 1;
    auto replacement_wire = WireChannelLifetime::Create();
    if (!Check(replacement_wire &&
                   !recreated->AttachServer(replacement_wire,
                                            replacement_wire->Generation()),
               "recreated server remains frozen")) return 1;
    const auto recreated_second_stamp = recreated_root->Snapshot();
    if (!Check(recreated_second_stamp.latest_emitted_serial !=
                   recreated_first_stamp.latest_emitted_serial,
               "recreated root publication advanced")) return 1;
    {
      auto transaction = LockInputRoutingDomain();
      auto authority_guard =
          LockRootKeyAuthorityForRouting(transaction, *recreated);
      if (!Check(!authority_guard.ValidateTicket(old_ticket) &&
                     !authority_guard.TrySnapshot(),
                 "old fact stays fenced after recreation")) return 1;
    }
    auto duplicate_first = CreateRootKeyDecisionRecord(
        recreated_root->incarnation(),
        recreated_second_stamp.latest_emitted_serial, 1, 1, true);
    if (!Check(duplicate_first && !recreated->PublishDecision(duplicate_first),
               "decision sequence high-water preserved")) return 1;
    auto revoke = CreateRootKeyDecisionRecord(
        recreated_root->incarnation(),
        recreated_second_stamp.latest_emitted_serial, 2, 0, false);
    if (!Check(revoke && recreated->PublishDecision(revoke),
               "new epoch-zero revoke publication")) return 1;
    {
      auto transaction = LockInputRoutingDomain();
      auto authority_guard =
          LockRootKeyAuthorityForRouting(transaction, *recreated);
      if (!Check(!authority_guard.TrySnapshot(),
                 "revoke cannot fabricate a key ticket")) return 1;
    }
    auto recreated_second = CreateRootKeyDecisionRecord(
        recreated_root->incarnation(),
        recreated_second_stamp.latest_emitted_serial, 3, 2, true);
    auto recreated_route2 = std::make_shared<InputRoutingState>();
    if (!Check(recreated_second && recreated->PublishDecision(recreated_second) &&
                   recreated->ResolveDecision(recreated_second, recreated_route2),
               "recreated newer decision")) return 1;
    {
      auto transaction = LockInputRoutingDomain();
      auto authority_guard =
          LockRootKeyAuthorityForRouting(transaction, *recreated);
      auto current_ticket = authority_guard.TrySnapshot();
      if (!Check(current_ticket && authority_guard.ValidateTicket(current_ticket),
                 "recreated current ticket")) return 1;
    }
    if (!Check(recreated->Close(), "recreated close")) return 1;
    recreated.reset();
    RootKeyAuthorityAcquireStatus recreated_closed_status =
        RootKeyAuthorityAcquireStatus::kAcquired;
    if (!Check(!AcquireRootKeyAuthority(recreated_root, &recreated_closed_status) &&
                   recreated_closed_status == RootKeyAuthorityAcquireStatus::kClosed,
               "recreated Control remains terminal")) return 1;

    // Releasing the facade leaves no strong authority→routing cycle.
    std::weak_ptr<RootKeyAuthority> weak_authority = authority;
    std::weak_ptr<InputRoutingState> weak_route = route_c;
    route_c.reset();
    results.clear();
    authority.reset();
    if (!Check(weak_authority.expired() && weak_route.expired(),
               "authority and routing weak lifetime")) return 1;

    std::fprintf(stdout,
                 "root key authority: PASS exact-root/decision/wire/ticket/close\n");
    return 0;
  }
}
