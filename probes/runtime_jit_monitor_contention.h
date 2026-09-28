#pragma once

#include <atomic>
#include <chrono>
#include <thread>
#include "monitor.h"
#include "scoped_thread_state_change-inl.h"

namespace aim_jni_acceptance_phase {
inline bool CheckJitMonitorContention(JNIEnv* env, art::Thread* self, jclass owner,
                                      jmethodID method, jobject lock_object,
                                      bool expect_throw = false, bool require_thin = false,
                                      bool instance_call = false) {
  jobject lock = env->NewGlobalRef(lock_object);
  jclass global_owner = static_cast<jclass>(env->NewGlobalRef(owner));
  if (!lock || !global_owner || env->ExceptionCheck()) {
    if (lock) env->DeleteGlobalRef(lock);
    if (global_owner) env->DeleteGlobalRef(global_owner);
    return false;
  }
  if (env->MonitorEnter(lock) != JNI_OK) {
    env->DeleteGlobalRef(global_owner);
    env->DeleteGlobalRef(lock);
    return false;
  }
  const bool began_thin = self->DecodeJObject(lock)->GetLockWord(true).GetState() == art::LockWord::kThinLocked;
  std::atomic<art::Thread*> waiting{nullptr};
  std::atomic<bool> done{false}, retire{false}, worker_ok{false};
  std::thread worker([&] {
    auto* runtime = art::Runtime::Current();
    // Unlike an internal checkpoint worker, this thread executes Java code
    // and needs the ordinary java.lang.Thread peer created by JNI attachment.
    if (!runtime->AttachCurrentThread("jit-monitor-contender", true, nullptr, true)) {
      done.store(true);
      return;
    }
    auto* current = art::Thread::Current();
    JNIEnv* worker_env = current->GetJniEnv();
    waiting.store(current, std::memory_order_release);
    jobject value = instance_call ? worker_env->CallObjectMethod(lock, method, lock)
                                 : worker_env->CallStaticObjectMethod(global_owner, method, lock);
    bool ok;
    if (expect_throw) {
      jthrowable pending = worker_env->ExceptionOccurred();
      worker_env->ExceptionClear();
      ok = pending != nullptr && worker_env->IsSameObject(pending, lock);
      worker_env->DeleteLocalRef(pending);
    } else {
      ok = !worker_env->ExceptionCheck() && worker_env->IsSameObject(value, lock);
    }
    if (worker_env->ExceptionCheck()) {
      worker_env->ExceptionDescribe();
      worker_env->ExceptionClear();
    }
    if (!ok) std::cerr << "ART JIT monitor contender unexpected return=" << value << "\n";
    worker_env->DeleteLocalRef(value);
    worker_ok.store(ok);
    done.store(true, std::memory_order_release);
    // Keep Thread* valid until the observer no longer uses it.
    while (!retire.load(std::memory_order_acquire)) std::this_thread::yield();
    runtime->DetachCurrentThread();
  });
  bool blocked = false, object_matches = false, remained_blocked = false, inflated = false;
  uint64_t collections = 0;
  {
    art::ScopedThreadStateChange native(self, art::ThreadState::kNative);
    const auto deadline = std::chrono::steady_clock::now() + std::chrono::seconds(10);
    art::Thread* target = nullptr;
    while (!done.load(std::memory_order_acquire) && std::chrono::steady_clock::now() < deadline) {
      target = waiting.load(std::memory_order_acquire);
      if (target != nullptr && target->GetState() == art::ThreadState::kBlocked) {
        blocked = true;
        break;
      }
      std::this_thread::yield();
    }
    if (blocked) {
      {
        art::ScopedObjectAccess soa(self);
        if (target->GetState() == art::ThreadState::kBlocked) {
          // State transition publishes monitor_enter_object with release;
          // GetState itself is relaxed. Main still owns the monitor.
          std::atomic_thread_fence(std::memory_order_acquire);
          object_matches = target->GetMonitorEnterObject() == self->DecodeJObject(lock).Ptr();
          inflated = self->DecodeJObject(lock)->GetLockWord(true).GetState() == art::LockWord::kFatLocked;
        }
      }
      auto* heap = art::Runtime::Current()->GetHeap();
      uint64_t before = heap->GetGcCount();
      heap->CollectGarbage(false);
      collections = heap->GetGcCount() - before;
      remained_blocked = target->GetState() == art::ThreadState::kBlocked && !done.load();
    }
  }
  const bool unlocked = env->MonitorExit(lock) == JNI_OK;
  {
    art::ScopedThreadStateChange native(self, art::ThreadState::kNative);
    retire.store(true, std::memory_order_release);
    worker.join();
  }
  const bool released = art::Monitor::GetLockOwnerThreadId(self->DecodeJObject(lock)) == 0;
  env->DeleteGlobalRef(global_owner);
  env->DeleteGlobalRef(lock);
  std::cerr << "ART JIT monitor contention: throwing=" << expect_throw << " thin=" << began_thin
            << " inflated=" << inflated << " blocked=" << blocked << " object=" << object_matches
            << " gc=" << collections << " stayed=" << remained_blocked
            << " worker=" << worker_ok.load() << " released=" << released << "\n";
  return (!require_thin || began_thin) && inflated && blocked && object_matches && collections > 0 && remained_blocked && unlocked &&
      worker_ok.load() && released && !env->ExceptionCheck();
}
}  // namespace aim_jni_acceptance_phase
