#pragma once
namespace aim_jni_acceptance_phase {
inline bool CheckJitObjectComposed(JNIEnv* env, art::Thread* self, art::jit::Jit* jit,
                                  art::Handle<art::mirror::Class> owner, jclass java_owner) {
  art::jit::ScopedJitSuspend workers;
  auto* method = owner->FindClassMethod("jitObjectCompose", "(ILjava/lang/Object;)Ldev/aim/probe/Hello;", art::kRuntimePointerSize);
  auto* constructor = owner->FindConstructor("(ILjava/lang/Object;)V", art::kRuntimePointerSize);
  auto id = env->GetStaticMethodID(java_owner, "jitObjectCompose", "(ILjava/lang/Object;)Ldev/aim/probe/Hello;");
  auto number = env->GetFieldID(java_owner, "jitComposedNumber", "I");
  auto link = env->GetFieldID(java_owner, "jitComposedObject", "Ljava/lang/Object;");
  auto marker_field = env->GetFieldID(java_owner, "jitInstanceMarker", "I");
  auto payload_field = env->GetFieldID(java_owner, "jitInstancePayload", "Ljava/lang/Object;");
  jclass illegal = env->FindClass("java/lang/IllegalArgumentException");
  jobject payload = env->AllocObject(java_owner);
  if (!method || !constructor || !id || !number || !link || !marker_field || !payload_field || !illegal || !payload || env->ExceptionCheck()) return false;
  unsigned cases = 0;
  for (int phase = 0; phase < 3; ++phase) {
    if (phase && (!jit->CompileMethod(constructor, self, phase == 1 ? art::CompilationKind::kBaseline : art::CompilationKind::kOptimized, false) ||
        !jit->GetCodeCache()->ContainsPc(constructor->GetEntryPointFromQuickCompiledCode()))) {
      std::cerr << "ART JIT general constructor compile failed phase=" << phase << "\n";
      return false;
    }
    if (phase && (!jit->CompileMethod(method, self, phase == 1 ? art::CompilationKind::kBaseline : art::CompilationKind::kOptimized, false) ||
        !jit->GetCodeCache()->ContainsPc(method->GetEntryPointFromQuickCompiledCode()))) {
      std::cerr << "ART JIT object composition compile failed phase=" << phase << "\n";
      return false;
    }
    for (jint marker : {-1, 0, 42, INT32_MAX}) for (jobject value : {jobject(nullptr), payload, jobject(java_owner)}) {
      auto* heap = art::Runtime::Current()->GetHeap();
      uint64_t before = heap->GetGcCount();
      jobject result = env->CallStaticObjectMethod(java_owner, id, marker, value);
      jthrowable exception = env->ExceptionOccurred();
      if (exception) env->ExceptionClear();
      bool correct = marker < 0 ? exception && env->IsInstanceOf(exception, illegal) : !exception && result;
      if (correct && marker < 0) {
        auto get_message = env->GetMethodID(illegal, "getMessage", "()Ljava/lang/String;");
        if (!get_message || env->ExceptionCheck()) return false;
        auto message = static_cast<jstring>(env->CallObjectMethod(exception, get_message));
        if (!message || env->ExceptionCheck()) return false;
        const char* text = env->GetStringUTFChars(message, nullptr);
        if (!text) return false;
        correct &= std::strcmp(text, "constructor marker") == 0;
        env->ReleaseStringUTFChars(message, text);
        env->DeleteLocalRef(message);
      }
      if (correct && marker >= 0) {
        correct &= heap->GetGcCount() > before;
        correct &= static_cast<uint32_t>(env->GetIntField(result, number)) == static_cast<uint32_t>(marker) * 31u;
        jobject first = env->GetObjectField(result, link);
        correct &= first && !env->IsSameObject(first, result);
        if (first) {
          correct &= env->GetIntField(first, marker_field) == marker;
          jobject actual = env->GetObjectField(first, payload_field);
          correct &= env->IsSameObject(actual, value);
          if (actual) env->DeleteLocalRef(actual);
          env->DeleteLocalRef(first);
        }
      }
      if (result) env->DeleteLocalRef(result);
      if (exception) env->DeleteLocalRef(exception);
      if (!correct || env->ExceptionCheck()) {
        std::cerr << "ART JIT object composition failed phase=" << phase << " marker=" << marker << "\n";
        return false;
      }
      ++cases;
    }
  }
  env->DeleteLocalRef(payload);
  env->DeleteLocalRef(illegal);
  std::cerr << "ART JIT composed objects: compiled general constructor/two allocations/fields/GC/throw recovery PASS cases=" << cases << "\n";
  return true;
}
}  // namespace aim_jni_acceptance_phase
