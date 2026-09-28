#pragma once
namespace aim_jni_acceptance_phase {
inline bool CheckJitArrayComposed(JNIEnv* env, art::Thread* self, art::jit::Jit* jit,
                                 art::Handle<art::mirror::Class> owner, jclass java_owner) {
  art::jit::ScopedJitSuspend workers;
  auto* method = owner->FindClassMethod("jitArrayTransform", "([III)I", art::kRuntimePointerSize);
  auto id = env->GetStaticMethodID(java_owner, "jitArrayTransform", "([III)I");
  jclass npe = env->FindClass("java/lang/NullPointerException");
  jclass bounds = env->FindClass("java/lang/ArrayIndexOutOfBoundsException");
  if (!method || !id || !npe || !bounds || env->ExceptionCheck()) return false;
  unsigned cases = 0;
  for (int phase = 0; phase < 3; ++phase) {
    if (phase && (!jit->CompileMethod(method, self, phase == 1 ? art::CompilationKind::kBaseline : art::CompilationKind::kOptimized, false) ||
        !jit->GetCodeCache()->ContainsPc(method->GetEntryPointFromQuickCompiledCode()))) {
      std::cerr << "ART JIT array composition compile failed phase=" << phase << "\n";
      return false;
    }
    for (int size : {-1, 0, 4}) for (jint count : {-1, 0, 1, 4, 5}) for (jint delta : {0, -17, INT32_MAX}) {
      jintArray array = size < 0 ? nullptr : env->NewIntArray(size);
      jint initial[] = {0, -1, INT32_MIN, INT32_MAX};
      if (size > 0) env->SetIntArrayRegion(array, 0, size, initial);
      if (env->ExceptionCheck() || (size >= 0 && !array)) return false;
      uint32_t expected[4]{};
      uint32_t sum = 0;
      for (int i = 0; i < 4; ++i) expected[i] = static_cast<uint32_t>(initial[i]);
      for (int i = 0; i < size && i < count; ++i) {
        expected[i] = expected[i] * 31u + static_cast<uint32_t>(delta);
        sum += expected[i];
      }
      jint result = env->CallStaticIntMethod(java_owner, id, array, count, delta);
      jthrowable exception = env->ExceptionOccurred();
      if (exception) env->ExceptionClear();
      bool correct = size < 0 ? exception && env->IsInstanceOf(exception, npe) :
          count > size ? exception && env->IsInstanceOf(exception, bounds) :
          !exception && static_cast<uint32_t>(result) == sum + static_cast<uint32_t>(size);
      if (size > 0) {
        jint actual[4]{};
        env->GetIntArrayRegion(array, 0, size, actual);
        for (int i = 0; i < size; ++i) correct &= static_cast<uint32_t>(actual[i]) == expected[i];
      }
      if (exception) env->DeleteLocalRef(exception);
      if (array) env->DeleteLocalRef(array);
      if (!correct || env->ExceptionCheck()) {
        std::cerr << "ART JIT array composition failed phase=" << phase << " size=" << size << " count=" << count << "\n";
        return false;
      }
      ++cases;
    }
  }
  env->DeleteLocalRef(npe);
  env->DeleteLocalRef(bounds);
  std::cerr << "ART JIT composed arrays: loop/read/write/length/null/bounds/partial stores PASS cases=" << cases << "\n";
  return true;
}
}  // namespace aim_jni_acceptance_phase
