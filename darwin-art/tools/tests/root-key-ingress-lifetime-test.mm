#import <AppKit/AppKit.h>
#include "runtime/framework/input/root_key_ingress_lifetime.h"
#include "compat/looper/android_looper_owner.h"
#include <cassert>
#include <cstdio>
#include <cstdlib>

// Actual opaque lifetime and Looper objects; only the POSIX wake broker and
// unused submit port are controlled seams. This is not physical APK evidence.
@interface IngressLifetimeWindow : NSWindow
@end
@implementation IngressLifetimeWindow
- (BOOL)isKeyWindow { return YES; }
@end

using namespace darwin_art::input;
static darwin_art::DarwinArtInputEnqueueResult NeverSubmit(InputRoutingAdmission) {
  std::fputs("unexpected key submission in inert lifetime test\n", stderr);
  std::abort();
}
static void Count(void* opaque) { ++*static_cast<int*>(opaque); }

int main() {
  @autoreleasepool {
    [NSApplication sharedApplication];
    void* looper = darwin_art::looper::PrepareCurrent();
    assert(looper);
    auto* window = [[IngressLifetimeWindow alloc]
        initWithContentRect:NSMakeRect(0, 0, 100, 100)
        styleMask:NSWindowStyleMaskBorderless backing:NSBackingStoreBuffered defer:YES];
    window.releasedWhenClosed = NO;
    auto root = darwin_art::window::DesktopRootEvents::Create(window);
    auto authority = AcquireRootKeyAuthority(root);
    assert(root && authority);

    auto inert = PrepareRootKeyIngress(authority, looper, NeverSubmit);
    assert(inert.facade && inert.lifetime);
    assert(!IsRootKeyIngressLifetimeClosed(inert.lifetime));
    int calls = 0;
    auto existing = darwin_art::looper::ReusableLooperTask::Prepare(looper, Count, &calls);
    // Preparation has not occupied the authority's progress port.
    assert(existing && authority->SubscribeProgressTask(existing));
    assert(!StartRootKeyIngress(inert.lifetime, authority));
    assert(IsRootKeyIngressLifetimeClosed(inert.lifetime));
    assert(IsRootKeyIngressLifetimeQuiescent(inert.lifetime));
    // Failed candidate startup must not cancel another owner's task.
    assert(existing->Request());
    (void)darwin_art::looper::PollCurrent(0);
    assert(calls == 1);
    existing->Cancel();
    existing.reset();
    inert.facade.reset();
    inert.lifetime.reset();

    auto started = PrepareRootKeyIngress(authority, looper, NeverSubmit);
    assert(started.facade && started.lifetime);
    assert(StartRootKeyIngress(started.lifetime, authority));
    assert(!StartRootKeyIngress(started.lifetime, authority));
    assert(!IsRootKeyIngressLifetimeClosed(started.lifetime));
    authority->RequestProgress();
    (void)darwin_art::looper::PollCurrent(0);
    CloseRootKeyIngressLifetime(started.lifetime);
    assert(IsRootKeyIngressLifetimeQuiescent(started.lifetime));
    assert(started.facade->IsQuiescent());
    started.facade.reset();
    started.lifetime.reset();

    auto cancelled = PrepareRootKeyIngress(authority, looper, NeverSubmit);
    assert(cancelled.facade && cancelled.lifetime);
    CloseRootKeyIngressLifetime(cancelled.lifetime);
    assert(!StartRootKeyIngress(cancelled.lifetime, authority));
    assert(IsRootKeyIngressLifetimeQuiescent(cancelled.lifetime));
    cancelled.facade.reset();
    cancelled.lifetime.reset();
    authority->Close();
    root->Close();
    [window close];
    std::puts("root ingress lifetime: inert prepare / failed start isolation / one-shot / close PASS");
  }
}
