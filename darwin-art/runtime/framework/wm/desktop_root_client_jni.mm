#include "desktop_root_client_jni.h"
#include "../../../compat/window/desktop_root_target.h"
#include <dispatch/dispatch.h>
#include <atomic>
#include <bit>
#include <cstdio>
#include <limits>
#include <memory>
#include <mutex>
#include <unordered_map>
#include <type_traits>

namespace darwin_art::framework::wm {
namespace {
using window::DesktopRootTarget;
struct Observation {
  std::shared_ptr<DesktopRootTarget> target;
  jobject callback = nullptr;
  jmethodID deliver = nullptr;
  jmethodID rejected = nullptr;
  std::atomic<size_t> pins{0};
  bool sealed = false;
  bool observing = false;
  bool unbind_queued = false;
};
struct ClientState {
  std::mutex mutex;
  JavaVM* vm = nullptr;
  jclass type = nullptr;
  jmethodID ensure = nullptr;
  jmethodID close = nullptr;
  jmethodID quiesced = nullptr;
  bool closed = false;
  uint64_t next = 1;
  size_t work = 0;
  size_t in_flight = 0;
  std::unordered_map<uint64_t, std::shared_ptr<Observation>> observations;
};
ClientState& State() { static ClientState state; return state; }

template <typename Vm>
jint Attach(Vm* vm, JNIEnv** env) {
  // AOSP and the host JDK spell the same invocation ABI differently.
  if constexpr (std::is_invocable_r_v<jint,
      decltype(vm->functions->AttachCurrentThread), Vm*, JNIEnv**, void*>) {
    return vm->functions->AttachCurrentThread(vm, env, nullptr);
  } else {
    return vm->functions->AttachCurrentThread(vm, reinterpret_cast<void**>(env), nullptr);
  }
}

class NativeAdmission {
 public:
  NativeAdmission() {
    std::lock_guard lock(State().mutex);
    admitted_ = !State().closed;
    if (admitted_) ++State().in_flight;
  }
  ~NativeAdmission() {
    if (admitted_) { std::lock_guard lock(State().mutex); --State().in_flight; }
  }
  explicit operator bool() const { return admitted_; }
 private:
  bool admitted_ = false;
};

void Retain(void* context) noexcept {
  ++static_cast<Observation*>(context)->pins;
}
void Release(void* context) noexcept {
  --static_cast<Observation*>(context)->pins;
}

// The admitted tail covers VM access, attach, Java enqueue and detach. The
// process registry and provider retain count independently cover context life.
void Invoke(void* context, DarwinArtDesktopRootEvent fact, bool rejected) noexcept {
  auto& state = State();
  auto& observation = *static_cast<Observation*>(context);
  JavaVM* vm;
  jobject callback;
  jmethodID method;
  {
    std::lock_guard lock(state.mutex);
    if (state.closed || observation.sealed || observation.callback == nullptr) return;
    ++state.in_flight;
    vm = state.vm;
    callback = observation.callback;
    method = rejected ? observation.rejected : observation.deliver;
  }
  JNIEnv* env = nullptr;
  const jint result = vm->GetEnv(reinterpret_cast<void**>(&env), JNI_VERSION_1_6);
  const bool attached = result == JNI_EDETACHED &&
      Attach(vm, &env) == JNI_OK;
  // Existing JNI callers retain ownership of their pending exception.
  bool failed = env == nullptr || env->ExceptionCheck();
  if (!failed) {
    if (rejected) {
      env->CallVoidMethod(callback, method, std::bit_cast<jlong>(fact.incarnation));
    } else {
      env->CallVoidMethod(callback, method, static_cast<jint>(fact.kind),
          std::bit_cast<jlong>(fact.incarnation), std::bit_cast<jlong>(fact.serial),
          fact.key_window_snapshot ? JNI_TRUE : JNI_FALSE);
    }
    failed = env->ExceptionCheck();
    if (failed) {
      // No Java caller exists on this AppKit callback stack. Report the
      // exception and permanently revoke this observation, never acknowledge it.
      env->ExceptionDescribe();
      env->ExceptionClear();
    }
  }
  if (attached && vm->DetachCurrentThread() != JNI_OK) failed = true;
  {
    std::lock_guard lock(state.mutex);
    if (failed) observation.sealed = true;
    --state.in_flight;
  }
  if (failed) std::fputs("DesktopRootClient: host fact enqueue failed; observation revoked\n", stderr);
}

void Deliver(void* context, DarwinArtDesktopRootEvent fact) noexcept {
  Invoke(context, fact, false);
}

void QueueUnbindLocked(const std::shared_ptr<Observation>& observation) {
  auto& state = State();
  if (observation->target == nullptr || observation->unbind_queued) return;
  observation->unbind_queued = true;
  ++state.work;
  const auto target = observation->target;
  dispatch_async(dispatch_get_main_queue(), ^{
    (void)target->UnbindExpected(observation.get());
    auto& current = State();
    std::shared_ptr<DesktopRootTarget> released;
    {
      std::lock_guard lock(current.mutex);
      released = std::move(observation->target);
    }
    released.reset();
    { std::lock_guard lock(current.mutex); --current.work; }
  });
}

jlong Acquire(JNIEnv* env, jclass) {
  NativeAdmission admission;
  if (!admission) return 0;
  if (env->ExceptionCheck()) return 0;
  try {
  auto target = window::AcquireProcessDesktopRootTarget();
  if (target == nullptr) return 0;
  auto observation = std::make_shared<Observation>();
  observation->target = std::move(target);
  auto& state = State();
  std::lock_guard lock(state.mutex);
  if (state.closed || state.next == std::numeric_limits<uint64_t>::max()) return 0;
  // Reclaim canceled attempts on a live JNI caller, not on AppKit or after
  // DestroyVM. No admitted callback/queued provider tail may retain them.
  if (state.work == 0 && state.in_flight == 1) {
    for (auto it = state.observations.begin(); it != state.observations.end();) {
      const auto& previous = it->second;
      if (!previous->sealed || previous->target != nullptr || previous->pins.load() != 0) {
        ++it;
        continue;
      }
      if (previous->callback != nullptr) env->DeleteGlobalRef(previous->callback);
      it = state.observations.erase(it);
    }
  }
  const auto id = state.next++;
  state.observations.emplace(id, std::move(observation));
  return std::bit_cast<jlong>(id);
  } catch (const std::bad_alloc&) {
    jclass error = env->FindClass("java/lang/OutOfMemoryError");
    if (error != nullptr) {
      env->ThrowNew(error, "Could not retain desktop root target");
      env->DeleteLocalRef(error);
    }
    return 0;
  }
}

jlong Incarnation(JNIEnv*, jclass, jlong handle) {
  NativeAdmission admission;
  if (!admission) return 0;
  auto& state = State();
  std::lock_guard lock(state.mutex);
  const auto found = state.observations.find(std::bit_cast<uint64_t>(handle));
  return state.closed || found == state.observations.end() || found->second->sealed ||
      found->second->target == nullptr ? 0 :
      std::bit_cast<jlong>(found->second->target->incarnation());
}

jboolean Observe(JNIEnv* env, jclass, jlong handle, jobject callback) {
  NativeAdmission admission;
  if (!admission) return JNI_FALSE;
  if (callback == nullptr || env->ExceptionCheck()) return JNI_FALSE;
  jclass type = env->GetObjectClass(callback);
  jmethodID method = type == nullptr ? nullptr :
      env->GetMethodID(type, "onHostFact", "(IJJZ)V");
  jmethodID rejected = method == nullptr || env->ExceptionCheck() ? nullptr :
      env->GetMethodID(type, "onObservationRejected", "(J)V");
  if (type != nullptr) env->DeleteLocalRef(type);
  if (rejected == nullptr || env->ExceptionCheck()) return JNI_FALSE;
  jobject global = env->NewGlobalRef(callback);
  if (global == nullptr || env->ExceptionCheck()) return JNI_FALSE;
  auto& state = State();
  std::lock_guard lock(state.mutex);
  const auto found = state.observations.find(std::bit_cast<uint64_t>(handle));
  if (state.closed || found == state.observations.end() || found->second->sealed ||
      found->second->observing || found->second->target == nullptr) {
    env->DeleteGlobalRef(global);
    return JNI_FALSE;
  }
  const auto observation = found->second;
  const auto target = observation->target;
  observation->callback = global;
  observation->deliver = method;
  observation->rejected = rejected;
  observation->observing = true;
  ++state.work;
  dispatch_async(dispatch_get_main_queue(), ^{
    bool admitted;
    {
      std::lock_guard lock(State().mutex);
      admitted = !State().closed && !observation->sealed;
    }
    const bool bound = admitted && target->Bind({Deliver,
        {observation.get(), Retain, Release}});
    if (admitted && !bound)
      Invoke(observation.get(), {.incarnation = target->incarnation()}, true);
    {
      std::lock_guard lock(State().mutex);
      if (!bound) observation->sealed = true;
      --State().work;
    }
    if (admitted && !bound)
      std::fputs("DesktopRootClient: exact root binding rejected\n", stderr);
  });
  // Scheduling acknowledgement only, not root attachment or Android focus.
  return JNI_TRUE;
}

void ReleaseTarget(JNIEnv*, jclass, jlong handle) {
  NativeAdmission admission;
  if (!admission) return;
  auto& state = State();
  std::lock_guard lock(state.mutex);
  const auto found = state.observations.find(std::bit_cast<uint64_t>(handle));
  if (found == state.observations.end()) return;
  found->second->sealed = true;
  QueueUnbindLocked(found->second);
}

jclass LoadClient(JNIEnv* env) {
  jclass loader_type = env->FindClass("java/lang/ClassLoader");
  if (loader_type == nullptr) return nullptr;
  jmethodID get = env->GetStaticMethodID(loader_type, "getSystemClassLoader",
      "()Ljava/lang/ClassLoader;");
  jmethodID load = get == nullptr ? nullptr : env->GetMethodID(loader_type,
      "loadClass", "(Ljava/lang/String;)Ljava/lang/Class;");
  jobject loader = load == nullptr ? nullptr : env->CallStaticObjectMethod(loader_type, get);
  jstring name = loader == nullptr || env->ExceptionCheck() ? nullptr :
      env->NewStringUTF("dev.darwinart.runtime.wm.DesktopRootClient");
  jclass result = name == nullptr ? nullptr :
      static_cast<jclass>(env->CallObjectMethod(loader, load, name));
  if (name != nullptr) env->DeleteLocalRef(name);
  if (loader != nullptr) env->DeleteLocalRef(loader);
  env->DeleteLocalRef(loader_type);
  return result;
}
}  // namespace

bool RegisterDesktopRootClient(JNIEnv* env) {
  NativeAdmission admission;
  if (!admission) return false;
  if (env == nullptr || env->ExceptionCheck()) return false;
  auto& state = State();
  { std::lock_guard lock(state.mutex);
    if (state.type != nullptr) return !state.closed;
  }
  jclass type = LoadClient(env);
  if (type == nullptr || env->ExceptionCheck()) return false;
  const JNINativeMethod methods[] = {
    {const_cast<char*>("acquireTarget"), const_cast<char*>("()J"), reinterpret_cast<void*>(Acquire)},
    {const_cast<char*>("targetIncarnation"), const_cast<char*>("(J)J"), reinterpret_cast<void*>(Incarnation)},
    {const_cast<char*>("observe"), const_cast<char*>("(JLdev/darwinart/runtime/wm/DesktopRootClient;)Z"), reinterpret_cast<void*>(Observe)},
    {const_cast<char*>("releaseTarget"), const_cast<char*>("(J)V"), reinterpret_cast<void*>(ReleaseTarget)},
  };
  const bool registered = env->RegisterNatives(type, methods, 4) == JNI_OK;
  jmethodID ensure = !registered || env->ExceptionCheck() ? nullptr :
      env->GetStaticMethodID(type, "ensureAttached",
                             "(Landroid/view/InputChannel;)Z");
  jmethodID close = ensure == nullptr || env->ExceptionCheck() ? nullptr :
      env->GetStaticMethodID(type, "closeAdmission", "()V");
  jmethodID quiesced = close == nullptr || env->ExceptionCheck() ? nullptr :
      env->GetStaticMethodID(type, "isQuiesced", "()Z");
  jclass global = quiesced == nullptr || env->ExceptionCheck() ? nullptr :
      static_cast<jclass>(env->NewGlobalRef(type));
  env->DeleteLocalRef(type);
  if (global == nullptr || env->ExceptionCheck()) return false;
  JavaVM* vm = nullptr;
  if (env->GetJavaVM(&vm) != JNI_OK) { env->DeleteGlobalRef(global); return false; }
  std::lock_guard lock(state.mutex);
  if (state.closed) { env->DeleteGlobalRef(global); return false; }
  if (state.type != nullptr) {
    env->DeleteGlobalRef(global);
    return state.vm == vm;
  }
  state.vm = vm;
  state.type = global;
  state.ensure = ensure;
  state.close = close;
  state.quiesced = quiesced;
  return true;
}

std::shared_ptr<window::DesktopRootEvents> RetainDesktopRootClientTarget(
    jlong target_id) {
  // Retain the exact capability while the client registry is locked, then
  // inspect its provider outside that lock. Final target destruction may
  // enqueue AppKit work; its pin therefore outlives the admission scope too.
  std::shared_ptr<DesktopRootTarget> target;
  NativeAdmission admission;
  if (!admission || target_id == 0) return nullptr;
  {
    auto& state = State();
    std::lock_guard lock(state.mutex);
    auto found = state.observations.find(std::bit_cast<uint64_t>(target_id));
    if (state.closed || found == state.observations.end() || found->second->sealed)
      return nullptr;
    target = found->second->target;
  }
  return target == nullptr ? nullptr : target->RetainEvents();
}

bool EnsureDesktopRootClient(JNIEnv* env, jobject input_channel) {
  if (env == nullptr || input_channel == nullptr || env->ExceptionCheck())
    return false;
  auto& state = State();
  jclass local;
  jmethodID method;
  {
    std::lock_guard lock(state.mutex);
    if (state.closed || state.type == nullptr) return false;
    ++state.in_flight;
    local = static_cast<jclass>(env->NewLocalRef(state.type));
    method = state.ensure;
  }
  bool success = local != nullptr && !env->ExceptionCheck();
  if (success) {
    success = env->CallStaticBooleanMethod(local, method, input_channel) ==
                  JNI_TRUE &&
              !env->ExceptionCheck();
  }
  if (local != nullptr) env->DeleteLocalRef(local);
  { std::lock_guard lock(state.mutex); --state.in_flight; }
  return success;
}

bool CloseDesktopRootClientAdmission(JNIEnv* env) {
  if (env == nullptr || env->ExceptionCheck()) return false;
  auto& state = State();
  jclass local = nullptr;
  jmethodID close = nullptr;
  {
    std::lock_guard lock(state.mutex);
    if (!state.closed) {
      state.closed = true;
      for (const auto& [id, observation] : state.observations) {
        (void)id;
        observation->sealed = true;
        QueueUnbindLocked(observation);
      }
    }
    if (state.type == nullptr) return true;
    ++state.in_flight;
    local = static_cast<jclass>(env->NewLocalRef(state.type));
    close = state.close;
  }
  if (local != nullptr && !env->ExceptionCheck()) env->CallStaticVoidMethod(local, close);
  if (local != nullptr) env->DeleteLocalRef(local);
  { std::lock_guard lock(state.mutex); --state.in_flight; }
  return !env->ExceptionCheck();
}

bool PollDesktopRootClientQuiesced(JNIEnv* env) {
  if (env == nullptr || env->ExceptionCheck()) return false;
  auto& state = State();
  std::unique_lock lock(state.mutex);
  if (!state.closed || state.work != 0 || state.in_flight != 0) return false;
  for (const auto& [id, observation] : state.observations) {
    (void)id;
    if (observation->pins.load() != 0) return false;
  }
  if (state.type == nullptr) return true;
  ++state.in_flight;
  jclass local = static_cast<jclass>(env->NewLocalRef(state.type));
  const auto method = state.quiesced;
  lock.unlock();
  const bool ready = local != nullptr && !env->ExceptionCheck() &&
      env->CallStaticBooleanMethod(local, method) == JNI_TRUE && !env->ExceptionCheck();
  if (local != nullptr) env->DeleteLocalRef(local);
  lock.lock();
  --state.in_flight;
  return ready && state.in_flight == 0 && state.work == 0;
}

bool ClearDesktopRootClientReferences(JNIEnv* env) {
  if (env == nullptr || !PollDesktopRootClientQuiesced(env)) return false;
  auto& state = State();
  std::lock_guard lock(state.mutex);
  for (const auto& [id, observation] : state.observations) {
    (void)id;
    if (observation->callback != nullptr) env->DeleteGlobalRef(observation->callback);
  }
  state.observations.clear();
  if (state.type != nullptr) env->DeleteGlobalRef(state.type);
  state.type = nullptr;
  state.ensure = nullptr;
  state.close = nullptr;
  state.quiesced = nullptr;
  state.vm = nullptr;
  return true;
}
}  // namespace darwin_art::framework::wm
