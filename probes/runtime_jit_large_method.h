#pragma once
namespace aim_jni_acceptance_phase {
inline bool CheckJitLargeMethod(JNIEnv* env, art::Thread* self, art::jit::Jit* jit,
                                art::Handle<art::mirror::Class> owner, jclass java_owner) {
  art::jit::ScopedJitSuspend workers;
  auto* method = owner->FindClassMethod("jitLargeArithmetic", "(I)I", art::kRuntimePointerSize);
  auto id = env->GetStaticMethodID(java_owner, "jitLargeArithmetic", "(I)I");
  if (!method || !id || env->ExceptionCheck()) return false;
  art::CodeItemDataAccessor code(*method->GetDexFile(), method->GetCodeItem());
  if (code.InsnsSizeInCodeUnits() <= 256) return false;
  unsigned cases = 0;
  for (int phase = 0; phase < 3; ++phase) {
    if (phase && (!jit->CompileMethod(method, self, phase == 1 ? art::CompilationKind::kBaseline : art::CompilationKind::kOptimized, false) ||
        !jit->GetCodeCache()->ContainsPc(method->GetEntryPointFromQuickCompiledCode()))) {
      std::cerr << "ART JIT large method compile failed phase=" << phase << "\n";
      return false;
    }
    for (jint input : {jint(0), jint(1), jint(-1), jint(INT32_MAX), jint(INT32_MIN), jint(0x12345678)}) {
      uint32_t expected = static_cast<uint32_t>(input);
      for (uint32_t step = 0; step < 96; ++step) expected = (expected * 33u) ^ (step * 12345u + 7u);
      jint actual = env->CallStaticIntMethod(java_owner, id, input);
      if (env->ExceptionCheck() || static_cast<uint32_t>(actual) != expected) {
        std::cerr << "ART JIT large method mismatch phase=" << phase << " input=" << input << "\n";
        return false;
      }
      ++cases;
    }
  }
  std::cerr << "ART JIT large method: DEX units=" << code.InsnsSizeInCodeUnits()
            << " independent wrapping arithmetic PASS cases=" << cases << "\n";
  return true;
}
}  // namespace aim_jni_acceptance_phase
