// Controlled seam test for the real native Binder endpoint provider.
//
// The Binder/JNI classes below intentionally shadow the AOSP headers in the
// test runner.  This exercises the production provider's ownership, obituary,
// admission, and quiescence logic, but is not physical-device or genuine AOSP
// Binder acceptance.

#include "compat/binder/native_endpoint_lifetime.h"

#include <binder/BpBinder.h>
#include <binder/IBinder.h>

#include <jni.h>

#include <cassert>
#include <atomic>
#include <condition_variable>
#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <functional>
#include <map>
#include <mutex>
#include <string>
#include <thread>
#include <sys/wait.h>
#include <unistd.h>

namespace {

using darwin_art::binder::CaptureNativeBinderEndpoint;
using darwin_art::binder::CloseNativeBinderEndpointAdmission;
using darwin_art::binder::InstallNativeBinderAuthority;
using darwin_art::binder::PollNativeBinderEndpointsQuiesced;

struct HookState {
  std::mutex mutex;
  std::condition_variable cv;
  bool live_enabled = true;
  bool block_live = false;
  bool live_entered = false;
  bool release_block = false;
  bool release_entered = false;
  bool release_continue = false;
  int retains = 0;
  int lives = 0;
  int releases = 0;
};

void* Retain(void* context) {
  auto* state = static_cast<HookState*>(context);
  std::lock_guard lock(state->mutex);
  ++state->retains;
  return context;
}

int32_t Live(void* retained) {
  auto* state = static_cast<HookState*>(retained);
  std::unique_lock lock(state->mutex);
  ++state->lives;
  if (state->block_live) {
    state->live_entered = true;
    state->cv.notify_all();
    state->cv.wait(lock, [&] { return !state->block_live; });
  }
  return state->live_enabled ? 1 : 0;
}

void Release(void* retained) {
  auto* state = static_cast<HookState*>(retained);
  std::unique_lock lock(state->mutex);
  ++state->releases;
  if (state->release_block) {
    state->release_entered = true;
    state->cv.notify_all();
    state->cv.wait(lock, [&] { return state->release_continue; });
  }
  state->cv.notify_all();
}

darwin_art_binder_authority_hooks_t Hooks(HookState* state) {
  darwin_art_binder_authority_hooks_t hooks{};
  hooks.struct_size = sizeof(hooks);
  hooks.abi_version = 1;
  hooks.context = state;
  hooks.retain = Retain;
  hooks.live = Live;
  hooks.release = Release;
  return hooks;
}

class FakeRemote final : public android::BpBinder {
 public:
  explicit FakeRemote(bool rpc = false) : rpc_(rpc) {}

  bool isRpcBinder() const override { return rpc_; }
  bool isBinderAlive() const override { return alive_.load(std::memory_order_acquire); }

  android::status_t linkToDeath(
      const android::sp<DeathRecipient>& recipient) override {
    std::lock_guard lock(mutex_);
    ++links_;
    if (link_status_ != android::OK) return link_status_;
    death_ = recipient;
    return android::OK;
  }

  android::status_t unlinkToDeath(
      const android::wp<DeathRecipient>&) override {
    std::lock_guard lock(mutex_);
    ++unlinks_;
    death_ = android::sp<DeathRecipient>();
    return android::OK;
  }

  void SetLinkStatus(android::status_t status) { link_status_ = status; }

  void Die() {
    android::sp<DeathRecipient> recipient;
    {
      std::lock_guard lock(mutex_);
      alive_.store(false, std::memory_order_release);
      recipient = death_;
    }
    if (recipient != nullptr) recipient->binderDied(android::wp<android::IBinder>());
  }

 private:
  const bool rpc_;
  std::atomic<bool> alive_{true};
  std::mutex mutex_;
  android::status_t link_status_ = android::OK;
  android::sp<DeathRecipient> death_;
  int links_ = 0;
  int unlinks_ = 0;
};

class LocalBinder final : public android::IBinder {};

std::mutex g_objects_mutex;
std::map<jobject, android::sp<android::IBinder>> g_objects;

android::sp<android::IBinder> ObjectBinder(jobject object) {
  std::lock_guard lock(g_objects_mutex);
  const auto found = g_objects.find(object);
  return found == g_objects.end() ? android::sp<android::IBinder>() : found->second;
}

}  // namespace

