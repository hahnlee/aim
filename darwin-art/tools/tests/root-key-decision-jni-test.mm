#import <AppKit/AppKit.h>
#include "runtime/framework/wm/root_key_decision_jni.h"
#include "runtime/framework/wm/root_key_server_jni.h"
#include "runtime/framework/input/channel_identity_catalog.h"
#include "runtime/framework/input/root_key_authority.h"
#include "runtime/framework/input/channel_resources.h"
#include "runtime/framework/input/input_routing_state_internal.h"
#include "compat/binder/wire_channel_lifetime.h"
#include <cassert>
#include <cstring>
#include <functional>
#include <unordered_map>
#include <cstdio>
#include <cstdarg>

// Component fixture: actual JNI adapter, authority, domain, root provider and
// wire lifetime/resource core. Target lookup, server capture, identity catalog,
// continuation transport and routing-state construction are controlled ports,
// not product-level Binder or physical-input acceptance.
@interface KeyDecisionTestWindow : NSWindow
@end
@implementation KeyDecisionTestWindow
- (BOOL)isKeyWindow { return YES; }
@end
namespace {
using namespace darwin_art::framework::wm;
std::shared_ptr<darwin_art::window::DesktopRootEvents> root;
std::shared_ptr<darwin_art::binder::WireChannelLifetime> wire;
std::shared_ptr<darwin_art::input::InputChannelResources> resolved_core;
int catalog_lookups = 0;
JNIEnv* current_env;
JavaVM* current_vm;
bool pending = false;
int globals = 0;
bool fail_token = false;
std::function<void()> on_token_pin;
std::function<void()> on_global_delete;
using AttachFn = jlong (*)(JNIEnv*, jclass, jlong, jlong, jlong, jobject);
using PublishFn = jint (*)(JNIEnv*, jclass, jlong, jlong, jlong, jlong, jlong, jobject);
using CloseFn = void (*)(JNIEnv*, jclass, jlong);
AttachFn attach;
PublishFn publish;
CloseFn close_binding;
template<class T> T Object(uintptr_t value) { return reinterpret_cast<T>(value); }
constexpr uintptr_t kToken = 0x400;
jclass FindClass(JNIEnv*, const char*) { return Object<jclass>(0x100); }
jmethodID GetStaticMethodID(JNIEnv*, jclass, const char*, const char*) {
  return Object<jmethodID>(0x201);
}
jmethodID GetMethodID(JNIEnv*, jclass, const char*, const char*) {
  return Object<jmethodID>(0x202);
}
jobject CallStaticObjectMethodV(JNIEnv*, jclass, jmethodID, va_list) {
  return Object<jobject>(0x210);
}
jobject CallObjectMethodV(JNIEnv*, jobject, jmethodID, va_list) {
  return Object<jobject>(0x100);
}
jstring NewStringUTF(JNIEnv*, const char*) { return Object<jstring>(0x250); }
jobject NewGlobalRef(JNIEnv*, jobject value) {
  if (value == Object<jobject>(kToken)) {
    auto callback = std::move(on_token_pin);
    on_token_pin = {};
    if (callback) callback();
    if (fail_token) { fail_token = false; return nullptr; }
  }
  if (value) ++globals;
  return value;
}
void DeleteGlobalRef(JNIEnv*, jobject value) {
  if (value) --globals;
  assert(globals >= 0);
  auto callback = std::move(on_global_delete);
  on_global_delete = {};
  if (callback) callback();
}
void DeleteLocalRef(JNIEnv*, jobject) {}
jobject NewLocalRef(JNIEnv*, jobject value) { return value; }
jboolean ExceptionCheck(JNIEnv*) { return pending ? JNI_TRUE : JNI_FALSE; }
jboolean IsSameObject(JNIEnv*, jobject a, jobject b) { return a == b ? JNI_TRUE : JNI_FALSE; }
jint GetJavaVM(JNIEnv*, JavaVM** vm) { *vm = current_vm; return JNI_OK; }
jint GetEnv(JavaVM*, void** env, jint) { *env = current_env; return JNI_OK; }
jint RegisterNatives(JNIEnv*, jclass, const JNINativeMethod* methods, jint count) {
  for (int i = 0; i < count; ++i) {
    if (!std::strcmp(methods[i].name, "nativeAttach")) {
      assert(!std::strcmp(methods[i].signature, "(JJJLandroid/os/IBinder;)J"));
      attach = reinterpret_cast<AttachFn>(methods[i].fnPtr);
    } else if (!std::strcmp(methods[i].name, "nativePublish")) {
      assert(!std::strcmp(methods[i].signature, "(JJJJJLandroid/os/IBinder;)I"));
      publish = reinterpret_cast<PublishFn>(methods[i].fnPtr);
    } else if (!std::strcmp(methods[i].name, "nativeClose")) {
      close_binding = reinterpret_cast<CloseFn>(methods[i].fnPtr);
    } else assert(false);
  }
  return JNI_OK;
}
}
namespace darwin_art::framework::wm {
std::shared_ptr<window::DesktopRootEvents> RetainDesktopRootClientTarget(jlong target) {
  return target == 1 || target == 2 ? root : nullptr;
}
RootKeyServerCapture CaptureRootKeyServer(JNIEnv*, jobject capability, jclass) {
  if ((capability != Object<jobject>(0x300) && capability != Object<jobject>(0x301)) ||
      !wire->Live()) return {};
  return {RootKeyServerStatus::kCaptured, wire};
}
}
namespace darwin_art::input {
InputChannelIdentityCatalog::InputChannelIdentityCatalog() = default;
InputChannelIdentityCatalog::~InputChannelIdentityCatalog() = default;
ChannelIdentityResult InputChannelIdentityCatalog::Find(JNIEnv*, jobject) {
  ++catalog_lookups;
  if (resolved_core) return {ChannelIdentityStatus::kResolved, resolved_core};
  return {};
}
InputRoutingHandle CreateInputRoutingState() { return std::make_shared<InputRoutingState>(); }
ChannelRoutingContinuation::~ChannelRoutingContinuation() = default;
void ChannelRoutingContinuation::Retire() {
  std::lock_guard lock(mutex_);
  closed_ = true;
  looper_ = nullptr;
}
bool ChannelRoutingContinuation::Bind(void*, const std::shared_ptr<ChannelEndpoint>&,
    const InputRoutingHandle&, bool (*)(const InputRoutingHandle&)) { return false; }
bool ChannelRoutingContinuation::Request() { return false; }
InputChannelIdentityCatalog& GetChannelIdentityCatalog() {
  static InputChannelIdentityCatalog catalog;
  return catalog;
}
InputRoutingSelectionSnapshot SnapshotInputRoutingSelection(const InputRoutingHandle&) { return {}; }
bool ValidateInputRoutingSelection(const InputRoutingHandle&,
    const InputRoutingSelectionSnapshot&, InputRoutingAdmission*) { return false; }
}
int main() {
  @autoreleasepool {
    JNINativeInterface_ table{};
    table.FindClass = FindClass;
    table.GetStaticMethodID = GetStaticMethodID;
    table.GetMethodID = GetMethodID;
    table.CallStaticObjectMethodV = CallStaticObjectMethodV;
    table.CallObjectMethodV = CallObjectMethodV;
    table.NewStringUTF = NewStringUTF;
    table.NewGlobalRef = NewGlobalRef;
    table.DeleteGlobalRef = DeleteGlobalRef;
    table.DeleteLocalRef = DeleteLocalRef;
    table.NewLocalRef = NewLocalRef;
    table.ExceptionCheck = ExceptionCheck;
    table.IsSameObject = IsSameObject;
    table.GetJavaVM = GetJavaVM;
    table.RegisterNatives = RegisterNatives;
    JNIEnv env{&table};
    JNIInvokeInterface_ vm_table{};
    vm_table.GetEnv = GetEnv;
    JavaVM vm{&vm_table};
    current_env = &env;
    current_vm = &vm;
    NSWindow* window = [[KeyDecisionTestWindow alloc] initWithContentRect:NSMakeRect(0,0,80,80)
        styleMask:NSWindowStyleMaskTitled backing:NSBackingStoreBuffered defer:NO];
    window.releasedWhenClosed = NO;
    root = darwin_art::window::DesktopRootEvents::Create(window);
    assert(root->Bind(window, [](void*, DarwinArtDesktopRootEvent) noexcept {}, {}));
    assert(root->Snapshot().latest_emitted_serial == 1);
    wire = darwin_art::binder::WireChannelLifetime::Create();
    const jlong incarnation = static_cast<jlong>(root->Snapshot().incarnation);
    assert(RegisterRootKeyDecisionClient(&env));
    assert(attach && publish && close_binding);
    const jobject capability = Object<jobject>(0x300);
    const jlong binding = attach(&env, nullptr, 0, 1, incarnation, capability);
    assert(binding > 0);
    assert(attach(&env, nullptr, binding, 2, incarnation, capability) == binding);
    assert(attach(&env, nullptr, binding, 2, incarnation, Object<jobject>(0x301)) == 0);
    assert(attach(&env, nullptr, binding, 3, incarnation, capability) == 0);
    const int binding_globals = globals;
    assert(publish(&env, nullptr, binding, incarnation, 1, 1, 1,
                   Object<jobject>(kToken)) == 1);
    assert(globals == binding_globals + 1);
    const int before_resolution = catalog_lookups;
    auto core = std::make_shared<darwin_art::input::InputChannelResources>("fixture", nullptr);
    resolved_core = core;
    assert(publish(&env, nullptr, binding, incarnation, 1, 1, 1,
                   Object<jobject>(kToken)) == 0);
    assert(catalog_lookups > before_resolution);
    auto resolved_authority = darwin_art::input::AcquireRootKeyAuthority(root);
    darwin_art::input::RootKeyAuthorityTicket resolved_ticket;
    {
      auto domain = darwin_art::input::LockInputRoutingDomain();
      auto guard = darwin_art::input::LockRootKeyAuthorityForRouting(domain, *resolved_authority);
      resolved_ticket = guard.TrySnapshot();
      assert(resolved_ticket);
    }
    assert(resolved_ticket.Routing() == core->Routing());
    resolved_core.reset();
    assert(publish(&env, nullptr, binding, incarnation, 1, 1, 1,
                   Object<jobject>(kToken)) == 1);
    assert(globals == binding_globals + 1);  // equal retries reuse the envelope
    jint nested_retry = -1;
    on_token_pin = [&] {
      {
        auto domain = darwin_art::input::LockInputRoutingDomain();
        auto guard = darwin_art::input::LockRootKeyAuthorityForRouting(domain, *resolved_authority);
        assert(!guard.TrySnapshot() && !guard.ValidateTicket(resolved_ticket));
      }
      nested_retry = publish(&env, nullptr, binding, incarnation, 2, 2, 2,
                             Object<jobject>(kToken));
    };
    assert(publish(&env, nullptr, binding, incarnation, 2, 2, 2,
                   Object<jobject>(kToken)) == 1);
    assert(nested_retry == 1);
    on_token_pin = [&] {
      assert(publish(&env, nullptr, binding, incarnation, 0, 4, 0, nullptr) == 0);
      fail_token = true;
    };
    assert(publish(&env, nullptr, binding, incarnation, 3, 3, 3,
                   Object<jobject>(kToken)) == 0);
    assert(publish(&env, nullptr, binding, incarnation, 0, 4, 0, nullptr) == 0);
    assert(publish(&env, nullptr, binding, incarnation, 5, 5, 5,
                   Object<jobject>(kToken)) == 1);
    bool deletion_reentered = false;
    on_global_delete = [&] {
      deletion_reentered = true;
      const jint result = publish(&env, nullptr, binding, incarnation, 0, 6, 0, nullptr);
      assert(result == 0 || result == 1);  // retirement may precede envelope commit
    };
    assert(publish(&env, nullptr, binding, incarnation, 0, 6, 0, nullptr) == 0);
    assert(deletion_reentered);
    assert(publish(&env, nullptr, binding, incarnation, 0, 6, 0, nullptr) == 0);
    auto authority = darwin_art::input::AcquireRootKeyAuthority(root);
    assert(authority && !authority->Closed());
    pending = true;
    const int before_explicit_close = globals;
    close_binding(&env, nullptr, binding);
    assert(pending && authority->Closed());
    assert(globals == before_explicit_close - 1);  // capability released now
    close_binding(&env, nullptr, binding);
    assert(globals == before_explicit_close - 1);
    assert(wire->Live());  // binding close cannot terminate the shared transport
    pending = false;
    root = darwin_art::window::DesktopRootEvents::Create(window);
    const jlong second_incarnation = static_cast<jlong>(root->Snapshot().incarnation);
    const jlong second_binding = attach(&env, nullptr, 0, 1, second_incarnation, capability);
    assert(second_binding > 0 && second_binding != binding);
    fail_token = true;
    assert(publish(&env, nullptr, second_binding, second_incarnation, 1, 1, 1,
                   Object<jobject>(kToken)) == 2);
    auto second_authority = darwin_art::input::AcquireRootKeyAuthority(root);
    assert(!second_authority || second_authority->Closed());
    root = darwin_art::window::DesktopRootEvents::Create(window);
    const jlong third_incarnation = static_cast<jlong>(root->Snapshot().incarnation);
    const jlong third_binding = attach(&env, nullptr, 0, 1, third_incarnation, capability);
    assert(third_binding > 0);
    const int before_inflight_close = globals;
    on_token_pin = [&] {
      close_binding(&env, nullptr, third_binding);
      assert(globals == before_inflight_close);  // admitted publisher still pins Entry
    };
    assert(publish(&env, nullptr, third_binding, third_incarnation, 1, 1, 1,
                   Object<jobject>(kToken)) == 2);
    assert(globals == before_inflight_close - 1);  // both pins drained
    pending = true;
    bool close_retirement_admitted = false;
    on_global_delete = [&] {
      close_retirement_admitted = true;
      assert(!PollRootKeyDecisionQuiesced(&env));
    };
    assert(CloseRootKeyDecisionAdmission(&env));
    assert(close_retirement_admitted);
    assert(PollRootKeyDecisionQuiesced(&env));
    pending = false;
    bool clear_retirement_admitted = false;
    on_global_delete = [&] {
      clear_retirement_admitted = true;
      assert(!PollRootKeyDecisionQuiesced(&env));
    };
    assert(ClearRootKeyDecisionReferences(&env));
    assert(clear_retirement_admitted);
    assert(globals == 0);
    authority.reset();
    resolved_authority.reset();
    core.reset();
    second_authority.reset();
    root.reset();
    wire.reset();
    [window close];
    std::puts("root-key-decision JNI controlled-port component: PASS");
  }
}
