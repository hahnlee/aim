#pragma once
namespace aim_jni_acceptance_phase {
inline bool CheckJitFieldComposed(JNIEnv* env, art::Thread* self, art::jit::Jit* jit,
                                 art::Handle<art::mirror::Class> owner, jclass java_owner) {
  art::jit::ScopedJitSuspend workers;
  jobject receiver = env->AllocObject(java_owner), target = env->AllocObject(java_owner);
  auto number = env->GetFieldID(java_owner, "jitComposedNumber", "I");
  auto object = env->GetFieldID(java_owner, "jitComposedObject", "Ljava/lang/Object;");
  auto wide = env->GetStaticFieldID(java_owner, "jitComposedWide", "J");
  jclass npe = env->FindClass("java/lang/NullPointerException");
  if (!receiver || !target || !number || !object || !wide || !npe || env->ExceptionCheck()) return false;
  const char* names[] = {"jitFieldUpdate", "jitWideFieldUpdate", "jitFieldExchange"};
  const char* sigs[] = {"(Ldev/aim/probe/Hello;I)I", "(J)J", "(Ljava/lang/Object;)Ljava/lang/Object;"};
  art::ArtMethod* methods[3]{};
  jmethodID ids[3]{};
  for (int i = 0; i < 3; ++i) {
    methods[i] = owner->FindClassMethod(names[i], sigs[i], art::kRuntimePointerSize);
    ids[i] = i == 1 ? env->GetStaticMethodID(java_owner, names[i], sigs[i]) : env->GetMethodID(java_owner, names[i], sigs[i]);
    if (!methods[i] || !ids[i] || env->ExceptionCheck()) return false;
  }
  unsigned cases = 0;
  for (int phase = 0; phase < 3; ++phase) {
    if (phase) for (auto* method : methods) {
      if (!jit->CompileMethod(method, self, phase == 1 ? art::CompilationKind::kBaseline : art::CompilationKind::kOptimized, false) ||
          !jit->GetCodeCache()->ContainsPc(method->GetEntryPointFromQuickCompiledCode())) {
        std::cerr << "ART JIT composed field compile failed: " << method->PrettyMethod() << "\n";
        return false;
      }
    }
    for (jint initial : {0, -17, INT32_MIN, INT32_MAX}) for (jint add : {0, 1, -1, INT32_MAX}) {
      env->SetIntField(target, number, initial);
      uint32_t expected = static_cast<uint32_t>(initial) * 31u + static_cast<uint32_t>(add);
      jint result = env->CallIntMethod(receiver, ids[0], target, add);
      if (env->ExceptionCheck() || static_cast<uint32_t>(result) != expected ||
          static_cast<uint32_t>(env->GetIntField(target, number)) != expected) return false;
      ++cases;
    }
    env->CallIntMethod(receiver, ids[0], jobject(nullptr), jint(1));
    jthrowable exception = env->ExceptionOccurred();
    if (exception) env->ExceptionClear();
    if (!exception || !env->IsInstanceOf(exception, npe)) return false;
    env->DeleteLocalRef(exception);
    ++cases;
    for (jlong initial : {jlong(0), jlong(-17), jlong(INT64_MAX)}) for (jlong add : {jlong(1), jlong(-1), jlong(0x12345678abcdef01LL)}) {
      env->SetStaticLongField(java_owner, wide, initial);
      uint64_t expected = static_cast<uint64_t>(initial) + static_cast<uint64_t>(add);
      jlong result = env->CallStaticLongMethod(java_owner, ids[1], add);
      if (env->ExceptionCheck() || static_cast<uint64_t>(result) != expected ||
          static_cast<uint64_t>(env->GetStaticLongField(java_owner, wide)) != expected) return false;
      ++cases;
    }
    for (jobject old : {jobject(nullptr), target, jobject(java_owner)}) for (jobject value : {jobject(nullptr), target, receiver}) {
      env->SetObjectField(receiver, object, old);
      auto* heap = art::Runtime::Current()->GetHeap();
      uint64_t before = heap->GetGcCount();
      jobject result = env->CallObjectMethod(receiver, ids[2], value);
      if (env->ExceptionCheck()) return false;
      jobject stored = env->GetObjectField(receiver, object);
      bool correct = !env->ExceptionCheck() && env->IsSameObject(result, old) &&
          env->IsSameObject(stored, value) && heap->GetGcCount() > before;
      if (result) env->DeleteLocalRef(result);
      if (stored) env->DeleteLocalRef(stored);
      if (!correct) return false;
      ++cases;
    }
  }
  env->SetObjectField(receiver, object, nullptr);
  env->DeleteLocalRef(receiver);
  env->DeleteLocalRef(target);
  env->DeleteLocalRef(npe);
  std::cerr << "ART JIT composed fields: read/math/write/null/volatile-wide/reference-GC PASS cases=" << cases << "\n";
  return !env->ExceptionCheck();
}
}  // namespace aim_jni_acceptance_phase
