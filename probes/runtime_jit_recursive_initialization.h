#pragma once
namespace aim_jni_acceptance_phase {
inline bool CheckJitRecursiveInitialization(JNIEnv* env, art::Thread* self, art::jit::Jit* jit,
                                            art::Handle<art::mirror::Class> owner, jclass java_owner) {
  auto* method = owner->FindClassMethod("jitRecursiveInitialization", "()I", art::kRuntimePointerSize);
  jmethodID id = env->GetStaticMethodID(java_owner, "jitRecursiveInitialization", "()I");
  jfieldID count = env->GetStaticFieldID(java_owner, "jitRecursiveInitializationCount", "I");
  jfieldID observed = env->GetStaticFieldID(java_owner, "jitRecursiveInitializationObserved", "I");
  if (!method || !id || !count || !observed || env->ExceptionCheck()) return false;
  art::CodeItemDataAccessor code(*method->GetDexFile(), method->GetCodeItem());
  const auto* call = art::Instruction::At(code.Insns());
  if (call->Opcode() != art::Instruction::INVOKE_STATIC) return false;
  auto* callee = art::Runtime::Current()->GetClassLinker()->ResolveMethodId(call->VRegB_35c(), method);
  if (!callee || env->ExceptionCheck() || callee->GetDeclaringClass()->IsInitialized() ||
      env->GetStaticIntField(java_owner, count) != 0) return false;
  if (!jit->CompileMethod(method, self, art::CompilationKind::kOptimized, false) ||
      !jit->GetCodeCache()->ContainsPc(method->GetEntryPointFromQuickCompiledCode()) ||
      callee->GetDeclaringClass()->IsInitialized() || env->GetStaticIntField(java_owner, count) != 0) return false;
  for (int i = 0; i < 3; ++i) {
    if (env->CallStaticIntMethod(java_owner, id) != 74 || env->ExceptionCheck() ||
        env->GetStaticIntField(java_owner, count) != 1 ||
        env->GetStaticIntField(java_owner, observed) != 1) return false;
  }
  if (!callee->GetDeclaringClass()->IsInitialized() ||
      !jit->GetCodeCache()->ContainsPc(method->GetEntryPointFromQuickCompiledCode())) return false;
  std::cerr << "ART JIT recursive initialization: cold compile, inner default=1 outer=74 once-only PASS\n";
  return true;
}
}  // namespace aim_jni_acceptance_phase
