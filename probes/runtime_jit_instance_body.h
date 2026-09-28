#pragma once
namespace aim_jni_acceptance_phase {
inline bool CheckJitInstanceBody(JNIEnv* env, art::Thread* self, art::jit::Jit* jit,
                                art::Handle<art::mirror::Class> owner, jclass java_owner) {
  art::jit::ScopedJitSuspend workers;
  jobject receiver = env->AllocObject(java_owner);
  if (!receiver || env->ExceptionCheck()) return false;
  const char* names[] = {"jitInstanceMath", "jitInstanceChoose", "jitInstanceWide"};
  const char* sigs[] = {"(II)I", "(Ljava/lang/Object;I)Ljava/lang/Object;", "(JDI)D"};
  art::ArtMethod* methods[3]{};
  jmethodID ids[3]{};
  for (int i = 0; i < 3; ++i) {
    methods[i] = owner->FindClassMethod(names[i], sigs[i], art::kRuntimePointerSize);
    ids[i] = env->GetMethodID(java_owner, names[i], sigs[i]);
    if (!methods[i] || !ids[i] || env->ExceptionCheck() || methods[i]->IsStatic() ||
        jit->GetCodeCache()->ContainsPc(methods[i]->GetEntryPointFromQuickCompiledCode())) return false;
  }
  unsigned cases = 0;
  const jint ints[] = {0, 1, -1, 17, INT32_MIN, INT32_MAX};
  for (int phase = 0; phase < 3; ++phase) {
    if (phase) for (auto* method : methods) {
      if (!jit->CompileMethod(method, self, phase == 1 ? art::CompilationKind::kBaseline : art::CompilationKind::kOptimized, false) ||
          !jit->GetCodeCache()->ContainsPc(method->GetEntryPointFromQuickCompiledCode())) {
        std::cerr << "ART JIT instance body compile failed: " << method->PrettyMethod() << "\n";
        return false;
      }
    }
    for (jint a : ints) for (jint b : ints) {
      uint32_t x = static_cast<uint32_t>(a), y = static_cast<uint32_t>(b);
      uint32_t expected = ((x * 31u + y) ^ (x >> (y & 31u))) ^ (a < b ? 17u : 93u);
      jint result = env->CallIntMethod(receiver, ids[0], a, b);
      if (env->ExceptionCheck() || static_cast<uint32_t>(result) != expected) return false;
      ++cases;
    }
    for (jobject value : {jobject(nullptr), receiver, jobject(java_owner)}) for (jint selector : ints) {
      jobject result = env->CallObjectMethod(receiver, ids[1], value, selector);
      bool correct = !env->ExceptionCheck() && env->IsSameObject(result, selector < 0 ? receiver : value);
      if (result) env->DeleteLocalRef(result);
      if (!correct) return false;
      ++cases;
    }
    for (jlong value : {jlong(0), jlong(-17), jlong(0x12345678abcdef01LL)})
      for (jdouble real : {jdouble(0.0), jdouble(-123.25), jdouble(19.5)}) for (jint scale : ints) {
        jdouble result = env->CallDoubleMethod(receiver, ids[2], value, real, scale);
        jdouble expected = static_cast<double>(value) + real * scale;
        if (env->ExceptionCheck() || result != expected) return false;
        ++cases;
      }
  }
  env->DeleteLocalRef(receiver);
  std::cerr << "ART JIT instance bodies: arithmetic/call/branch/this/mixed wide arguments PASS cases=" << cases << "\n";
  return true;
}
}  // namespace aim_jni_acceptance_phase
