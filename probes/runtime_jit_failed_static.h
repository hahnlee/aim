#pragma once
namespace aim_jni_acceptance_phase {
inline bool CheckJitFailedStatic(JNIEnv* env, art::Thread* self, art::jit::Jit* jit,
                                 art::Handle<art::mirror::Class> owner, jclass java_owner) {
  const char* names[] = {"jitFailedStaticRead", "jitFailedStaticWrite"};
  const char* sigs[] = {"()J", "(J)V"};
  const char* counts[] = {"jitFailedStaticReadCount", "jitFailedStaticWriteCount"};
  jclass initial = env->FindClass("java/lang/ExceptionInInitializerError");
  jclass repeated = env->FindClass("java/lang/NoClassDefFoundError");
  jclass arithmetic = env->FindClass("java/lang/ArithmeticException");
  if (!initial || !repeated || !arithmetic || env->ExceptionCheck()) return false;
  jmethodID get_cause = env->GetMethodID(initial, "getCause", "()Ljava/lang/Throwable;");
  if (!get_cause || env->ExceptionCheck()) return false;
  for (int op = 0; op < 2; ++op) {
    auto* method = owner->FindClassMethod(names[op], sigs[op], art::kRuntimePointerSize);
    jmethodID id = env->GetStaticMethodID(java_owner, names[op], sigs[op]);
    jfieldID count = env->GetStaticFieldID(java_owner, counts[op], "I");
    if (!method || !id || !count || env->ExceptionCheck()) return false;
    art::CodeItemDataAccessor code(*method->GetDexFile(), method->GetCodeItem());
    const auto* access = art::Instruction::At(code.Insns());
    if (access->Opcode() != (op == 0 ? art::Instruction::SGET_WIDE : art::Instruction::SPUT_WIDE)) return false;
    auto* field = art::Runtime::Current()->GetClassLinker()->ResolveField(access->VRegB_21c(), method, true);
    if (!field || env->ExceptionCheck() || field->GetDeclaringClass()->IsInitialized() ||
        env->GetStaticIntField(java_owner, count) != 0) return false;
    if (!jit->CompileMethod(method, self, art::CompilationKind::kOptimized, false) ||
        !jit->GetCodeCache()->ContainsPc(method->GetEntryPointFromQuickCompiledCode()) ||
        field->GetDeclaringClass()->IsInitialized() || env->GetStaticIntField(java_owner, count) != 0) return false;
    for (int repeat = 0; repeat < 3; ++repeat) {
      if (op == 0) env->CallStaticLongMethod(java_owner, id);
      else env->CallStaticVoidMethod(java_owner, id, static_cast<jlong>(0x12345678abcdef01LL));
      if (!env->ExceptionCheck()) return false;
      jthrowable error = env->ExceptionOccurred(); env->ExceptionClear();
      bool correct = env->IsInstanceOf(error, repeat == 0 ? initial : repeated);
      if (repeat == 0) {
        jobject cause = env->CallObjectMethod(error, get_cause);
        correct = correct && !env->ExceptionCheck() && cause && env->IsInstanceOf(cause, arithmetic);
        if (cause) env->DeleteLocalRef(cause);
      }
      env->DeleteLocalRef(error);
      if (!correct || env->ExceptionCheck() || env->GetStaticIntField(java_owner, count) != 1 ||
          !field->GetDeclaringClass()->IsErroneous() || field->GetLong(field->GetDeclaringClass()) != 0) return false;
    }
  }
  env->DeleteLocalRef(initial); env->DeleteLocalRef(repeated); env->DeleteLocalRef(arithmetic);
  std::cerr << "ART JIT failed static: cold read/write, initializer cause, repeat NCDFE, no-store PASS\n";
  return true;
}
}  // namespace aim_jni_acceptance_phase
