#pragma once
#include "runtime_jit_monitor_contention.h"
namespace aim_jni_acceptance_phase {
inline bool CheckJitSynchronized(JNIEnv* env, art::Thread* self, art::jit::Jit* jit,
                                 art::Handle<art::mirror::Class> owner, jclass java_owner) {
  art::jit::ScopedJitSuspend workers;
  const char* names[] = {"jitSynchronizedStatic", "jitSynchronizedInstance", "jitSynchronizedStaticThrow", "jitSynchronizedInstanceThrow"};
  const char* sig = "(Ljava/lang/Object;)Ljava/lang/Object;";
  art::ArtMethod* methods[4]{};
  jmethodID ids[4]{};
  jobject receiver = env->AllocObject(java_owner);
  if (!receiver || env->ExceptionCheck()) return false;
  for (int i = 0; i < 4; ++i) {
    const char* signature = i < 2 ? sig : "(Ljava/lang/Throwable;)Ljava/lang/Object;";
    methods[i] = owner->FindClassMethod(names[i], signature, art::kRuntimePointerSize);
    ids[i] = i % 2 == 0 ? env->GetStaticMethodID(java_owner, names[i], signature) : env->GetMethodID(java_owner, names[i], signature);
    if (!methods[i] || !ids[i] || env->ExceptionCheck() || !methods[i]->IsSynchronized()) return false;
    art::CodeItemDataAccessor code(*methods[i]->GetDexFile(), methods[i]->GetCodeItem());
    bool enter = false, exit = false;
    for (auto instruction : code) {
      enter |= instruction.Inst().Opcode() == art::Instruction::MONITOR_ENTER;
      exit |= instruction.Inst().Opcode() == art::Instruction::MONITOR_EXIT;
    }
    if (!enter || !exit) return false;
  }
  jclass throwable = env->FindClass("java/lang/IllegalArgumentException");
  jclass npe = env->FindClass("java/lang/NullPointerException");
  if (!throwable || !npe || env->ExceptionCheck()) return false;
  auto constructor = env->GetMethodID(throwable, "<init>", "()V");
  if (!constructor || env->ExceptionCheck()) return false;
  jobject exception_value = env->NewObject(throwable, constructor);
  if (!exception_value || env->ExceptionCheck()) return false;
  unsigned cases = 0;
  for (int phase = 0; phase < 3; ++phase) {
    if (phase) for (auto* method : methods) {
      if (!jit->CompileMethod(method, self, phase == 1 ? art::CompilationKind::kBaseline : art::CompilationKind::kOptimized, false) ||
          !jit->GetCodeCache()->ContainsPc(method->GetEntryPointFromQuickCompiledCode())) {
        std::cerr << "ART JIT synchronized compile failed: " << method->PrettyMethod() << "\n";
        return false;
      }
    }
    for (int kind = 0; kind < 2; ++kind) for (jobject value : {jobject(nullptr), receiver, jobject(java_owner)}) {
      jobject lock = kind == 0 ? jobject(java_owner) : receiver;
      if (env->MonitorEnter(lock) != JNI_OK) return false;
      uint64_t before = art::Runtime::Current()->GetHeap()->GetGcCount();
      jobject result = kind == 0 ? env->CallStaticObjectMethod(java_owner, ids[kind], value)
                                 : env->CallObjectMethod(receiver, ids[kind], value);
      bool correct = !env->ExceptionCheck() && env->IsSameObject(result, value) &&
          art::Runtime::Current()->GetHeap()->GetGcCount() > before &&
          art::Monitor::GetLockOwnerThreadId(self->DecodeJObject(lock)) == self->GetThreadId();
      if (result) env->DeleteLocalRef(result);
      bool exited = env->MonitorExit(lock) == JNI_OK;
      if (!correct || !exited || art::Monitor::GetLockOwnerThreadId(self->DecodeJObject(lock)) != 0) return false;
      ++cases;
    }
    if (!CheckJitMonitorContention(env, self, java_owner, ids[0], java_owner)) return false;
    if (!CheckJitMonitorContention(env, self, java_owner, ids[1], receiver, false, false, true)) return false;
    for (int kind = 0; kind < 2; ++kind) for (jobject value : {jobject(nullptr), exception_value}) {
      jobject lock = kind == 0 ? jobject(java_owner) : receiver;
      if (env->MonitorEnter(lock) != JNI_OK) return false;
      uint64_t before = art::Runtime::Current()->GetHeap()->GetGcCount();
      jobject result = kind == 0 ? env->CallStaticObjectMethod(java_owner, ids[kind + 2], value)
                                 : env->CallObjectMethod(receiver, ids[kind + 2], value);
      jthrowable pending = env->ExceptionOccurred();
      if (pending) env->ExceptionClear();
      bool correct = pending && (value ? env->IsSameObject(pending, value) : env->IsInstanceOf(pending, npe)) &&
          art::Runtime::Current()->GetHeap()->GetGcCount() > before &&
          art::Monitor::GetLockOwnerThreadId(self->DecodeJObject(lock)) == self->GetThreadId();
      if (result) env->DeleteLocalRef(result);
      if (pending) env->DeleteLocalRef(pending);
      bool exited = env->MonitorExit(lock) == JNI_OK;
      if (!correct || !exited || art::Monitor::GetLockOwnerThreadId(self->DecodeJObject(lock)) != 0) return false;
      ++cases;
    }
  }
  env->DeleteLocalRef(exception_value);
  env->DeleteLocalRef(throwable);
  env->DeleteLocalRef(npe);
  env->DeleteLocalRef(receiver);
  std::cerr << "ART JIT synchronized: static/instance DEX monitors/reentrancy/GC/exception release + static/instance contention PASS cases=" << cases << "\n";
  return !env->ExceptionCheck();
}
}  // namespace aim_jni_acceptance_phase
