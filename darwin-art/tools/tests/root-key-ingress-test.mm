#import <AppKit/AppKit.h>

#include "compat/binder/wire_channel_lifetime.h"
#include "compat/looper/android_looper_owner.h"
#include "runtime/framework/input/input_routing.h"
#include "runtime/framework/input/input_routing_focus.h"
#include "runtime/framework/input/root_key_authority.h"
#include "runtime/framework/input/root_key_ingress.h"
#include "runtime/framework/input/root_key_routing.h"

#include <cassert>
#include <cstdint>
#include <cstdio>
#include <memory>
#include <new>
#include <vector>

// This test uses real DesktopRootEvents, RootKeyAuthority, input routing,
// focus admission, and ReusableLooperTask. The POSIX wake broker and the
// SubmitPort below are controlled component-test seams: they do not establish
// Android Binder/ALooper or physical input-device acceptance.

@interface RootKeyIngressTestWindow : NSWindow
@property(nonatomic, assign) BOOL reportedKeyWindow;
@end

@implementation RootKeyIngressTestWindow
- (BOOL)isKeyWindow { return self.reportedKeyWindow; }
@end

namespace {

using darwin_art::DarwinArtInputEnqueueResult;
using darwin_art::binder::WireChannelLifetime;
using darwin_art::input::AcquireRootKeyAuthority;
using darwin_art::input::AcquireRootKeyIngress;
using darwin_art::input::ApplyInputRoutingFocusControl;
using darwin_art::input::CommitInputRoutingFocusNotification;
using darwin_art::input::CommitInputRoutingFocusReadiness;
using darwin_art::input::CreateInputRoutingState;
using darwin_art::input::CreateRootKeyDecisionRecord;
using darwin_art::input::FocusControl;
using darwin_art::input::InputRoutingAdmission;
using darwin_art::input::InputRoutingHandle;
using darwin_art::input::InputRoutingInflightLease;
using darwin_art::input::InputRoutingRecipientHandle;
using darwin_art::input::PublishInputRoutingWmsFrame;
using darwin_art::input::PublishInputRoutingRecipient;
using darwin_art::input::RootKeyAuthority;
using darwin_art::input::RootKeyIngress;
using darwin_art::input::AcquireInputRoutingHead;
using darwin_art::input::BeginInputRoutingTransportSend;
using darwin_art::input::CompleteInputRoutingPacket;
using darwin_art::input::ReserveInputRoutingPacket;
using darwin_art::input::RouteRootFrameworkKeyPacket;
using darwin_art::window::DesktopRootEvents;

struct SinkState {
  bool backpressure_once = false;
  bool throw_once = false;
  int attempts = 0;
  std::vector<uint64_t> accepted_sequences;
  std::vector<const void*> admission_decisions;
  std::vector<uint64_t> admission_cache_revisions;
};

SinkState* g_sink = nullptr;

DarwinArtInputEnqueueResult SubmitControlled(InputRoutingAdmission admission) {
  assert(g_sink != nullptr);
  ++g_sink->attempts;
  if (admission.key_fence.ticket != nullptr) {
    g_sink->admission_decisions.push_back(admission.key_fence.ticket->Decision().get());
    g_sink->admission_cache_revisions.push_back(admission.focus_cache_revision);
  }
  if (g_sink->backpressure_once) {
    g_sink->backpressure_once = false;
    return DarwinArtInputEnqueueResult::kBackpressured;
  }
  if (g_sink->throw_once) {
    g_sink->throw_once = false;
    throw std::bad_alloc();
  }
  g_sink->accepted_sequences.push_back(admission.packet.key.sequence);
  return DarwinArtInputEnqueueResult::kQueued;
}

void Fact(void*, DarwinArtDesktopRootEvent) noexcept {}

bool Check(bool value, const char* message) {
  if (value) return true;
  std::fprintf(stderr, "root key ingress failure: %s\n", message);
  return false;
}

::DarwinArtKeyEventV1 Key(uint64_t sequence) {
  ::DarwinArtKeyEventV1 key{};
  key.version = 1;
  key.size = sizeof(::DarwinArtKeyEventV1);
  key.action = 0;
  key.sequence = sequence;
  key.event_time_nanos = 1;
  key.down_time_nanos = 1;
  key.key_code = 29;
  key.scan_code = 30;
  key.device_id = 1;
  key.source = 0x101;
  return key;
}

bool PublishReady(const std::shared_ptr<RootKeyAuthority>& authority,
                  const std::shared_ptr<DesktopRootEvents>& root,
                  const InputRoutingHandle& route,
                  const InputRoutingRecipientHandle& recipient,
                  uint64_t sequence, uint64_t epoch) {
  const auto record = CreateRootKeyDecisionRecord(
      root->incarnation(), root->Snapshot().latest_emitted_serial, sequence,
      epoch, true);
  if (!record || !authority->PublishDecision(record) ||
      !authority->ResolveDecision(record, route))
    return false;
  const auto focus = ApplyInputRoutingFocusControl(recipient,
                                                   FocusControl{epoch, true});
  return focus.Accepted() && CommitInputRoutingFocusNotification(focus) &&
         CommitInputRoutingFocusReadiness(focus, true);
}

bool PublishRevoke(const std::shared_ptr<RootKeyAuthority>& authority,
                   const std::shared_ptr<DesktopRootEvents>& root,
                   uint64_t sequence, uint64_t fact_serial) {
  const auto revoke = CreateRootKeyDecisionRecord(
      root->incarnation(), fact_serial, sequence, 0, false);
  return revoke != nullptr && authority->PublishDecision(revoke);
}

}  // namespace

