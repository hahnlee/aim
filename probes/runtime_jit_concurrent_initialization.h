#pragma once
#include <atomic>
#include <chrono>
#include <thread>
#include "scoped_thread_state_change-inl.h"
namespace aim_jni_acceptance_phase {
inline bool CheckJitConcurrentInitialization(JNIEnv* env, art::Thread* self, art::jit::Jit* jit,
                                             art::Handle<art::mirror::Class> owner, jclass java_owner,
                                             bool fail = false) {
  const char* name = fail ? "jitConcurrentFailedInitialization" : "jitConcurrentInitialization";
  const char* counter = fail ? "jitConcurrentFailedInitializationCount" : "jitConcurrentInitializationCount";
  auto* method = owner->FindClassMethod(name, "()I", art::kRuntimePointerSize);
  jmethodID id = env->GetStaticMethodID(java_owner, name, "()I");
  jfieldID count = env->GetStaticFieldID(java_owner, counter, "I");
  jfieldID lock_field = env->GetStaticFieldID(java_owner, "jitConcurrentInitializationLock", "Ljava/lang/Object;");
  if (!method || !id || !count || !lock_field || env->ExceptionCheck()) return false;
  art::CodeItemDataAccessor code(*method->GetDexFile(), method->GetCodeItem());
  const auto* call = art::Instruction::At(code.Insns());
  if (call->Opcode() != art::Instruction::INVOKE_STATIC) return false;
  auto* callee = art::Runtime::Current()->GetClassLinker()->ResolveMethodId(call->VRegB_35c(), method);
  if (!callee || env->ExceptionCheck() || callee->GetDeclaringClass()->IsInitialized()) return false;
  if (!jit->CompileMethod(method, self, art::CompilationKind::kOptimized, false) ||
      !jit->GetCodeCache()->ContainsPc(method->GetEntryPointFromQuickCompiledCode()) ||
      callee->GetDeclaringClass()->IsInitialized()) return false;
  jclass global_owner = static_cast<jclass>(env->NewGlobalRef(java_owner));
  jobject lock = env->AllocObject(java_owner);
  if (!global_owner || !lock || env->ExceptionCheck()) return false;
  env->SetStaticObjectField(java_owner, lock_field, lock);
  if (env->ExceptionCheck() || env->MonitorEnter(lock) != JNI_OK) return false;
  std::atomic<art::Thread*> threads[2]{};
  std::atomic<bool> done[2]{}, ok[2]{};
  std::atomic<bool> start_second{false}, retire{false};
  auto run = [&](int i) {
    auto* runtime = art::Runtime::Current();
    if (!runtime->AttachCurrentThread("jit-class-initializer", true, nullptr, true)) {
      done[i].store(true); return;
    }
    auto* current = art::Thread::Current();
    threads[i].store(current, std::memory_order_release);
    if (i == 1) while (!start_second.load(std::memory_order_acquire)) std::this_thread::yield();
    JNIEnv* worker_env = current->GetJniEnv();
    jint value = worker_env->CallStaticIntMethod(global_owner, id);
    if (fail) {
      jthrowable error = worker_env->ExceptionOccurred();
      worker_env->ExceptionClear();
      jclass expected = worker_env->FindClass(i == 0 ? "java/lang/ExceptionInInitializerError" : "java/lang/NoClassDefFoundError");
      ok[i].store(error && expected && !worker_env->ExceptionCheck() && worker_env->IsInstanceOf(error, expected));
      if (error) worker_env->DeleteLocalRef(error);
      if (expected) worker_env->DeleteLocalRef(expected);
    } else {
      ok[i].store(!worker_env->ExceptionCheck() && value == 74);
    }
    if (worker_env->ExceptionCheck()) { worker_env->ExceptionDescribe(); worker_env->ExceptionClear(); }
    done[i].store(true, std::memory_order_release);
    while (!retire.load(std::memory_order_acquire)) std::this_thread::yield();
    runtime->DetachCurrentThread();
  };
  std::thread first(run, 0), second(run, 1);
  bool blocked = false, waiting = false, initializing = false;
  {
    art::ScopedThreadStateChange native(self, art::ThreadState::kNative);
    auto wait_state = [&](int i, art::ThreadState state) {
      auto deadline = std::chrono::steady_clock::now() + std::chrono::seconds(10);
      while (!done[i].load(std::memory_order_acquire) && std::chrono::steady_clock::now() < deadline) {
        auto* target = threads[i].load(std::memory_order_acquire);
        if (target && target->GetState() == state) return true;
        std::this_thread::yield();
      }
      return false;
    };
    blocked = wait_state(0, art::ThreadState::kBlocked);
    start_second.store(true, std::memory_order_release);
    waiting = wait_state(1, art::ThreadState::kWaiting);
    {
      art::ScopedObjectAccess soa(self);
      initializing = callee->GetDeclaringClass()->GetStatus() == art::ClassStatus::kInitializing;
    }
  }
  bool held = !done[0].load() && !done[1].load();
  bool unlocked = env->MonitorExit(lock) == JNI_OK;
  {
    art::ScopedThreadStateChange native(self, art::ThreadState::kNative);
    retire.store(true, std::memory_order_release);
    first.join(); second.join();
  }
  bool once = env->GetStaticIntField(java_owner, count) == 1;
  bool initialized = fail ? callee->GetDeclaringClass()->IsErroneous() : callee->GetDeclaringClass()->IsInitialized();
  env->SetStaticObjectField(java_owner, lock_field, nullptr);
  env->DeleteLocalRef(lock); env->DeleteGlobalRef(global_owner);
  std::cerr << "ART JIT concurrent initialization: fail=" << fail << " blocked=" << blocked << " waiting=" << waiting
            << " initializing=" << initializing << " held=" << held << " once=" << once
            << " values=" << ok[0].load() << "," << ok[1].load() << "\n";
  return blocked && waiting && initializing && held && unlocked && once && initialized &&
      ok[0].load() && ok[1].load() && !env->ExceptionCheck();
}
}  // namespace aim_jni_acceptance_phase
