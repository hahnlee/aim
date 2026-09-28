#pragma once
namespace aim_jni_acceptance_phase {
// Thread-local test controls keep unrelated JNI acceptance calls unchanged.
inline thread_local int native_callback_mode = 0;
inline thread_local bool native_callback_gc_observed = false;
inline jint NativeBaseToken(JNIEnv*, jobject) { return 193; }
inline jint NativeChildToken(JNIEnv*, jobject) { return 827; }
inline bool CheckJitNativePolymorphic(JNIEnv* env, art::Thread* self, art::jit::Jit* jit,
                                      art::Handle<art::mirror::Class> owner, jclass java_owner) {
  jclass classes[2]{};
  jobject receivers[3]{};
  const char* factories[] = {"jitVirtualBaseClass", "jitVirtualChildClass"};
  for (int i = 0; i < 2; ++i) {
    auto factory = env->GetStaticMethodID(java_owner, factories[i], "()Ljava/lang/Class;");
    if (!factory || env->ExceptionCheck()) return false;
    classes[i] = static_cast<jclass>(env->CallStaticObjectMethod(java_owner, factory));
    if (!classes[i] || env->ExceptionCheck()) return false;
    receivers[i + 1] = env->AllocObject(classes[i]);
    if (!receivers[i + 1] || env->ExceptionCheck()) return false;
  }
  const char* names[] = {"jitNativePolymorphic", "jitNativeInterface"};
  const char* sigs[] = {"(Ldev/aim/probe/JitVirtualBase;)I", "(Ldev/aim/probe/JitCallable;)I"};
  art::ArtMethod* methods[2]{};
  jmethodID ids[2]{};
  for (int i = 0; i < 2; ++i) {
    methods[i] = owner->FindClassMethod(names[i], sigs[i], art::kRuntimePointerSize);
    ids[i] = env->GetStaticMethodID(java_owner, names[i], sigs[i]);
    if (!methods[i] || !ids[i] || env->ExceptionCheck()) return false;
    art::CodeItemDataAccessor code(*methods[i]->GetDexFile(), methods[i]->GetCodeItem());
    if (art::Instruction::At(code.Insns())->Opcode() != (i == 0
        ? art::Instruction::INVOKE_VIRTUAL : art::Instruction::INVOKE_INTERFACE)) return false;
  }
  jclass npe = env->FindClass("java/lang/NullPointerException");
  if (!npe || env->ExceptionCheck()) return false;
  jclass unsatisfied = env->FindClass("java/lang/UnsatisfiedLinkError");
  if (!unsatisfied || env->ExceptionCheck()) return false;
  for (int phase = 0; phase < 3; ++phase) {
    if (phase) for (auto* method : methods) {
      if (!jit->CompileMethod(method, self, phase == 1 ? art::CompilationKind::kBaseline : art::CompilationKind::kOptimized, false) ||
          !jit->GetCodeCache()->ContainsPc(method->GetEntryPointFromQuickCompiledCode())) {
        std::cerr << "ART JIT polymorphic native compile failed: " << method->PrettyMethod() << "\n";
        return false;
      }
    }
    // Only these fixture classes are unregistered; unrelated native tables stay intact.
    for (auto klass : classes)
      if (env->UnregisterNatives(klass) != JNI_OK || env->ExceptionCheck()) return false;
    for (int call = 0; call < 2; ++call) for (int index = 1; index < 3; ++index) {
      env->CallStaticIntMethod(java_owner, ids[call], receivers[index]);
      jthrowable thrown = env->ExceptionOccurred();
      if (thrown) env->ExceptionClear();
      bool correct = thrown && env->IsInstanceOf(thrown, unsatisfied);
      if (thrown) env->DeleteLocalRef(thrown);
      if (!correct) {
        std::cerr << "ART JIT missing native binding failed phase=" << phase << " call=" << call
                  << " receiver=" << index << "\n";
        return false;
      }
    }
    for (int i = 0; i < 2; ++i) {
      JNINativeMethod native{const_cast<char*>("nativeToken"), const_cast<char*>("()I"),
          i == 0 ? reinterpret_cast<void*>(&NativeBaseToken) : reinterpret_cast<void*>(&NativeChildToken)};
      if (env->RegisterNatives(classes[i], &native, 1) != JNI_OK || env->ExceptionCheck()) return false;
    }
    // Reuse the same caller entrypoint after binding recovery; no recompilation.
    for (int call = 0; call < 2; ++call) for (int index = 0; index < 3; ++index) {
      jint result = env->CallStaticIntMethod(java_owner, ids[call], receivers[index]);
      jthrowable thrown = env->ExceptionOccurred();
      if (thrown) env->ExceptionClear();
      bool correct = index == 0 ? thrown && env->IsInstanceOf(thrown, npe)
          : !thrown && result == (index == 1 ? 193 : 827);
      if (thrown) env->DeleteLocalRef(thrown);
      if (!correct) {
        std::cerr << "ART JIT polymorphic native failed phase=" << phase << " call=" << call
                  << " receiver=" << index << " result=" << result << "\n";
        return false;
      }
    }
  }
  for (auto receiver : receivers) if (receiver) env->DeleteLocalRef(receiver);
  for (auto klass : classes) env->DeleteLocalRef(klass);
  env->DeleteLocalRef(npe);
  env->DeleteLocalRef(unsatisfied);
  std::cerr << "ART JIT native polymorphism: virtual/interface native overrides/null PASS cases=18"
            << "; unregister/UnsatisfiedLinkError/rebind recovery PASS failures=12\n";
  return true;
}
inline bool CheckJitNativeCalls(JNIEnv* env, art::Thread* self, art::jit::Jit* jit,
                                art::Handle<art::mirror::Class> owner, jclass java_owner) {
  art::jit::ScopedJitSuspend workers;
  const char* names[] = {"jitCallNative", "jitNativeReference", "nativeStackPcsRoundTrip", "jitNativeVirtual", "jitNativeDirect"};
  const char* sigs[] = {"()I", "(Ljava/lang/Object;)Ljava/lang/Object;", "()I",
      "(Ldev/aim/probe/Hello;)Ljava/lang/Object;", "(Ldev/aim/probe/Hello;)Ljava/lang/Object;"};
  art::ArtMethod* methods[5]{};
  jmethodID ids[5]{};
  for (int i = 0; i < 5; ++i) {
    methods[i] = owner->FindClassMethod(names[i], sigs[i], art::kRuntimePointerSize);
    ids[i] = env->GetStaticMethodID(java_owner, names[i], sigs[i]);
    if (!methods[i] || !ids[i] || env->ExceptionCheck()) return false;
    if (i >= 3) {
      art::CodeItemDataAccessor code(*methods[i]->GetDexFile(), methods[i]->GetCodeItem());
      auto expected = i == 3 ? art::Instruction::INVOKE_VIRTUAL : art::Instruction::INVOKE_DIRECT;
      if (art::Instruction::At(code.Insns())->Opcode() != expected) return false;
    }
  }
  jobject value = env->AllocObject(java_owner);
  if (!value || env->ExceptionCheck()) return false;
  jclass exception_class = env->FindClass("java/lang/IllegalArgumentException");
  jmethodID constructor = exception_class ? env->GetMethodID(exception_class, "<init>", "()V") : nullptr;
  jobject failure = constructor ? env->NewObject(exception_class, constructor) : nullptr;
  if (!failure || env->ExceptionCheck()) return false;
  jclass npe = env->FindClass("java/lang/NullPointerException");
  if (!npe || env->ExceptionCheck()) return false;
  unsigned cases = 0;
  for (int phase = 0; phase < 3; ++phase) {
    if (phase) for (auto* method : methods) {
      if (!jit->CompileMethod(method, self, phase == 1 ? art::CompilationKind::kBaseline : art::CompilationKind::kOptimized, false) ||
          !jit->GetCodeCache()->ContainsPc(method->GetEntryPointFromQuickCompiledCode())) {
        std::cerr << "ART JIT native caller compile failed: " << method->PrettyMethod() << "\n";
        return false;
      }
    }
    jint packed = env->CallStaticIntMethod(java_owner, ids[2]);
    if (packed != 42 || env->ExceptionCheck()) {
      std::cerr << "ART JIT packed JNI arguments failed: phase=" << phase << " result=" << packed << "\n";
      return false;
    }
    for (jobject input : {jobject(nullptr), value, jobject(java_owner)}) {
      if (env->CallStaticIntMethod(java_owner, ids[0]) != 16384 || env->ExceptionCheck()) return false;
      uint64_t before = art::Runtime::Current()->GetHeap()->GetGcCount();
      jobject result = env->CallStaticObjectMethod(java_owner, ids[1], input);
      bool correct = !env->ExceptionCheck() && env->IsSameObject(result, input) &&
          art::Runtime::Current()->GetHeap()->GetGcCount() > before;
      if (result) env->DeleteLocalRef(result);
      if (!correct) return false;
      ++cases;
    }
    for (int index : {3, 4}) for (jobject receiver : {jobject(nullptr), value}) {
      jobject result = env->CallStaticObjectMethod(java_owner, ids[index], receiver);
      jthrowable thrown = env->ExceptionOccurred();
      if (thrown) env->ExceptionClear();
      bool correct = receiver ? !thrown && env->IsSameObject(result, receiver)
                              : thrown && env->IsInstanceOf(thrown, npe);
      if (result) env->DeleteLocalRef(result);
      if (thrown) env->DeleteLocalRef(thrown);
      if (!correct) {
        std::cerr << "ART JIT instance JNI failed phase=" << phase << " index=" << index << "\n";
        return false;
      }
    }
    for (int mode : {1, 2}) {
      native_callback_mode = mode;
      native_callback_gc_observed = false;
      jobject result = env->CallStaticObjectMethod(java_owner, ids[1], mode == 1 ? value : failure);
      native_callback_mode = 0;
      jthrowable thrown = env->ExceptionOccurred();
      if (thrown) env->ExceptionClear();
      bool correct = native_callback_gc_observed && (mode == 1
          ? !thrown && env->IsSameObject(result, value)
          : thrown && env->IsSameObject(thrown, failure));
      if (result) env->DeleteLocalRef(result);
      if (thrown) env->DeleteLocalRef(thrown);
      if (!correct) {
        std::cerr << "ART JIT JNI callback failed phase=" << phase << " mode=" << mode << "\n";
        return false;
      }
    }
  }
  env->DeleteLocalRef(failure);
  env->DeleteLocalRef(npe);
  env->DeleteLocalRef(exception_class);
  env->DeleteLocalRef(value);
  std::cerr << "ART JIT native calls: primitive/reference JNI return + subsequent GC PASS cases=" << cases
            << "; packed integer/FP/reference/narrow JNI stack arguments PASS phases=3"
            << "; native-to-Java callback GC/reference/exception identity PASS cases=6"
            << "; instance JNI direct/virtual receiver identity/null PASS cases=12\n";
  return !env->ExceptionCheck() && CheckJitNativePolymorphic(env, self, jit, owner, java_owner);
}
}  // namespace aim_jni_acceptance_phase