namespace android {
sp<IBinder> ibinderForJavaObject(JNIEnv*, jobject object) { return ObjectBinder(object); }
}  // namespace android

namespace {

jboolean ExceptionCheck(JNIEnv*) { return JNI_FALSE; }

struct JniFixture {
  JNINativeInterface_ table{};
  JNIEnv env{nullptr};

  JniFixture() : env(&table) { table.ExceptionCheck = ExceptionCheck; }
};

jobject Object(uintptr_t value) {
  return reinterpret_cast<jobject>(value);
}

template <typename T>
android::sp<android::IBinder> InstallObject(jobject object, bool rpc = false) {
  auto binder = android::sp<T>::make(rpc);
  std::lock_guard lock(g_objects_mutex);
  g_objects[object] = binder;
  return binder;
}

android::sp<android::IBinder> InstallLocalObject(jobject object) {
  auto binder = android::sp<LocalBinder>::make();
  std::lock_guard lock(g_objects_mutex);
  g_objects[object] = binder;
  return binder;
}

void ClearObjects() {
  std::lock_guard lock(g_objects_mutex);
  g_objects.clear();
}

void Install(HookState* state) {
  const auto hooks = Hooks(state);
  assert(InstallNativeBinderAuthority(&hooks));
}

void CloseAndCheck(HookState* state) {
  (void)CloseNativeBinderEndpointAdmission();
  assert(PollNativeBinderEndpointsQuiesced());
  std::lock_guard lock(state->mutex);
  assert(state->retains == 1);
  assert(state->releases == 1);
}

void CanonicalGenerationRetry() {
  HookState state;
  Install(&state);
  JniFixture jni;
  const auto first = InstallObject<FakeRemote>(Object(0x101));
  auto one = CaptureNativeBinderEndpoint(&jni.env, Object(0x101));
  assert(one && one->Live());
  const auto generation = one->Generation();
  auto retry = CaptureNativeBinderEndpoint(&jni.env, Object(0x101));
  assert(retry == one && retry->Matches(generation));
  (void)first;
  InstallObject<FakeRemote>(Object(0x102));
  auto successor = CaptureNativeBinderEndpoint(&jni.env, Object(0x102));
  assert(successor && successor->Generation() > generation);
  CloseAndCheck(&state);
  ClearObjects();
}

void NodeDeathSticky() {
  HookState state;
  Install(&state);
  JniFixture jni;
  auto binder = android::sp<FakeRemote>::make();
  {
    std::lock_guard lock(g_objects_mutex);
    g_objects[Object(0x111)] = binder;
  }
  auto endpoint = CaptureNativeBinderEndpoint(&jni.env, Object(0x111));
  assert(endpoint && endpoint->Live());
  const auto generation = endpoint->Generation();
  binder->Die();
  assert(!endpoint->Live());
  assert(!endpoint->Matches(generation));
  assert(!CaptureNativeBinderEndpoint(&jni.env, Object(0x111)));
  CloseAndCheck(&state);
  ClearObjects();
}

void RejectLocal() {
  HookState state;
  Install(&state);
  JniFixture jni;
  InstallLocalObject(Object(0x121));
  assert(!CaptureNativeBinderEndpoint(&jni.env, Object(0x121)));
  CloseAndCheck(&state);
  ClearObjects();
}

void RejectRpc() {
  HookState state;
  Install(&state);
  JniFixture jni;
  InstallObject<FakeRemote>(Object(0x122), true);
  assert(!CaptureNativeBinderEndpoint(&jni.env, Object(0x122)));
  CloseAndCheck(&state);
  ClearObjects();
}

void RejectLinkFailure() {
  HookState state;
  Install(&state);
  JniFixture jni;
  auto binder = android::sp<FakeRemote>::make();
  binder->SetLinkStatus(android::DEAD_OBJECT);
  {
    std::lock_guard lock(g_objects_mutex);
    g_objects[Object(0x123)] = binder;
  }
  assert(!CaptureNativeBinderEndpoint(&jni.env, Object(0x123)));
  CloseAndCheck(&state);
  ClearObjects();
}

void ConnectionEof() {
  HookState state;
  Install(&state);
  JniFixture jni;
  InstallObject<FakeRemote>(Object(0x131));
  auto endpoint = CaptureNativeBinderEndpoint(&jni.env, Object(0x131));
  assert(endpoint && endpoint->Live());
  {
    std::lock_guard lock(state.mutex);
    state.live_enabled = false;
  }
  assert(!endpoint->Live());
  // A terminal EOF is sticky at the provider boundary: a callback that later
  // says live again cannot resurrect the captured endpoint.
  {
    std::lock_guard lock(state.mutex);
    state.live_enabled = true;
  }
  assert(!endpoint->Live());
  assert(!CaptureNativeBinderEndpoint(&jni.env, Object(0x131)));
  CloseAndCheck(&state);
  ClearObjects();
}

void RetainedTicketInertAfterClose() {
  HookState state;
  Install(&state);
  JniFixture jni;
  InstallObject<FakeRemote>(Object(0x141));
  auto ticket = CaptureNativeBinderEndpoint(&jni.env, Object(0x141));
  assert(ticket && ticket->Live());
  const auto generation = ticket->Generation();
  CloseAndCheck(&state);
  assert(!ticket->Live());
  assert(!ticket->Matches(generation));
  ticket.reset();
  ClearObjects();
}

void BlockedLiveCloseThenReaderExit() {
  HookState state;
  Install(&state);
  {
    std::lock_guard lock(state.mutex);
    state.block_live = true;
  }
  JniFixture jni;
  InstallObject<FakeRemote>(Object(0x151));
  std::shared_ptr<darwin_art::binder::EndpointLifetime> result;
  std::thread reader([&] { result = CaptureNativeBinderEndpoint(&jni.env, Object(0x151)); });
  {
    std::unique_lock lock(state.mutex);
    state.cv.wait(lock, [&] { return state.live_entered; });
  }
  assert(!CloseNativeBinderEndpointAdmission());
  assert(!PollNativeBinderEndpointsQuiesced());
  {
    std::lock_guard lock(state.mutex);
    state.block_live = false;
    state.cv.notify_all();
  }
  reader.join();
  assert(!result);
  assert(PollNativeBinderEndpointsQuiesced());
  {
    std::lock_guard lock(state.mutex);
    assert(state.releases == 1);
  }
  ClearObjects();
}

void BlockedReleaseConcurrentPoll() {
  HookState state;
  Install(&state);
  {
    std::lock_guard lock(state.mutex);
    state.release_block = true;
  }
  std::thread closer([&] { (void)CloseNativeBinderEndpointAdmission(); });
  {
    std::unique_lock lock(state.mutex);
    state.cv.wait(lock, [&] { return state.release_entered; });
  }
  assert(!PollNativeBinderEndpointsQuiesced());
  {
    std::lock_guard lock(state.mutex);
    state.release_continue = true;
    state.cv.notify_all();
  }
  closer.join();
  assert(PollNativeBinderEndpointsQuiesced());
  {
    std::lock_guard lock(state.mutex);
    assert(state.releases == 1);
  }
}

void PreVmInstallCloseBalance() {
  HookState state;
  Install(&state);
  CloseAndCheck(&state);
}

void RunCase(const char* name, const std::function<void()>& test) {
  const pid_t child = fork();
  assert(child >= 0);
  if (child == 0) {
    test();
    std::fprintf(stdout, "native endpoint lifetime: %s PASS\n", name);
    std::fflush(stdout);
    _exit(0);
  }
  int status = 0;
  assert(waitpid(child, &status, 0) == child);
  if (!WIFEXITED(status) || WEXITSTATUS(status) != 0) {
    std::fprintf(stderr, "native endpoint lifetime: %s FAIL\n", name);
    std::abort();
  }
}

}  // namespace

int main() {
  RunCase("canonical generation retry", CanonicalGenerationRetry);
  RunCase("node death sticky", NodeDeathSticky);
  RunCase("local Binder rejection", RejectLocal);
  RunCase("RPC Binder rejection", RejectRpc);
  RunCase("link failure", RejectLinkFailure);
  RunCase("connection EOF", ConnectionEof);
  RunCase("retained ticket inert after close", RetainedTicketInertAfterClose);
  RunCase("blocked live callback close/poll", BlockedLiveCloseThenReaderExit);
  RunCase("blocked release callback poll", BlockedReleaseConcurrentPoll);
  RunCase("pre-VM install/close balance", PreVmInstallCloseBalance);
  std::puts("controlled Binder/JNI seam (not physical AOSP acceptance): PASS");
}
