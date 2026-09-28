#pragma once
namespace aim_jni_acceptance_phase {
inline bool CheckJitCallArguments(JNIEnv* env, art::Thread* self, art::jit::Jit* jit,
                                  art::Handle<art::mirror::Class> owner, jclass java_owner) {
  const char* names[] = {"jitCallReordered", "jitCallDuplicated", "jitCallDropped"};
  const char* sigs[] = {"(II)I", "(I)I", "(II)I"};
  const jint inputs[] = {0, 1, -1, 17, INT32_MIN, INT32_MAX};
  unsigned cases = 0;
  for (int kind = 0; kind < 3; ++kind) {
    auto* method = owner->FindClassMethod(names[kind], sigs[kind], art::kRuntimePointerSize);
    jmethodID id = env->GetStaticMethodID(java_owner, names[kind], sigs[kind]);
    if (!method || !id || env->ExceptionCheck() ||
        jit->GetCodeCache()->ContainsPc(method->GetEntryPointFromQuickCompiledCode())) return false;
    for (int phase = 0; phase < 2; ++phase) {
      if (phase == 1 && (!jit->CompileMethod(method, self, art::CompilationKind::kOptimized, false) ||
          !jit->GetCodeCache()->ContainsPc(method->GetEntryPointFromQuickCompiledCode()))) return false;
      for (jint x : inputs) for (jint y : inputs) {
        uint32_t a = kind == 0 ? static_cast<uint32_t>(y) : static_cast<uint32_t>(x);
        uint32_t b = kind == 0 ? static_cast<uint32_t>(x) : kind == 1 ? static_cast<uint32_t>(x) : 7u;
        uint32_t expected = (a * 31u + b) ^ (a >> (b & 31u));
        jint actual = kind == 1 ? env->CallStaticIntMethod(java_owner, id, x)
                               : env->CallStaticIntMethod(java_owner, id, x, y);
        if (env->ExceptionCheck() || static_cast<uint32_t>(actual) != expected) return false;
        ++cases;
      }
    }
  }
  std::cerr << "ART JIT static arguments: reordered/duplicated/dropped and unsigned oracle PASS cases=" << cases << "\n";
  return true;
}
}  // namespace aim_jni_acceptance_phase
