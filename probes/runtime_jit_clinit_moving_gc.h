#pragma once
#include <atomic>
#include <chrono>
#include <thread>
#include "gc/heap.h"
#include "scoped_thread_state_change-inl.h"
namespace aim_jni_acceptance_phase {
inline bool CheckJitClinitMovingGc(JNIEnv* env, art::Thread* self, art::jit::Jit* jit,
                                  art::Handle<art::mirror::Class> owner, jclass java_owner) {
  const char* names[] = {"jitColdMovingSet", "jitColdMovingGet"};
  const char* sigs[] = {"(Ljava/lang/Object;)V", "()Ljava/lang/Object;"};
  jmethodID ids[2]{};
  art::ArtField* field = nullptr;
  for (int i = 0; i < 2; ++i) {
    ids[i] = env->GetStaticMethodID(java_owner, names[i], sigs[i]);
    auto* method = owner->FindClassMethod(names[i], sigs[i], art::kRuntimePointerSize);
    if (!ids[i] || !method || env->ExceptionCheck()) return false;
    art::CodeItemDataAccessor code(*method->GetDexFile(), method->GetCodeItem());
    const auto* access = art::Instruction::At(code.Insns());
    if (access->Opcode() != (i == 0 ? art::Instruction::SPUT_OBJECT : art::Instruction::SGET_OBJECT)) return false;
    field = art::Runtime::Current()->GetClassLinker()->ResolveField(access->VRegB_21c(), method, true);
    if (!field || env->ExceptionCheck() || field->GetDeclaringClass()->IsInitialized()) return false;
    if (!jit->CompileMethod(method, self, art::CompilationKind::kOptimized, false) ||
        !jit->GetCodeCache()->ContainsPc(method->GetEntryPointFromQuickCompiledCode()) ||
        field->GetDeclaringClass()->IsInitialized()) return false;
  }
  jfieldID count = env->GetStaticFieldID(java_owner, "jitColdMovingCount", "I");
  jfieldID lock_field = env->GetStaticFieldID(java_owner, "jitConcurrentInitializationLock", "Ljava/lang/Object;");
  jobject value = env->AllocObject(java_owner);
  jobject lock = env->AllocObject(java_owner);
  jclass global_owner = static_cast<jclass>(env->NewGlobalRef(java_owner));
  jobject global_value = env->NewGlobalRef(value);
  if (!count || !lock_field || !value || !lock || !global_owner || !global_value || env->ExceptionCheck()) return false;
  env->SetStaticObjectField(java_owner, lock_field, lock);
  if (env->ExceptionCheck() || env->MonitorEnter(lock) != JNI_OK) return false;
  std::atomic<art::Thread*> target{nullptr};
  std::atomic<bool> done{false}, retire{false}, ok{false};
  std::thread worker([&] {
    auto* runtime = art::Runtime::Current();
    if (!runtime->AttachCurrentThread("jit-clinit-moving-gc", true, nullptr, true)) { done.store(true); return; }
    auto* current = art::Thread::Current();
    target.store(current, std::memory_order_release);
    auto* worker_env = current->GetJniEnv();
    worker_env->CallStaticVoidMethod(global_owner, ids[0], global_value);
    ok.store(!worker_env->ExceptionCheck());
    if (worker_env->ExceptionCheck()) { worker_env->ExceptionDescribe(); worker_env->ExceptionClear(); }
    done.store(true, std::memory_order_release);
    while (!retire.load(std::memory_order_acquire)) std::this_thread::yield();
    runtime->DetachCurrentThread();
  });
  bool blocked = false, initializing = false, moving_gc = false, still_blocked = false;
  uintptr_t before = 0, after = 0;
  {
    art::ScopedThreadStateChange native(self, art::ThreadState::kNative);
    auto deadline = std::chrono::steady_clock::now() + std::chrono::seconds(10);
    while (!done.load(std::memory_order_acquire) && std::chrono::steady_clock::now() < deadline) {
      auto* current = target.load(std::memory_order_acquire);
      if (current && current->GetState() == art::ThreadState::kBlocked) { blocked = true; break; }
      std::this_thread::yield();
    }
    if (blocked) {
      {
        art::ScopedObjectAccess soa(self);
        initializing = field->GetDeclaringClass()->GetStatus() == art::ClassStatus::kInitializing;
        before = reinterpret_cast<uintptr_t>(self->DecodeJObject(global_value).Ptr());
      }
      // Concurrent Copying is already a moving collector. Homogeneous-space
      // compaction is a CMS-only transition and AOSP rejects it while CC is
      // active, so exercise the configured collector itself.
      art::Runtime::Current()->GetHeap()->CollectGarbage(false);
      moving_gc = true;
      {
        art::ScopedObjectAccess soa(self);
        after = reinterpret_cast<uintptr_t>(self->DecodeJObject(global_value).Ptr());
      }
      still_blocked = !done.load() && target.load()->GetState() == art::ThreadState::kBlocked;
    }
  }
  bool unlocked = env->MonitorExit(lock) == JNI_OK;
  {
    art::ScopedThreadStateChange native(self, art::ThreadState::kNative);
    retire.store(true, std::memory_order_release); worker.join();
  }
  jobject actual = env->CallStaticObjectMethod(java_owner, ids[1]);
  bool correct = !env->ExceptionCheck() && env->IsSameObject(actual, value) &&
      env->GetStaticIntField(java_owner, count) == 1;
  env->SetStaticObjectField(java_owner, lock_field, nullptr);
  env->DeleteLocalRef(actual); env->DeleteLocalRef(value); env->DeleteLocalRef(lock);
  env->DeleteGlobalRef(global_value); env->DeleteGlobalRef(global_owner);
  std::cerr << "ART JIT clinit moving GC: blocked=" << blocked << " initializing=" << initializing
            << " moving_gc=" << moving_gc << " before=" << reinterpret_cast<void*>(before)
            << " after=" << reinterpret_cast<void*>(after) << " stayed=" << still_blocked
            << " correct=" << correct << "\n";
  return blocked && initializing && moving_gc && before != 0 && before != after && still_blocked &&
      unlocked && ok.load() && correct && !env->ExceptionCheck();
}
}  // namespace aim_jni_acceptance_phase
