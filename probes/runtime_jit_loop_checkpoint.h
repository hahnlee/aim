#pragma once

#include <atomic>
#include <chrono>
#include <thread>
#include "base/mutex.h"
#include "oat/oat_quick_method_header.h"
#include "stack.h"
#include "thread_pool.h"
#include "thread_list.h"

namespace aim_jni_acceptance_phase {

inline bool CheckEmptyCheckpointMutexContention(art::Thread* self) {
  art::Mutex mutex("empty-checkpoint contention acceptance");
  mutex.SetShouldRespondToEmptyCheckpointRequest(true);
  std::atomic<bool> holder_ready{false};
  std::atomic<bool> main_about_to_lock{false};
  std::atomic<bool> checkpoint_done{false};
  std::atomic<bool> requester_attached{false};
  std::atomic<int64_t> checkpoint_us{-1};

  // Keep the mutex unavailable long enough to distinguish Darwin's 100us
  // checkpoint polling from merely returning after the native lock is free.
  std::thread holder([&] {
    mutex.ExclusiveLock(nullptr);
    holder_ready.store(true, std::memory_order_release);
    std::this_thread::sleep_for(std::chrono::milliseconds(500));
    mutex.ExclusiveUnlock(nullptr);
  });
  std::thread requester([&] {
    auto* runtime = art::Runtime::Current();
    if (!runtime->AttachCurrentThread("empty-checkpoint-requester", true, nullptr, false)) return;
    requester_attached.store(true, std::memory_order_release);
    while (!main_about_to_lock.load(std::memory_order_acquire)) std::this_thread::yield();
    const auto start = std::chrono::steady_clock::now();
    runtime->GetThreadList()->RunEmptyCheckpoint();
    checkpoint_us.store(std::chrono::duration_cast<std::chrono::microseconds>(
                            std::chrono::steady_clock::now() - start)
                            .count(),
                        std::memory_order_release);
    checkpoint_done.store(true, std::memory_order_release);
    runtime->DetachCurrentThread();
  });

  while (!holder_ready.load(std::memory_order_acquire) ||
         !requester_attached.load(std::memory_order_acquire)) {
    std::this_thread::yield();
  }
  const auto lock_start = std::chrono::steady_clock::now();
  main_about_to_lock.store(true, std::memory_order_release);
  mutex.ExclusiveLock(self);
  const auto lock_us = std::chrono::duration_cast<std::chrono::microseconds>(
                           std::chrono::steady_clock::now() - lock_start)
                           .count();
  mutex.ExclusiveUnlock(self);
  holder.join();
  requester.join();

  // The holder delay proves the mutex was genuinely contended. The checkpoint
  // must complete well before release, while the normal lock still succeeds.
  const int64_t observed_checkpoint_us = checkpoint_us.load(std::memory_order_acquire);
  const bool ok = checkpoint_done.load(std::memory_order_acquire) &&
                  observed_checkpoint_us >= 0 && observed_checkpoint_us < 250'000 &&
                  lock_us >= 250'000 && lock_us < 2'000'000;
  if (ok) {
    std::cerr << "ART empty checkpoint mutex contention PASS checkpoint_us="
              << observed_checkpoint_us << " lock_us=" << lock_us << "\n";
  } else {
    std::cerr << "ART empty checkpoint mutex contention failed checkpoint_done="
              << checkpoint_done.load() << " checkpoint_us=" << observed_checkpoint_us
              << " lock_us=" << lock_us << "\n";
  }
  return ok;
}

class LoopCheckpoint final : public art::Closure {
 public:
  explicit LoopCheckpoint(art::ArtMethod* method) : method_(method) {}
  void Run(art::Thread* thread) override {
    class Visitor final : public art::StackVisitor {
     public:
      Visitor(art::Thread* thread, art::ArtMethod* method)
          : StackVisitor(thread, nullptr, StackWalkKind::kSkipInlinedFrames), method_(method) {}
      bool VisitFrame() override REQUIRES_SHARED(art::Locks::mutator_lock_) {
        auto* method = GetMethod();
        if (method == nullptr || method->IsRuntimeMethod()) return true;
        // Only the top managed activation counts. A parent GC call with the
        // constructor below it must not masquerade as a loop safepoint.
        if (method == method_ && !IsShadowFrame()) {
          const auto* header = GetCurrentOatQuickMethodHeader();
          const uintptr_t address = GetCurrentQuickFramePc();
          if (header != nullptr && header->IsOptimized() &&
              art::Runtime::Current()->GetJit()->GetCodeCache()->ContainsPc(
                  reinterpret_cast<const void*>(address))) {
            pc = GetDexPc(false);
            native_pc = address;
          }
        }
        return false;
      }
      uint32_t pc = art::dex::kDexNoIndex;
      uintptr_t native_pc = 0;
     private:
      art::ArtMethod* method_;
    } visitor(thread, method_);
    visitor.WalkStack();
    pc.store(visitor.pc, std::memory_order_relaxed);
    native_pc.store(visitor.native_pc, std::memory_order_relaxed);
    done.store(true, std::memory_order_release);
  }
  std::atomic<bool> done{false};
  std::atomic<uint32_t> pc{art::dex::kDexNoIndex};
  std::atomic<uintptr_t> native_pc{0};
 private:
  art::ArtMethod* method_;
};

inline bool CheckJitLoopCheckpoint(JNIEnv* env, art::Thread* self, art::jit::Jit* jit,
                                  jclass owner, jmethodID factory_id,
                                  art::ArtMethod* constructor, art::ArtMethod* factory,
                                  jfieldID payload_field, jfieldID computed_field) {
  if (!CheckEmptyCheckpointMutexContention(self)) return false;
  uint32_t loop_begin = art::dex::kDexNoIndex, loop_end = 0;
  art::CodeItemDataAccessor code(*constructor->GetDexFile(), constructor->GetCodeItem());
  for (auto pair : code) {
    if (pair.Inst().IsBranch() && pair.Inst().GetTargetOffset() < 0) {
      const uint32_t target = pair.DexPc() + pair.Inst().GetTargetOffset();
      loop_begin = std::min(loop_begin, target);
      loop_end = std::max(loop_end, pair.DexPc());
    }
  }
  if (loop_begin == art::dex::kDexNoIndex) return false;
  jintArray payload = env->NewIntArray(1);
  const jint marker = -123456789;
  if (payload == nullptr || env->ExceptionCheck()) return false;
  env->SetIntArrayRegion(payload, 0, 1, &marker);
  LoopCheckpoint checkpoint(constructor);  // Alive until worker join and all queued callbacks drain.
  std::atomic<bool> call_done{false}, worker_ok{false};
  std::atomic<uint32_t> before_pc{art::dex::kDexNoIndex}, after_pc{art::dex::kDexNoIndex};
  std::atomic<uintptr_t> before_native_pc{0}, after_native_pc{0};
  std::atomic<uint64_t> collections{0};
  std::thread worker([&] {
    auto* runtime = art::Runtime::Current();
    if (!runtime->AttachCurrentThread("jit-loop-checkpoint", true, nullptr, false)) return;
    auto* current = art::Thread::Current();
    const auto deadline = std::chrono::steady_clock::now() + std::chrono::seconds(10);
    auto observe_loop = [&]() {
      while (!call_done.load() && std::chrono::steady_clock::now() < deadline) {
        checkpoint.done.store(false, std::memory_order_relaxed);
        bool queued;
        {
          art::MutexLock lock(current, *art::Locks::thread_suspend_count_lock_);
          queued = self->RequestCheckpoint(&checkpoint);
        }
        if (!queued) { std::this_thread::yield(); continue; }
        // Never abandon a queued closure. If the loop returns, the target's
        // transition to Native before join drains its pending checkpoints.
        while (!checkpoint.done.load(std::memory_order_acquire)) std::this_thread::yield();
        const uint32_t pc = checkpoint.pc.load();
        if (pc >= loop_begin && pc <= loop_end) return pc;
      }
      return art::dex::kDexNoIndex;
    };
    before_pc.store(observe_loop());
    before_native_pc.store(checkpoint.native_pc.load());
    if (before_pc.load() != art::dex::kDexNoIndex) {
      const uint64_t before_gc = runtime->GetHeap()->GetGcCount();
      // GC runs outside the callback with no locks held. The callback's return
      // epilogue may still be finishing; GC waits for the normal safepoint.
      runtime->GetHeap()->CollectGarbage(false);
      after_pc.store(observe_loop());
      after_native_pc.store(checkpoint.native_pc.load());
      collections.store(runtime->GetHeap()->GetGcCount() - before_gc);
      worker_ok.store(runtime->GetHeap()->GetGcCount() > before_gc &&
                      after_pc.load() != art::dex::kDexNoIndex);
    }
    runtime->DetachCurrentThread();
  });
  constexpr jint iterations = 500000000;
  jobject holder = env->CallStaticObjectMethod(owner, factory_id, payload, iterations);
  call_done.store(true);
  {
    art::ScopedThreadStateChange native(self, art::ThreadState::kNative);
    worker.join();
  }
  if (holder == nullptr || env->ExceptionCheck() || !worker_ok.load()) {
    std::cerr << "ART JIT loop checkpoint failed: before=" << before_pc.load()
              << " after=" << after_pc.load() << " interval=" << loop_begin << ".." << loop_end << "\n";
    return false;
  }
  uint32_t expected = 19;
  for (uint32_t i = 1; i <= static_cast<uint32_t>(iterations); ++i) expected = (expected * 31u) ^ i;
  jobject actual = env->GetObjectField(holder, payload_field);
  jint recovered = 0;
  env->GetIntArrayRegion(payload, 0, 1, &recovered);
  const bool ok = !env->ExceptionCheck() && env->IsSameObject(actual, payload) && recovered == marker &&
      static_cast<uint32_t>(env->GetIntField(holder, computed_field)) == expected &&
      jit->GetCodeCache()->ContainsPc(constructor->GetEntryPointFromQuickCompiledCode()) &&
      jit->GetCodeCache()->ContainsPc(factory->GetEntryPointFromQuickCompiledCode());
  env->DeleteLocalRef(actual);
  env->DeleteLocalRef(holder);
  env->DeleteLocalRef(payload);
  if (ok) std::cerr << "ART JIT loop checkpoint/worker GC PASS before_dex_pc=" << before_pc.load()
                    << " after_dex_pc=" << after_pc.load()
                    << " before_native_pc=" << reinterpret_cast<const void*>(before_native_pc.load())
                    << " after_native_pc=" << reinterpret_cast<const void*>(after_native_pc.load())
                    << " collections=" << collections.load() << " iterations=" << iterations << "\n";
  return ok;
}
}  // namespace aim_jni_acceptance_phase
