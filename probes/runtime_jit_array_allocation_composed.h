#pragma once
namespace aim_jni_acceptance_phase {
inline bool CheckJitArrayAllocationComposed(JNIEnv* env, art::Thread* self, art::jit::Jit* jit,
                                           art::Handle<art::mirror::Class> owner, jclass java_owner) {
  art::jit::ScopedJitSuspend workers;
  const char* names[] = {"jitArrayAllocate", "jitArrayCopyGc"};
  const char* sigs[] = {"(II)[I", "([Ljava/lang/Object;)[Ljava/lang/Object;"};
  art::ArtMethod* methods[2]{};
  jmethodID ids[2]{};
  for (int i = 0; i < 2; ++i) {
    methods[i] = owner->FindClassMethod(names[i], sigs[i], art::kRuntimePointerSize);
    ids[i] = env->GetStaticMethodID(java_owner, names[i], sigs[i]);
    if (!methods[i] || !ids[i] || env->ExceptionCheck()) return false;
  }
  jclass negative = env->FindClass("java/lang/NegativeArraySizeException");
  jclass npe = env->FindClass("java/lang/NullPointerException");
  jclass object = env->FindClass("java/lang/Object");
  jobject value = env->AllocObject(java_owner);
  if (!negative || !npe || !object || !value || env->ExceptionCheck()) return false;
  unsigned cases = 0;
  for (int phase = 0; phase < 3; ++phase) {
    if (phase) for (int method_index = 0; method_index < 2; ++method_index) {
      auto* method = methods[method_index];
      if (!jit->CompileMethod(method, self, phase == 1 ? art::CompilationKind::kBaseline : art::CompilationKind::kOptimized, false) ||
          !jit->GetCodeCache()->ContainsPc(method->GetEntryPointFromQuickCompiledCode())) {
        std::cerr << "ART JIT composed array allocation compile failed: " << method->PrettyMethod() << "\n";
        return false;
      }
    }
    for (jint size : {-1, 0, 1, 7}) for (jint delta : {0, -17, INT32_MAX}) {
      auto result = static_cast<jintArray>(env->CallStaticObjectMethod(java_owner, ids[0], size, delta));
      jthrowable exception = env->ExceptionOccurred();
      if (exception) env->ExceptionClear();
      bool correct = size < 0 ? exception && env->IsInstanceOf(exception, negative) :
          !exception && result && env->GetArrayLength(result) == size;
      if (correct && size > 0) {
        jint values[7]{};
        env->GetIntArrayRegion(result, 0, size, values);
        for (int i = 0; i < size; ++i)
          correct &= static_cast<uint32_t>(values[i]) == uint32_t(i) * 31u + static_cast<uint32_t>(delta);
      }
      if (result) env->DeleteLocalRef(result);
      if (exception) env->DeleteLocalRef(exception);
      if (!correct || env->ExceptionCheck()) return false;
      ++cases;
    }
    for (int size : {-1, 0, 3}) {
      jobjectArray source = size < 0 ? nullptr : env->NewObjectArray(size, object, nullptr);
      if (size > 0) {
        env->SetObjectArrayElement(source, 1, value);
        env->SetObjectArrayElement(source, 2, java_owner);
      }
      if (env->ExceptionCheck() || (size >= 0 && !source)) return false;
      auto* heap = art::Runtime::Current()->GetHeap();
      uint64_t before = heap->GetGcCount();
      auto result = static_cast<jobjectArray>(env->CallStaticObjectMethod(java_owner, ids[1], source));
      jthrowable exception = env->ExceptionOccurred();
      if (exception) env->ExceptionClear();
      bool correct = size < 0 ? exception && env->IsInstanceOf(exception, npe) :
          !exception && result && env->GetArrayLength(result) == size && !env->IsSameObject(result, source);
      if (correct && size > 0) {
        correct &= heap->GetGcCount() > before;
        for (int i = 0; i < size; ++i) {
          jobject element = env->GetObjectArrayElement(result, i);
          correct &= env->IsSameObject(element, i == 0 ? nullptr : i == 1 ? value : jobject(java_owner));
          if (element) env->DeleteLocalRef(element);
        }
      }
      if (source) env->DeleteLocalRef(source);
      if (result) env->DeleteLocalRef(result);
      if (exception) env->DeleteLocalRef(exception);
      if (!correct || env->ExceptionCheck()) return false;
      ++cases;
    }
  }
  env->DeleteLocalRef(value);
  env->DeleteLocalRef(object);
  env->DeleteLocalRef(npe);
  env->DeleteLocalRef(negative);
  std::cerr << "ART JIT composed array allocation: initialize/negative-size/reference-copy-GC PASS cases=" << cases << "\n";
  return true;
}
}  // namespace aim_jni_acceptance_phase
