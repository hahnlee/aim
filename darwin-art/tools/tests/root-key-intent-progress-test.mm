#import <AppKit/AppKit.h>
#include "runtime/framework/input/root_key_authority.h"
#include "compat/binder/wire_channel_lifetime.h"
#include "compat/looper/android_looper_owner.h"
#include <cassert>
#include <cstdio>

using namespace darwin_art::input;
@interface IntentProgressWindow : NSWindow
@end
@implementation IntentProgressWindow
- (BOOL)isKeyWindow { return YES; }
@end
namespace darwin_art::input {
// Routing selection is outside this authority/typed-wake test.
InputRoutingSelectionSnapshot SnapshotInputRoutingSelection(const InputRoutingHandle&) { return {}; }
bool ValidateInputRoutingSelection(const InputRoutingHandle&,
    const InputRoutingSelectionSnapshot&, InputRoutingAdmission*) { return false; }
}
struct Evidence {
  std::weak_ptr<RootKeyAuthority> authority;
  int calls = 0;
  RootKeyIntent last;
};
struct WakeContext { std::weak_ptr<Evidence> evidence; };
static void Wake(void* opaque) {
  const auto evidence = static_cast<WakeContext*>(opaque)->evidence.lock();
  if (!evidence) return;
  const auto authority = evidence->authority.lock();
  if (!authority) return;
  auto domain = LockInputRoutingDomain();
  auto guard = LockRootKeyAuthorityForRouting(domain, *authority);
  evidence->last = guard.SnapshotKeyIntent();
  ++evidence->calls;
}
static void Fact(void*, DarwinArtDesktopRootEvent) noexcept {}
int main() {
  @autoreleasepool {
    [NSApplication sharedApplication];
    void* looper = darwin_art::looper::PrepareCurrent();
    assert(looper);
    auto* window = [[IntentProgressWindow alloc]
        initWithContentRect:NSMakeRect(0, 0, 100, 100)
        styleMask:NSWindowStyleMaskBorderless backing:NSBackingStoreBuffered defer:YES];
    window.releasedWhenClosed = NO;
    auto root = darwin_art::window::DesktopRootEvents::Create(window);
    auto authority = AcquireRootKeyAuthority(root);
    assert(root && authority);
    auto evidence = std::make_shared<Evidence>();
    evidence->authority = authority;
    auto context = std::make_shared<WakeContext>();
    context->evidence = evidence;
    auto task = darwin_art::looper::ReusableLooperTask::Prepare(looper,
        {Wake, context.get(), context});
    assert(task && authority->SubscribeProgressTask(task));
    assert(authority->SubscribeProgressTask(task));
    auto conflicting = darwin_art::looper::ReusableLooperTask::Prepare(looper,
        {Wake, context.get(), context});
    assert(conflicting && !authority->SubscribeProgressTask(conflicting));
    {
      auto domain = LockInputRoutingDomain();
      auto guard = LockRootKeyAuthorityForRouting(domain, *authority);
      const auto initial = guard.SnapshotKeyIntent();
      assert(!initial.closed && initial.stamp.key && !initial.server_live && !initial.decision);
    }
    auto wire = darwin_art::binder::WireChannelLifetime::Create();
    assert(authority->AttachServer(wire, wire->Generation()));
    assert(root->Bind(window, Fact, {}));
    auto record = CreateRootKeyDecisionRecord(root->incarnation(),
        root->Snapshot().latest_emitted_serial, 1, 7, true);
    assert(authority->PublishDecision(record));
    assert(evidence->calls == 0); // Never policy/JNI dispatch inline in mutation.
    (void)darwin_art::looper::PollCurrent(0);
    assert(evidence->calls == 1 && evidence->last.server_live &&
           evidence->last.decision == record && !evidence->last.closed);
    auto revoke = CreateRootKeyDecisionRecord(root->incarnation(),
        record->fact_serial, 2, 0, false);
    assert(authority->PublishDecision(revoke));
    authority->RequestProgress();
    assert(evidence->calls == 1);
    (void)darwin_art::looper::PollCurrent(0);
    assert(evidence->calls == 2 && evidence->last.decision == revoke);
    assert(authority->Close());
    assert(evidence->calls == 2);
    (void)darwin_art::looper::PollCurrent(0);
    assert(evidence->calls == 3 && evidence->last.closed);
    assert(task->Cancel() && task->IsQuiescent());
    root->Close();
    [window close];
    std::puts("root key intent/progress: actual reusable Looper / non-inline coalescing / exact intent / close PASS");
  }
}
