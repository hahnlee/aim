#pragma once
namespace aim_jni_acceptance_phase {
inline bool CheckJitNestedFinally(JNIEnv* env, art::Thread* self, art::jit::Jit* jit,
                                  art::Handle<art::mirror::Class> owner, jclass java_owner) {
  art::jit::ScopedJitSuspend workers;
  const char* sig = "(Ljava/lang/Throwable;Ljava/lang/Throwable;I)Ljava/lang/Object;";
  auto* method = owner->FindClassMethod("jitNestedFinally", sig, art::kRuntimePointerSize);
  auto id = env->GetStaticMethodID(java_owner, "jitNestedFinally", sig);
  auto counter = env->GetStaticFieldID(java_owner, "jitFinallyCounter", "I");
  jclass throwable = env->FindClass("java/lang/IllegalArgumentException");
  jclass npe = env->FindClass("java/lang/NullPointerException");
  if (!method || !id || !counter || !throwable || !npe || env->ExceptionCheck()) return false;
  auto constructor = env->GetMethodID(throwable, "<init>", "()V");
  if (!constructor || env->ExceptionCheck()) return false;
  jobject first = env->NewObject(throwable, constructor);
  jobject second = env->NewObject(throwable, constructor);
  if (!first || !second || env->ExceptionCheck()) return false;
  unsigned cases = 0;
  for (int phase = 0; phase < 3; ++phase) {
    if (phase && (!jit->CompileMethod(method, self, phase == 1 ? art::CompilationKind::kBaseline : art::CompilationKind::kOptimized, false) ||
        !jit->GetCodeCache()->ContainsPc(method->GetEntryPointFromQuickCompiledCode()))) {
      std::cerr << "ART JIT nested finally compile failed phase=" << phase << "\n";
      return false;
    }
    for (jobject original : {jobject(nullptr), first, second})
      for (jobject replacement : {jobject(nullptr), first, second}) for (jint mask = 0; mask < 4; ++mask) {
        env->SetStaticIntField(java_owner, counter, 0);
        jobject result = env->CallStaticObjectMethod(java_owner, id, original, replacement, mask);
        jthrowable exception = env->ExceptionOccurred();
        if (exception) env->ExceptionClear();
        bool correct = env->GetStaticIntField(java_owner, counter) == 12;
        if (mask & 2) correct &= !exception && env->IsSameObject(result, replacement);
        else {
          jobject expected = mask & 1 ? replacement : original;
          correct &= exception && (expected ? env->IsSameObject(exception, expected) : env->IsInstanceOf(exception, npe));
        }
        if (result) env->DeleteLocalRef(result);
        if (exception) env->DeleteLocalRef(exception);
        if (!correct || env->ExceptionCheck()) {
          std::cerr << "ART JIT nested finally failed phase=" << phase << " mask=" << mask << "\n";
          return false;
        }
        ++cases;
      }
  }
  env->SetStaticIntField(java_owner, counter, 0);
  env->DeleteLocalRef(first);
  env->DeleteLocalRef(second);
  env->DeleteLocalRef(throwable);
  env->DeleteLocalRef(npe);
  std::cerr << "ART JIT nested finally: order/rethrow identity/replacement/return override/null throw PASS cases=" << cases << "\n";
  return !env->ExceptionCheck();
}
}  // namespace aim_jni_acceptance_phase
