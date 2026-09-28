#pragma once
namespace aim_jni_acceptance_phase {
inline bool CheckJitFailedInitialization(JNIEnv* env, art::Thread* self, art::jit::Jit* jit,
                                         art::Handle<art::mirror::Class> owner, jclass java_owner) {
  auto* method = owner->FindClassMethod("jitFailedInitialization", "()I", art::kRuntimePointerSize);
  jmethodID id = env->GetStaticMethodID(java_owner, "jitFailedInitialization", "()I");
  jfieldID count = env->GetStaticFieldID(java_owner, "jitFailedInitializationCount", "I");
  jclass initial_error = env->FindClass("java/lang/ExceptionInInitializerError");
  jclass repeat_error = env->FindClass("java/lang/NoClassDefFoundError");
  jclass arithmetic = env->FindClass("java/lang/ArithmeticException");
  if (!method || !id || !count || !initial_error || !repeat_error || !arithmetic || env->ExceptionCheck()) return false;
  jmethodID get_cause = env->GetMethodID(initial_error, "getCause", "()Ljava/lang/Throwable;");
  if (!get_cause || env->ExceptionCheck()) return false;
  art::CodeItemDataAccessor code(*method->GetDexFile(), method->GetCodeItem());
  const auto* call = art::Instruction::At(code.Insns());
  if (call->Opcode() != art::Instruction::INVOKE_STATIC) return false;
  auto* callee = art::Runtime::Current()->GetClassLinker()->ResolveMethodId(call->VRegB_35c(), method);
  if (!callee || env->ExceptionCheck() || callee->GetDeclaringClass()->IsInitialized() ||
      env->GetStaticIntField(java_owner, count) != 0) return false;
  if (!jit->CompileMethod(method, self, art::CompilationKind::kOptimized, false) ||
      !jit->GetCodeCache()->ContainsPc(method->GetEntryPointFromQuickCompiledCode())) return false;
  if (callee->GetDeclaringClass()->IsInitialized() || env->GetStaticIntField(java_owner, count) != 0) return false;
  for (int i = 0; i < 3; ++i) {
    env->CallStaticIntMethod(java_owner, id);
    if (!env->ExceptionCheck()) return false;
    jthrowable error = env->ExceptionOccurred();
    env->ExceptionClear();
    bool correct = env->IsInstanceOf(error, i == 0 ? initial_error : repeat_error);
    if (i == 0) {
      jobject cause = env->CallObjectMethod(error, get_cause);
      correct = correct && !env->ExceptionCheck() && cause && env->IsInstanceOf(cause, arithmetic);
      if (cause) env->DeleteLocalRef(cause);
    }
    env->DeleteLocalRef(error);
    if (!correct || env->ExceptionCheck() || env->GetStaticIntField(java_owner, count) != 1 ||
        !callee->GetDeclaringClass()->IsErroneous()) return false;
  }
  env->DeleteLocalRef(initial_error);
  env->DeleteLocalRef(repeat_error);
  env->DeleteLocalRef(arithmetic);
  std::cerr << "ART JIT failed initialization: cold compile, initializer cause, repeat NCDFE, once-only PASS\n";
  return true;
}
}  // namespace aim_jni_acceptance_phase
