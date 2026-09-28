#pragma once
namespace aim_jni_acceptance_phase {
inline bool CheckJitComposedCalls(JNIEnv* env, art::Thread* self, art::jit::Jit* jit,
                                  art::Handle<art::mirror::Class> owner, jclass java_owner) {
  art::jit::ScopedJitSuspend workers;
  const char* names[] = {"jitComposedGc", "jitVoidComposed", "jitGcTarget", "jitVoidGc", "jitVoidTarget"};
  const char* sigs[] = {"(Ljava/lang/Object;Ljava/lang/Object;)Ljava/lang/Object;",
      "(Ljava/lang/Object;Ljava/lang/Object;)V", "(Ljava/lang/Object;)Ljava/lang/Object;", "()V", "(Ljava/lang/Object;)V"};
  art::ArtMethod* methods[5]{};
  jmethodID ids[5]{};
  for (int i = 0; i < 5; ++i) {
    methods[i] = owner->FindClassMethod(names[i], sigs[i], art::kRuntimePointerSize);
    ids[i] = env->GetStaticMethodID(java_owner, names[i], sigs[i]);
    if (!methods[i] || !ids[i] || env->ExceptionCheck()) return false;
  }
  jfieldID stored = env->GetStaticFieldID(java_owner, "jitVoidValue", "Ljava/lang/Object;");
  jobject value = env->AllocObject(java_owner);
  if (!stored || !value || env->ExceptionCheck()) return false;
  auto* heap = art::Runtime::Current()->GetHeap();
  unsigned cases = 0;
  for (int phase = 0; phase < 3; ++phase) {
    if (phase != 0) {
      for (int method_index = 0; method_index < 5; ++method_index) {
        auto* method = methods[method_index];
        if (!jit->CompileMethod(method, self, phase == 1 ? art::CompilationKind::kBaseline : art::CompilationKind::kOptimized, false) ||
            !jit->GetCodeCache()->ContainsPc(method->GetEntryPointFromQuickCompiledCode())) {
          return false;
        }
      }
    }
    for (jobject a : {jobject(nullptr), value, jobject(java_owner)}) {
      for (jobject b : {jobject(nullptr), value, jobject(java_owner)}) {
        uint64_t before = heap->GetGcCount();
        jobject actual = env->CallStaticObjectMethod(java_owner, ids[0], a, b);
        bool correct = !env->ExceptionCheck() && env->IsSameObject(actual, a ? a : b);
        if (actual) env->DeleteLocalRef(actual);
        if (!correct || heap->GetGcCount() < before + 3) return false;
        env->CallStaticVoidMethod(java_owner, ids[1], a, b);
        if (env->ExceptionCheck()) return false;
        actual = env->GetStaticObjectField(java_owner, stored);
        correct = !env->ExceptionCheck() && env->IsSameObject(actual, b);
        if (actual) env->DeleteLocalRef(actual);
        if (!correct) return false;
        ++cases;
      }
    }
  }
  env->SetStaticObjectField(java_owner, stored, nullptr);
  env->DeleteLocalRef(value);
  std::cerr << "ART JIT composed calls: reference results across 3 GCs, void call/store order PASS cases=" << cases << "\n";
  return !env->ExceptionCheck();
}
}  // namespace aim_jni_acceptance_phase
