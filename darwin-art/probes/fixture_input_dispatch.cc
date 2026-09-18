#include "fixture_input_dispatch.h"

#include <memory>
#include <new>

#include "fixture_input_guest_io.h"
#include "graphics_fixture_state.h"

namespace darwin_art_graphics_fixture {
namespace {
std::shared_ptr<FixtureInputEndpoint> FindOrCreateEndpoint(GraphicsFixtureState* fixture,
                                          JNIEnv* env, jobject root) {
  if (fixture->input_endpoints_retired) return nullptr;
  for (const auto& endpoint : fixture->input_endpoints) {
    if (env->IsSameObject(endpoint->channel.view_root(), root))
      return endpoint;
  }
  jclass looper_class = env->FindClass("android/os/Looper");
  if (looper_class == nullptr || env->ExceptionCheck()) {
    if (looper_class != nullptr) env->DeleteLocalRef(looper_class);
    return nullptr;
  }
  jmethodID my_looper = env->GetStaticMethodID(
      looper_class, "myLooper", "()Landroid/os/Looper;");
  jobject looper = my_looper == nullptr || env->ExceptionCheck()
                       ? nullptr
                       : env->CallStaticObjectMethod(looper_class, my_looper);
  env->DeleteLocalRef(looper_class);
  if (looper == nullptr || env->ExceptionCheck()) {
    if (looper != nullptr) env->DeleteLocalRef(looper);
    return nullptr;
  }
  std::shared_ptr<FixtureInputEndpoint> endpoint;
  try {
    endpoint = std::make_shared<FixtureInputEndpoint>();
    // Reserve before acquiring Java/native resources: vector growth must not
    // strand a channel whose destructor deliberately cannot enter JNI.
    fixture->input_endpoints.reserve(fixture->input_endpoints.size() + 1);
  } catch (const std::bad_alloc&) {
    env->DeleteLocalRef(looper);
    jclass oom = env->FindClass("java/lang/OutOfMemoryError");
    if (oom != nullptr && !env->ExceptionCheck())
      env->ThrowNew(oom, "fixture input endpoint allocation failed");
    if (oom != nullptr) env->DeleteLocalRef(oom);
    return nullptr;
  }
  const bool initialized = endpoint->channel.Initialize(
      env, root, looper, "darwin-art-genuine-fixture-input");
  env->DeleteLocalRef(looper);
  if (!initialized) {
    endpoint->channel.Dispose(env);
    return nullptr;
  }
  fixture->input_endpoints.emplace_back(endpoint);
  return endpoint;
}

bool AdvanceBounded(FixtureInputEndpoint* endpoint, FixtureInputGuestIo& guest,
                    JNIEnv* env, FixtureOwnerProgress progress, void* context) {
  // This is a finite fixture observation budget, not an artificial success
  // deadline. A delayed finish remains outstanding in the same exchange.
  for (size_t i = 0; i < 64 && !env->ExceptionCheck() && !endpoint->retired; ++i) {
    endpoint->exchange.Advance(guest.port());
    const auto phase = endpoint->exchange.phase();
    if (phase == FixtureExchangePhase::kTerminal) return false;
    if (phase == FixtureExchangePhase::kCompleted ||
        phase == FixtureExchangePhase::kIdle)
      return !env->ExceptionCheck() && !endpoint->retired;
    if (progress == nullptr || !progress(context, env)) return false;
  }
  return !env->ExceptionCheck() && !endpoint->retired;
}
}  // namespace

FixtureInputDispatchResult DispatchFixtureInputPacket(
    GraphicsFixtureState* fixture, JNIEnv* env, jobject root,
    const darwin_art::DarwinArtInputPacket& packet,
    FixtureOwnerProgress progress, void* context) {
  FixtureInputDispatchResult result;
  if (fixture == nullptr || env == nullptr || root == nullptr ||
      env->ExceptionCheck()) return result;
  if (packet.kind == darwin_art::DarwinArtInputPacketKind::kPointer &&
      packet.pointer.action > DARWIN_ART_POINTER_CANCEL) {
    jclass unsupported = env->FindClass("java/lang/UnsupportedOperationException");
    if (unsupported != nullptr && !env->ExceptionCheck())
      env->ThrowNew(unsupported, "fixture pointer action is outside the shared transport ABI");
    if (unsupported != nullptr) env->DeleteLocalRef(unsupported);
    return result;
  }
  auto endpoint = FindOrCreateEndpoint(fixture, env, root);
  if (endpoint == nullptr || env->ExceptionCheck()) return result;
  if (endpoint->dispatch_admitted || endpoint->retired) return result;
  endpoint->dispatch_admitted = true;
  struct DispatchAdmission {
    bool& admitted;
    ~DispatchAdmission() { admitted = false; }
  } admission{endpoint->dispatch_admitted};
  FixtureInputGuestIo guest(env, endpoint->channel.producer_fd());
  if (!guest.IsValid() || env->ExceptionCheck()) return result;
  if (endpoint->exchange.phase() != FixtureExchangePhase::kIdle) {
    // Settle the predecessor first. Never attribute its late ACK to this new
    // packet or retry it just because an earlier observer exhausted its budget.
    if (!AdvanceBounded(endpoint.get(), guest, env, progress, context)) {
      result.failed = true;
      return result;
    }
    FixtureExchangeResult predecessor;
    if (!endpoint->exchange.TakeCompleted(&predecessor)) return result;
  }
  if (endpoint->retired || env->ExceptionCheck() ||
      !endpoint->exchange.Submit(packet)) return result;
  result.failed = !AdvanceBounded(endpoint.get(), guest, env, progress, context) ||
                  fixture->retiring.load(std::memory_order_acquire);
  const auto observed = endpoint->exchange.result();
  result.delivered = observed.submitted;
  result.completed = observed.completed;
  result.handled = observed.completed && observed.handled;
  if (observed.completed) {
    FixtureExchangeResult completed;
    (void)endpoint->exchange.TakeCompleted(&completed);
  }
  return result;
}

void DisposeFixtureInputEndpoints(GraphicsFixtureState* fixture, JNIEnv* env) {
  if (fixture == nullptr || env == nullptr) return;
  fixture->input_endpoints_retired = true;
  auto endpoints = std::move(fixture->input_endpoints);
  for (const auto& endpoint : endpoints) endpoint->retired = true;
  for (const auto& endpoint : endpoints) {
    endpoint->channel.Dispose(env);
  }
}
}  // namespace darwin_art_graphics_fixture
