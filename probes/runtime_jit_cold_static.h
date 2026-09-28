#pragma once
namespace aim_jni_acceptance_phase {
inline bool CheckJitColdStatic(JNIEnv* env, art::Thread* self, art::jit::Jit* jit,
                               art::Handle<art::mirror::Class> owner, jclass java_owner) {
  auto* method = owner->FindClassMethod("jitColdStaticRead", "()I", art::kRuntimePointerSize);
  jmethodID id = env->GetStaticMethodID(java_owner, "jitColdStaticRead", "()I");
  jfieldID count = env->GetStaticFieldID(java_owner, "jitStaticInitializationCount", "I");
  if (!method || !id || !count || env->ExceptionCheck()) return false;
  art::CodeItemDataAccessor code(*method->GetDexFile(), method->GetCodeItem());
  const auto* access = art::Instruction::At(code.Insns());
  if (access->Opcode() != art::Instruction::SGET) return false;
  auto* field = art::Runtime::Current()->GetClassLinker()->ResolveField(access->VRegB_21c(), method, true);
  if (!field || env->ExceptionCheck() || field->GetDeclaringClass()->IsInitialized() ||
      env->GetStaticIntField(java_owner, count) != 0) return false;
  if (!jit->CompileMethod(method, self, art::CompilationKind::kOptimized, false) ||
      !jit->GetCodeCache()->ContainsPc(method->GetEntryPointFromQuickCompiledCode()) ||
      field->GetDeclaringClass()->IsInitialized() || env->GetStaticIntField(java_owner, count) != 0) return false;
  for (int i = 0; i < 3; ++i) {
    if (env->CallStaticIntMethod(java_owner, id) != 93 || env->ExceptionCheck() ||
        env->GetStaticIntField(java_owner, count) != 1) return false;
  }
  if (!field->GetDeclaringClass()->IsInitialized()) return false;
  std::cerr << "ART JIT cold static: SGET cold through compilation, value=93 once-only PASS\n";
  return true;
}
}  // namespace aim_jni_acceptance_phase