int main() {
  @autoreleasepool {
    [NSApplication sharedApplication];
    void* looper = darwin_art::looper::PrepareCurrent();
    if (!Check(looper != nullptr, "real current owner looper")) return 1;

    auto* window = [[RootKeyIngressTestWindow alloc]
        initWithContentRect:NSMakeRect(0, 0, 320, 240)
                  styleMask:NSWindowStyleMaskBorderless
                    backing:NSBackingStoreBuffered
                      defer:YES];
    if (!Check(window != nil, "real AppKit window")) return 1;
    window.releasedWhenClosed = NO;
    window.reportedKeyWindow = YES;
    auto root = DesktopRootEvents::Create(window);
    auto authority = AcquireRootKeyAuthority(root);
    auto wire = WireChannelLifetime::Create();
    if (!Check(root && authority && wire, "real root/authority/wire allocation")) return 1;
    if (!Check(authority->AttachServer(wire, wire->Generation()) &&
                   root->Bind(window, Fact, {}),
               "root server attachment and stamp binding")) return 1;

    auto route = CreateInputRoutingState();
    auto recipient = darwin_art::input::PrepareInputRoutingRecipient(route, 7101);
    if (!Check(route && recipient && PublishInputRoutingRecipient(recipient).Published() &&
                   !PublishInputRoutingWmsFrame(route, 0, 0, 320, 240, true),
               "real routing publication")) return 1;

    SinkState sink;
    g_sink = &sink;
    auto ingress = AcquireRootKeyIngress(authority, looper, &SubmitControlled);
    if (!Check(ingress != nullptr, "canonical ingress acquisition")) return 1;
    if (!Check(AcquireRootKeyIngress(authority, looper, &SubmitControlled) == ingress,
               "repeated surface install reuses one FIFO")) return 1;
    if (!Check(AcquireRootKeyIngress(authority, reinterpret_cast<void*>(0x1),
                                     &SubmitControlled) == nullptr,
               "conflicting owner looper rejected")) return 1;

    // A selected key is fenced to A at physical arrival. Publishing B before
    // the owner looper drains must drop it, never retarget it to B.
    if (!Check(PublishReady(authority, root, route, recipient, 1, 11),
               "decision A readiness")) return 1;
    if (!Check(ingress->Submit(Key(100)) == DarwinArtInputEnqueueResult::kQueued,
               "arrival captures decision A")) return 1;
    if (!Check(PublishReady(authority, root, route, recipient, 2, 12),
               "decision B readiness")) return 1;
    if (!Check(ingress->Submit(Key(101)) == DarwinArtInputEnqueueResult::kQueued,
               "successor B arrival behind stale A")) return 1;
    (void)darwin_art::looper::PollCurrent(0);
    if (!Check(sink.accepted_sequences.size() == 1 &&
                   sink.accepted_sequences.front() == 101 && sink.attempts == 1,
               "stale A is discarded and successor B drains in the same wake")) return 1;
    sink.attempts = 0;
    sink.accepted_sequences.clear();
    sink.admission_decisions.clear();
    sink.admission_cache_revisions.clear();

    // With no selected decision, the same activation interval waits at the
    // FIFO head. A later matching-fact decision then drains it.
    const uint64_t fact_serial = root->Snapshot().latest_emitted_serial;
    if (!Check(PublishRevoke(authority, root, 3, fact_serial),
               "same-interval revoke")) return 1;
    if (!Check(ingress->Submit(Key(200)) == DarwinArtInputEnqueueResult::kQueued,
               "unassigned arrival queued")) return 1;
    (void)darwin_art::looper::PollCurrent(0);
    if (!Check(sink.attempts == 0, "unassigned head waits without polling")) return 1;
    if (!Check(PublishReady(authority, root, route, recipient, 4, 13),
               "matching-fact decision after wait")) return 1;
    (void)darwin_art::looper::PollCurrent(0);
    if (!Check(sink.accepted_sequences.size() == 1 &&
                   sink.accepted_sequences.front() == 200,
               "unassigned key drains after decision and readiness")) return 1;

    // Delay readiness after EnsureSubscription has run. The actual readiness
    // notification must wake the ingress; no authority progress request is
    // used for this retry.
    const auto delayed = CreateRootKeyDecisionRecord(
        root->incarnation(), root->Snapshot().latest_emitted_serial, 5, 14, true);
    if (!Check(delayed && authority->PublishDecision(delayed) &&
                   authority->ResolveDecision(delayed, route),
               "delayed-readiness decision")) return 1;
    const auto delayed_focus = ApplyInputRoutingFocusControl(
        recipient, FocusControl{14, true});
    if (!Check(delayed_focus.Accepted() &&
                   CommitInputRoutingFocusNotification(delayed_focus),
               "delayed-readiness notification commit")) return 1;
    const int delayed_attempts = sink.attempts;
    if (!Check(ingress->Submit(Key(250)) == DarwinArtInputEnqueueResult::kQueued,
               "delayed-readiness arrival")) return 1;
    (void)darwin_art::looper::PollCurrent(0);
    if (!Check(sink.attempts == delayed_attempts,
               "unready key waits after route subscription")) return 1;
    if (!Check(CommitInputRoutingFocusReadiness(delayed_focus, true),
               "delayed-readiness success")) return 1;
    (void)darwin_art::looper::PollCurrent(0);
    if (!Check(sink.accepted_sequences.back() == 250,
               "focus readiness notification wakes ingress")) return 1;

    // The injected owner reports backpressure without taking ownership. The
    // ingress retries the same frozen admission fence, then preserves FIFO
    // order for subsequent physical keys. A real routing completion
    // notification supplies the retry wake (controlled submit/wake ports are
    // still not physical Android acceptance).
    sink.backpressure_once = true;
    const size_t decisions_before = sink.admission_decisions.size();
    if (!Check(ingress->Submit(Key(301)) == DarwinArtInputEnqueueResult::kQueued,
               "backpressure key arrival")) return 1;
    (void)darwin_art::looper::PollCurrent(0);
    if (!Check(sink.accepted_sequences.back() == 250 &&
                   sink.attempts == delayed_attempts + 2,
               "first owner backpressure retains key")) return 1;
    if (!Check(ingress->Submit(Key(302)) == DarwinArtInputEnqueueResult::kQueued &&
                   ingress->Submit(Key(303)) == DarwinArtInputEnqueueResult::kQueued,
               "successor keys queue behind frozen head")) return 1;

    InputRoutingAdmission completion_admission;
    if (!Check(RouteRootFrameworkKeyPacket(authority, Key(999),
                                           &completion_admission) ==
                   DarwinArtInputEnqueueResult::kQueued,
               "real completion admission")) return 1;
    InputRoutingInflightLease completion_lease;
    if (!Check(ReserveInputRoutingPacket(std::move(completion_admission), true,
                                         &completion_lease) &&
                   AcquireInputRoutingHead(route, &completion_lease) &&
                   BeginInputRoutingTransportSend(completion_lease) &&
                   CompleteInputRoutingPacket(
                       std::move(completion_lease),
                       darwin_art::input::InputRoutingDeliveryResult::kAccepted),
               "real packet completion notification")) return 1;
    (void)darwin_art::looper::PollCurrent(0);
    if (!Check(sink.accepted_sequences.size() == 5 &&
                   sink.accepted_sequences[2] == 301 &&
                   sink.accepted_sequences[3] == 302 &&
                   sink.accepted_sequences[4] == 303 &&
                   sink.admission_decisions.size() >= decisions_before + 2 &&
                   sink.admission_decisions[decisions_before] ==
                       sink.admission_decisions[decisions_before + 1] &&
                   sink.admission_cache_revisions[decisions_before] ==
                       sink.admission_cache_revisions[decisions_before + 1],
               "completion wake retries frozen head and preserves FIFO")) return 1;

    // A throwing injected owner port is a terminal boundary: the owner task
    // must contain it, close the ingress, and never blindly replay the same
    // admission or invoke the port again.
    sink.throw_once = true;
    const int attempts_before_throw = sink.attempts;
    if (!Check(ingress->Submit(Key(350)) == DarwinArtInputEnqueueResult::kQueued,
               "throw-regression arrival")) return 1;
    (void)darwin_art::looper::PollCurrent(0);
    if (!Check(sink.attempts == attempts_before_throw + 1 &&
                   ingress->Submit(Key(351)) ==
                       DarwinArtInputEnqueueResult::kNoFocusedChannel &&
                   ingress->IsQuiescent(),
               "throwing submit port closes without duplicate retry")) return 1;

    // Public ingress capacity is independently bounded even when its owner
    // task has not yet been polled. The 33rd event is never copied.
    auto* second_window = [[RootKeyIngressTestWindow alloc]
        initWithContentRect:NSMakeRect(0, 0, 320, 240)
                  styleMask:NSWindowStyleMaskBorderless
                    backing:NSBackingStoreBuffered
                      defer:YES];
    second_window.releasedWhenClosed = NO;
    second_window.reportedKeyWindow = YES;
    auto second_root = DesktopRootEvents::Create(second_window);
    auto second_authority = AcquireRootKeyAuthority(second_root);
    auto second_ingress = AcquireRootKeyIngress(second_authority, looper,
                                                &SubmitControlled);
    if (!Check(second_root && second_authority && second_ingress,
               "second bounded ingress")) return 1;
    for (uint64_t sequence = 1; sequence <= 32; ++sequence) {
      if (!Check(second_ingress->Submit(Key(1000 + sequence)) ==
                     DarwinArtInputEnqueueResult::kQueued,
                 "bounded public copy")) return 1;
    }
    if (!Check(second_ingress->Submit(Key(1033)) ==
                   DarwinArtInputEnqueueResult::kBackpressured,
               "33rd public key backpressured")) return 1;

    auto invalid = Key(400);
    invalid.version = 2;
    if (!Check(ingress->Submit(invalid) == DarwinArtInputEnqueueResult::kNoFocusedChannel,
               "invalid key ABI rejected")) return 1;
    ingress->Close();
    if (!Check(AcquireRootKeyIngress(authority, looper, &SubmitControlled) == nullptr,
               "closed canonical ingress cannot be reused")) return 1;
    // Teardown discovers both the retired first facade and the second facade's
    // armed timer immediately; it does not wait for the five-second deadline.
    if (!Check(darwin_art::input::CloseRootKeyIngressAdmission(),
               "global ingress admission close")) return 1;
    (void)darwin_art::looper::PollCurrent(0);
    if (!Check(darwin_art::input::PollRootKeyIngressQuiesced(),
               "global ingress quiescence")) return 1;

    g_sink = nullptr;
    authority->Close();
    root->Close();
    [window close];
    std::puts("root key ingress: real authority/routing/focus + controlled wake/submit ports PASS");
  }
}
