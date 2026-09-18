#import <AppKit/AppKit.h>
#include "runtime/framework/input/root_key_routing.h"
#include "runtime/framework/input/input_routing_focus.h"
#include "runtime/framework/input/input_transport.h"
#include <cassert>
#include <cstdio>

using namespace darwin_art::input;
using namespace darwin_art;
@interface KeyRoutingTestWindow : NSWindow
@end
@implementation KeyRoutingTestWindow
- (BOOL)isKeyWindow { return YES; }
@end

namespace darwin_art::input {
// This component test has no remote socket. Routing, focus, root authority,
// action admission and local FIFO delivery are the real production objects.
int InputTransport::RemoteEndpointFd() const { return -1; }
InputTransportStatus SendInputTransportPacket(InputTransport*, const DarwinArtInputPacket&) {
  return InputTransportStatus::kTerminal;
}
}
static void Fact(void*, DarwinArtDesktopRootEvent) noexcept {}
struct ProgressEvidence {
  int readiness = 0;
  int completions = 0;
};
int main() {
  @autoreleasepool {
    [NSApplication sharedApplication];
    auto* window = [[KeyRoutingTestWindow alloc]
        initWithContentRect:NSMakeRect(0, 0, 100, 100)
        styleMask:NSWindowStyleMaskBorderless backing:NSBackingStoreBuffered defer:YES];
    window.releasedWhenClosed = NO;
    auto root = darwin_art::window::DesktopRootEvents::Create(window);
    auto authority = AcquireRootKeyAuthority(root);
    auto wire = darwin_art::binder::WireChannelLifetime::Create();
    assert(root && authority && wire);
    assert(authority->AttachServer(wire, wire->Generation()));
    assert(root->Bind(window, Fact, {}));
    const auto stamp = root->Snapshot();
    auto route = CreateInputRoutingState();
    auto recipient = PrepareInputRoutingRecipient(route, 901);
    assert(PublishInputRoutingRecipient(recipient).Published());
    assert(!PublishInputRoutingWmsFrame(route, 0, 0, 100, 100, true));
    auto decision = CreateRootKeyDecisionRecord(root->incarnation(),
        stamp.latest_emitted_serial, 1, 8, true);
    assert(authority->PublishDecision(decision));
    assert(authority->ResolveDecision(decision, route));
    auto progress = std::make_shared<ProgressEvidence>();
    auto subscription = SubscribeInputRoutingNotifications(route, {
        .on_notification = [](void* opaque, const InputRoutingNotification& hint) {
          // Reentrant acquisition proves notification happens after the
          // original input-domain transaction has released its lock.
          auto domain = LockInputRoutingDomain();
          auto& evidence = *static_cast<ProgressEvidence*>(opaque);
          if (hint.kind == InputRoutingNotificationKind::kFocusReadiness)
            ++evidence.readiness;
          if (hint.kind == InputRoutingNotificationKind::kPacketCompletion)
            ++evidence.completions;
        }, .context = progress.get(), .context_token = progress});
    assert(subscription);
    DarwinArtKeyEventV1 key{};
    InputRoutingAdmission admission;
    // Selected root alone is insufficient: the exact receiver must be ready.
    assert(RouteRootFrameworkKeyPacket(authority, key, &admission) ==
        DarwinArtInputEnqueueResult::kNoFocusedChannel);
    auto focus = ApplyInputRoutingFocusControl(recipient, FocusControl{8, true});
    assert(focus.Accepted() && CommitInputRoutingFocusNotification(focus));
    assert(CommitInputRoutingFocusReadiness(focus, true));
    assert(progress->readiness == 1);
    auto propose = [&] {
      admission = {};
      assert(RouteRootFrameworkKeyPacket(authority, key, &admission) ==
          DarwinArtInputEnqueueResult::kQueued);
      assert(admission.state == route && admission.key_fence.required);
    };
    propose();
    InputRoutingInflightLease action;
    assert(ReserveInputRoutingPacket(std::move(admission), true, &action));
    assert(AcquireInputRoutingHead(route, &action));
    assert(BeginInputRoutingTransportSend(action));
    assert(CompleteInputRoutingPacket(std::move(action), InputRoutingDeliveryResult::kAccepted));
    InputRoutingPacketLease packet;
    assert(AcquireInputRoutingPacketLease(route, recipient, &packet));
    assert(packet.KeyFence().ticket);
    assert(!BeginInputRoutingPacketDelivery(packet, recipient, CreateInputRoutingState()));
    assert(BeginInputRoutingPacketDelivery(packet, recipient, route));
    assert(!BeginInputRoutingPacketDelivery(packet, recipient, route)); // Single use.
    assert(packet.Complete());

    propose();
    assert(ReserveInputRoutingPacket(std::move(admission), true, &action));
    assert(AcquireInputRoutingHead(route, &action));
    const int old_completions = progress->completions;
    assert(!CompleteInputRoutingPacket(std::move(action), InputRoutingDeliveryResult::kTerminal));
    assert(progress->completions == old_completions + 1);

    // A newer immutable decision revokes an already queued original record.
    propose();
    assert(ReserveInputRoutingPacket(std::move(admission), true, &action));
    assert(AcquireInputRoutingHead(route, &action));
    assert(BeginInputRoutingTransportSend(action));
    assert(CompleteInputRoutingPacket(std::move(action), InputRoutingDeliveryResult::kAccepted));
    assert(AcquireInputRoutingPacketLease(route, recipient, &packet));
    auto revoke = CreateRootKeyDecisionRecord(root->incarnation(),
        stamp.latest_emitted_serial, 2, 0, false);
    assert(authority->PublishDecision(revoke));
    assert(!BeginInputRoutingPacketDelivery(packet, recipient, route));
    assert(packet.Complete(false));
    assert(!HasInputRoutingPackets(route));

    // Fresh decision with matching epoch can route again; closing the exact
    // root then rejects a reserved action before irreversible transport.
    decision = CreateRootKeyDecisionRecord(root->incarnation(),
        stamp.latest_emitted_serial, 3, 8, true);
    assert(authority->PublishDecision(decision) && authority->ResolveDecision(decision, route));
    propose();
    revoke = CreateRootKeyDecisionRecord(root->incarnation(),
        stamp.latest_emitted_serial, 4, 0, false);
    assert(authority->PublishDecision(revoke));
    assert(!ReserveInputRoutingPacket(std::move(admission), true, &action));
    decision = CreateRootKeyDecisionRecord(root->incarnation(),
        stamp.latest_emitted_serial, 5, 9, true);
    assert(authority->PublishDecision(decision) && authority->ResolveDecision(decision, route));
    // Readiness for epoch 8 cannot authorize a decision for epoch 9.
    assert(RouteRootFrameworkKeyPacket(authority, key, &admission) ==
        DarwinArtInputEnqueueResult::kNoFocusedChannel);
    focus = ApplyInputRoutingFocusControl(recipient, FocusControl{9, true});
    assert(focus.Accepted() && CommitInputRoutingFocusNotification(focus));
    assert(CommitInputRoutingFocusReadiness(focus, true));
    propose();
    assert(ReserveInputRoutingPacket(std::move(admission), true, &action));
    assert(AcquireInputRoutingHead(route, &action));
    assert(BeginInputRoutingTransportSend(action));
    revoke = CreateRootKeyDecisionRecord(root->incarnation(),
        stamp.latest_emitted_serial, 6, 0, false);
    assert(authority->PublishDecision(revoke));
    // Already admitted transport cannot be undone, but no local FIFO delivery
    // may be published from its stale completion.
    assert(!CompleteInputRoutingPacket(std::move(action), InputRoutingDeliveryResult::kAccepted));
    assert(!HasInputRoutingPackets(route));
    decision = CreateRootKeyDecisionRecord(root->incarnation(),
        stamp.latest_emitted_serial, 7, 9, true);
    assert(authority->PublishDecision(decision) && authority->ResolveDecision(decision, route));
    propose();
    assert(ReserveInputRoutingPacket(std::move(admission), true, &action));
    assert(AcquireInputRoutingHead(route, &action));
    authority->Close();
    assert(!BeginInputRoutingTransportSend(action));
    assert(!CompleteInputRoutingPacket(std::move(action), InputRoutingDeliveryResult::kAccepted));
    assert(!HasInputRoutingPackets(route));
    root->Close();
    [window close];

    // Provider release tails may reenter arbitrary owners. The admitted FIFO
    // lease must retain the successful promotion through the synchronous Java
    // call, even if other owners relinquish all root/authority references.
    int releases = 0;
    auto* pin_window = [[KeyRoutingTestWindow alloc]
        initWithContentRect:NSMakeRect(0, 0, 100, 100)
        styleMask:NSWindowStyleMaskBorderless backing:NSBackingStoreBuffered defer:YES];
    pin_window.releasedWhenClosed = NO;
    auto pin_root = darwin_art::window::DesktopRootEvents::Create(pin_window);
    auto pin_authority = AcquireRootKeyAuthority(pin_root);
    assert(pin_authority->AttachServer(wire, wire->Generation()));
    assert(pin_root->Bind(pin_window, Fact, {&releases, [](void*) noexcept {},
        [](void* value) noexcept { ++*static_cast<int*>(value); }}));
    auto pin_route = CreateInputRoutingState();
    auto pin_recipient = PrepareInputRoutingRecipient(pin_route, 902);
    assert(PublishInputRoutingRecipient(pin_recipient).Published());
    assert(!PublishInputRoutingWmsFrame(pin_route, 0, 0, 100, 100, true));
    auto pin_decision = CreateRootKeyDecisionRecord(pin_root->incarnation(),
        pin_root->Snapshot().latest_emitted_serial, 1, 10, true);
    assert(pin_authority->PublishDecision(pin_decision) &&
           pin_authority->ResolveDecision(pin_decision, pin_route));
    focus = ApplyInputRoutingFocusControl(pin_recipient, FocusControl{10, true});
    assert(focus.Accepted() && CommitInputRoutingFocusNotification(focus));
    assert(CommitInputRoutingFocusReadiness(focus, true));
    assert(RouteRootFrameworkKeyPacket(pin_authority, key, &admission) ==
        DarwinArtInputEnqueueResult::kQueued);
    assert(ReserveInputRoutingPacket(std::move(admission), true, &action));
    assert(AcquireInputRoutingHead(pin_route, &action));
    assert(BeginInputRoutingTransportSend(action));
    assert(CompleteInputRoutingPacket(std::move(action), InputRoutingDeliveryResult::kAccepted));
    assert(AcquireInputRoutingPacketLease(pin_route, pin_recipient, &packet));
    assert(BeginInputRoutingPacketDelivery(packet, pin_recipient, pin_route));
    pin_root.reset();
    pin_authority.reset();
    assert(releases == 0); // Between admission and simulated Java return.
    assert(packet.Complete());
    assert(releases == 1); // Unlocked terminal lease settlement.
    [pin_window close];
    std::puts("root key routing: exact readiness / immutable FIFO fence / Java cutoff / send cutoff PASS");
  }
}
