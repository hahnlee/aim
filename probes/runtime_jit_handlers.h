#pragma once
namespace aim_jni_acceptance_phase {
inline bool CheckJitHandlers(JNIEnv* env, art::Thread* self, art::jit::Jit* jit,
                             art::Handle<art::mirror::Class> owner, jclass java_owner) {
  art::jit::ScopedJitSuspend workers;
  const char* names[] = {"jitTryArray", "jitTryConstructor"};
  const char* sigs[] = {"([II)I", "(ILjava/lang/Object;)Ldev/aim/probe/Hello;"};
  art::ArtMethod* methods[2]{};
  jmethodID ids[2]{};
  for (int i = 0; i < 2; ++i) {
    methods[i] = owner->FindClassMethod(names[i], sigs[i], art::kRuntimePointerSize);
    ids[i] = env->GetStaticMethodID(java_owner, names[i], sigs[i]);
    if (!methods[i] || !ids[i] || env->ExceptionCheck()) return false;
  }
  auto counter = env->GetStaticFieldID(java_owner, "jitFinallyCounter", "I");
  auto marker_field = env->GetFieldID(java_owner, "jitInstanceMarker", "I");
  auto payload_field = env->GetFieldID(java_owner, "jitInstancePayload", "Ljava/lang/Object;");
  if (!counter || !marker_field || !payload_field || env->ExceptionCheck()) return false;
  unsigned cases = 0;
  for (int phase = 0; phase < 3; ++phase) {
    if (phase) for (auto* method : methods) {
      if (!jit->CompileMethod(method, self, phase == 1 ? art::CompilationKind::kBaseline : art::CompilationKind::kOptimized, false) ||
          !jit->GetCodeCache()->ContainsPc(method->GetEntryPointFromQuickCompiledCode())) {
        std::cerr << "ART JIT handlers compile failed: " << method->PrettyMethod() << " phase=" << phase << "\n";
        return false;
      }
    }
    for (int size : {-1, 0, 3}) for (jint index : {-1, 0, 2, 3}) {
      jintArray array = size < 0 ? nullptr : env->NewIntArray(size);
      jint values[] = {19, INT32_MIN, INT32_MAX};
      if (size > 0) env->SetIntArrayRegion(array, 0, size, values);
      env->SetStaticIntField(java_owner, counter, 0);
      jint actual = env->CallStaticIntMethod(java_owner, ids[0], array, index);
      jint expected = size < 0 ? -31 : index < 0 || index >= size ? -73 : values[index];
      bool correct = !env->ExceptionCheck() && actual == expected && env->GetStaticIntField(java_owner, counter) == 1;
      if (array) env->DeleteLocalRef(array);
      if (!correct) return false;
      ++cases;
    }
    for (jint marker : {-1, 0, 42}) for (jobject payload : {jobject(nullptr), jobject(java_owner)}) {
      env->SetStaticIntField(java_owner, counter, 0);
      jobject result = env->CallStaticObjectMethod(java_owner, ids[1], marker, payload);
      bool correct = !env->ExceptionCheck() && env->GetStaticIntField(java_owner, counter) == 1;
      if (marker < 0) correct &= result == nullptr;
      else if (result) {
        correct &= env->GetIntField(result, marker_field) == marker;
        jobject value = env->GetObjectField(result, payload_field);
        correct &= env->IsSameObject(value, payload);
        if (value) env->DeleteLocalRef(value);
      } else correct = false;
      if (result) env->DeleteLocalRef(result);
      if (!correct || env->ExceptionCheck()) return false;
      ++cases;
    }
  }
  env->SetStaticIntField(java_owner, counter, 0);
  std::cerr << "ART JIT handlers: typed catches/constructor throw/normal return/finally exactly once PASS cases=" << cases << "\n";
  return !env->ExceptionCheck();
}
}  // namespace aim_jni_acceptance_phase
