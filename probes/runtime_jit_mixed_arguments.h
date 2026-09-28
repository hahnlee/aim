#pragma once
namespace aim_jni_acceptance_phase {
inline bool CheckJitMixedArguments(JNIEnv* env, art::Thread* self, art::jit::Jit* jit,
                                   art::Handle<art::mirror::Class> owner, jclass java_owner) {
  art::jit::ScopedJitSuspend workers;
  const char* names[] = {"jitMixedPermutation", "jitMixedDropped"};
  const char* sigs[] = {"(JDLjava/lang/Object;)Ljava/lang/Object;",
      "(Ljava/lang/Object;Ljava/lang/Object;JD)Ljava/lang/Object;"};
  art::ArtMethod* methods[2]{};
  jmethodID ids[2]{};
  auto* target = owner->FindClassMethod("jitMixedPermutationTarget",
      "(Ljava/lang/Object;JD)Ljava/lang/Object;", art::kRuntimePointerSize);
  if (!target || jit->GetCodeCache()->ContainsPc(target->GetEntryPointFromQuickCompiledCode())) return false;
  art::CodeItemDataAccessor target_code(*target->GetDexFile(), target->GetCodeItem());
  // Pinned AOSP CompilerOptions::kBaselineInlineMaxCodeUnits is 14.
  // Require a real baseline call, not an inlined replacement of the target.
  if (target_code.InsnsSizeInCodeUnits() <= 14) return false;
  for (int i = 0; i < 2; ++i) {
    ids[i] = env->GetStaticMethodID(java_owner, names[i], sigs[i]);
    methods[i] = owner->FindClassMethod(names[i], sigs[i], art::kRuntimePointerSize);
    if (!ids[i] || !methods[i] || env->ExceptionCheck()) return false;
  }
  jobject value = env->AllocObject(java_owner);
  if (!value || env->ExceptionCheck()) return false;
  unsigned cases = 0;
  for (int phase = 0; phase < 4; ++phase) {
    if (phase == 1 || phase == 3) {
      for (auto* method : methods) {
        if (!jit->CompileMethod(method, self, phase == 1 ? art::CompilationKind::kBaseline : art::CompilationKind::kOptimized, false) ||
            !jit->GetCodeCache()->ContainsPc(method->GetEntryPointFromQuickCompiledCode())) return false;
      }
    }
    if (phase == 1 && jit->GetCodeCache()->ContainsPc(target->GetEntryPointFromQuickCompiledCode())) return false;
    if (phase == 2 && (!jit->CompileMethod(target, self, art::CompilationKind::kOptimized, false) ||
        !jit->GetCodeCache()->ContainsPc(target->GetEntryPointFromQuickCompiledCode()))) return false;
    for (int kind = 0; kind < 2; ++kind) {
      for (jlong tag : {jlong{0x12345678abcdef01LL}, jlong{-1}, jlong{0}}) {
        for (jdouble real : {jdouble{-123.25}, jdouble{0.0}, jdouble{-0.0}}) {
          for (jobject object : {jobject(nullptr), value, jobject(java_owner)}) {
            jobject actual = kind == 0 ? env->CallStaticObjectMethod(java_owner, ids[kind], tag, real, object)
                : env->CallStaticObjectMethod(java_owner, ids[kind], java_owner, object, tag, real);
            jobject expected = tag == 0x12345678abcdef01LL && real == -123.25 ? object : nullptr;
            bool correct = !env->ExceptionCheck() && env->IsSameObject(actual, expected);
            if (actual) env->DeleteLocalRef(actual);
            if (!correct) return false;
            ++cases;
          }
        }
      }
    }
  }
  env->DeleteLocalRef(value);
  std::cerr << "ART JIT mixed arguments: interpreter/baseline-to-interpreter/baseline-to-JIT/optimized PASS cases=" << cases << "\n";
  return true;
}
}  // namespace aim_jni_acceptance_phase
