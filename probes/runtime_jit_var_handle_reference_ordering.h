#pragma once
namespace aim_jni_acceptance_phase {
inline bool CheckJitVarHandleReferenceOrderingShape(JNIEnv* env, art::Thread* self,
    art::jit::Jit* jit, art::Handle<art::mirror::Class> owner, jclass java_owner,
    jobject receiver, jclass npe, int shape) {
  const char* suffixes[] = {
      "GetOpaque", "GetAcquire", "GetVolatile", "SetOpaque", "SetRelease", "SetVolatile",
      "WeakPlain", "WeakAcquire", "WeakRelease", "WeakVolatile", "ExchangeAcquire",
      "ExchangeRelease", "SwapAcquire", "SwapRelease"};
  constexpr const char* kInstanceSigs[] = {
      "(Ljava/lang/invoke/VarHandle;Ldev/aim/probe/Hello;)Ljava/lang/Object;",
      "(Ljava/lang/invoke/VarHandle;Ldev/aim/probe/Hello;Ljava/lang/Object;)V",
      "(Ljava/lang/invoke/VarHandle;Ldev/aim/probe/Hello;Ljava/lang/Object;Ljava/lang/Object;)Z",
      "(Ljava/lang/invoke/VarHandle;Ldev/aim/probe/Hello;Ljava/lang/Object;Ljava/lang/Object;)Ljava/lang/Object;",
      "(Ljava/lang/invoke/VarHandle;Ldev/aim/probe/Hello;Ljava/lang/Object;)Ljava/lang/Object;"};
  constexpr const char* kStaticSigs[] = {
      "(Ljava/lang/invoke/VarHandle;)Ljava/lang/Object;",
      "(Ljava/lang/invoke/VarHandle;Ljava/lang/Object;)V",
      "(Ljava/lang/invoke/VarHandle;Ljava/lang/Object;Ljava/lang/Object;)Z",
      "(Ljava/lang/invoke/VarHandle;Ljava/lang/Object;Ljava/lang/Object;)Ljava/lang/Object;",
      "(Ljava/lang/invoke/VarHandle;Ljava/lang/Object;)Ljava/lang/Object;"};
  constexpr const char* kArraySigs[] = {
      "(Ljava/lang/invoke/VarHandle;[Ljava/lang/Object;I)Ljava/lang/Object;",
      "(Ljava/lang/invoke/VarHandle;[Ljava/lang/Object;ILjava/lang/Object;)V",
      "(Ljava/lang/invoke/VarHandle;[Ljava/lang/Object;ILjava/lang/Object;Ljava/lang/Object;)Z",
      "(Ljava/lang/invoke/VarHandle;[Ljava/lang/Object;ILjava/lang/Object;Ljava/lang/Object;)Ljava/lang/Object;",
      "(Ljava/lang/invoke/VarHandle;[Ljava/lang/Object;ILjava/lang/Object;)Ljava/lang/Object;"};
  auto signature_kind = [](int kind) {
    return kind < 3 ? 0 : kind < 6 ? 1 : kind < 10 ? 2 : kind < 12 ? 3 : 4;
  };
  jmethodID ids[14]{};
  art::ArtMethod* methods[14]{};
  for (int kind = 0; kind < 14; ++kind) {
    std::string name = shape == 0 ? "jitVarReference" :
        shape == 1 ? "jitVarStaticReference" : "jitVarObjectArray";
    name += suffixes[kind];
    const char* sig = shape == 0 ? kInstanceSigs[signature_kind(kind)]
        : shape == 1 ? kStaticSigs[signature_kind(kind)] : kArraySigs[signature_kind(kind)];
    ids[kind] = env->GetStaticMethodID(java_owner, name.c_str(), sig);
    methods[kind] = owner->FindClassMethod(name.c_str(), sig, art::kRuntimePointerSize);
    if (!ids[kind] || !methods[kind] || env->ExceptionCheck()) return false;
    art::CodeItemDataAccessor code(*methods[kind]->GetDexFile(), methods[kind]->GetCodeItem());
    bool polymorphic = false;
    for (const auto& pair : code) {
      polymorphic |= pair.Inst().Opcode() == art::Instruction::INVOKE_POLYMORPHIC ||
          pair.Inst().Opcode() == art::Instruction::INVOKE_POLYMORPHIC_RANGE;
    }
    if (!polymorphic) return false;
  }
  jobject handle = nullptr;
  jobjectArray array = nullptr;
  jobjectArray covariant_array = nullptr;
  jfieldID field = nullptr;
  jclass bounds = shape == 2 ? env->FindClass("java/lang/ArrayIndexOutOfBoundsException") : nullptr;
  jclass array_store = shape == 2 ? env->FindClass("java/lang/ArrayStoreException") : nullptr;
  if (shape == 0) {
    auto factory = env->GetStaticMethodID(
        java_owner, "jitVarReferenceHandle", "()Ljava/lang/invoke/VarHandle;");
    field = env->GetFieldID(java_owner, "jitVarReference", "Ljava/lang/Object;");
    if (factory && field) handle = env->CallStaticObjectMethod(java_owner, factory);
  } else if (shape == 1) {
    auto factory = env->GetStaticMethodID(
        java_owner, "jitVarStaticReferenceHandle", "()Ljava/lang/invoke/VarHandle;");
    field = env->GetStaticFieldID(java_owner, "jitVarStaticReference", "Ljava/lang/Object;");
    if (factory && field) handle = env->CallStaticObjectMethod(java_owner, factory);
  } else {
    auto factory = env->GetStaticMethodID(
        java_owner, "jitVarArrayHandle", "(Ljava/lang/Class;)Ljava/lang/invoke/VarHandle;");
    jclass array_class = env->FindClass("[Ljava/lang/Object;");
    jclass object_class = env->FindClass("java/lang/Object");
    jclass string_class = env->FindClass("java/lang/String");
    array = object_class ? env->NewObjectArray(5, object_class, nullptr) : nullptr;
    covariant_array = string_class ? env->NewObjectArray(1, string_class, nullptr) : nullptr;
    if (factory && array_class && array) {
      handle = env->CallStaticObjectMethod(java_owner, factory, array_class);
    }
    if (array_class) env->DeleteLocalRef(array_class);
    if (object_class) env->DeleteLocalRef(object_class);
    if (string_class) env->DeleteLocalRef(string_class);
  }
  if (!handle || (shape == 2 && (!array || !covariant_array || !bounds || !array_store)) ||
      env->ExceptionCheck()) return false;
  constexpr jint kIndex = 2;
  auto force_gc = [&]() {
    art::ScopedThreadStateChange native(self, art::ThreadState::kNative);
    art::Runtime::Current()->GetHeap()->CollectGarbage(false);
  };
  auto set_state = [&](jobject value) {
    if (shape == 0) env->SetObjectField(receiver, field, value);
    else if (shape == 1) env->SetStaticObjectField(java_owner, field, value);
    else env->SetObjectArrayElement(array, kIndex, value);
  };
  auto state_is = [&](jobject expected) {
    jobject value = shape == 0 ? env->GetObjectField(receiver, field)
        : shape == 1 ? env->GetStaticObjectField(java_owner, field)
                     : env->GetObjectArrayElement(array, kIndex);
    bool correct = !env->ExceptionCheck() && env->IsSameObject(value, expected);
    if (value) env->DeleteLocalRef(value);
    return correct;
  };
  auto args_for = [&](jobject value, jobject expected) {
    std::array<jvalue, 5> args{};
    args[0].l = handle;
    int coordinate = 1;
    if (shape == 0) args[coordinate++].l = receiver;
    else if (shape == 2) { args[coordinate++].l = array; args[coordinate++].i = kIndex; }
    args[coordinate++].l = expected;
    args[coordinate].l = value;
    return args;
  };
  int operation_groups = 0;
  int exception_groups = 0;
  for (int phase = 0; phase < 3; ++phase) {
    if (phase) for (auto* method : methods) {
      if (!jit->CompileMethod(method, self,
          phase == 1 ? art::CompilationKind::kBaseline : art::CompilationKind::kOptimized, false) ||
          !jit->GetCodeCache()->ContainsPc(method->GetEntryPointFromQuickCompiledCode())) return false;
    }
    for (jobject input : {jobject(nullptr), receiver, jobject(java_owner)}) {
      jobject replacement = env->IsSameObject(input, java_owner) ? receiver : java_owner;
      for (int kind = 0; kind < 3; ++kind) {
        set_state(input); force_gc();
        auto args = args_for(nullptr, nullptr);
        jobject result = env->CallStaticObjectMethodA(java_owner, ids[kind], args.data());
        bool correct = env->IsSameObject(result, input) && state_is(input);
        if (result) env->DeleteLocalRef(result);
        if (!correct) return false;
        ++operation_groups;
      }
      for (int kind = 3; kind < 6; ++kind) {
        set_state(input);
        auto args = args_for(nullptr, replacement);
        env->CallStaticVoidMethodA(java_owner, ids[kind], args.data()); force_gc();
        if (!state_is(replacement)) return false;
        ++operation_groups;
      }
      for (int kind = 6; kind < 10; ++kind) {
        set_state(input);
        auto args = args_for(input, replacement);
        if (env->CallStaticBooleanMethodA(java_owner, ids[kind], args.data()) != JNI_FALSE ||
            !state_is(input)) return false;
        ++operation_groups;
        args = args_for(replacement, input);
        bool success = false;
        for (int retry = 0; retry < 100 && !success; ++retry) {
          success = env->CallStaticBooleanMethodA(java_owner, ids[kind], args.data()) == JNI_TRUE;
          if (env->ExceptionCheck()) return false;
        }
        force_gc();
        if (!success || !state_is(replacement)) return false;
        ++operation_groups;
      }
      for (int kind = 10; kind < 12; ++kind) {
        set_state(input);
        auto args = args_for(input, replacement);
        jobject result = env->CallStaticObjectMethodA(java_owner, ids[kind], args.data());
        bool correct = env->IsSameObject(result, input) && state_is(input);
        if (result) env->DeleteLocalRef(result);
        if (!correct) return false;
        ++operation_groups;
        args = args_for(replacement, input);
        result = env->CallStaticObjectMethodA(java_owner, ids[kind], args.data()); force_gc();
        correct = env->IsSameObject(result, input) && state_is(replacement);
        if (result) env->DeleteLocalRef(result);
        if (!correct) return false;
        ++operation_groups;
      }
      for (int kind = 12; kind < 14; ++kind) {
        set_state(input);
        auto args = args_for(nullptr, replacement);
        jobject result = env->CallStaticObjectMethodA(java_owner, ids[kind], args.data()); force_gc();
        bool correct = env->IsSameObject(result, input) && state_is(replacement);
        if (result) env->DeleteLocalRef(result);
        if (!correct) return false;
        ++operation_groups;
      }
    }
    for (int kind = 0; kind < 14; ++kind) {
      int cases = shape == 0 ? 2 : shape == 1 ? 1 : 4;
      for (int bad = 0; bad < cases; ++bad) {
        auto args = args_for(nullptr, nullptr);
        args[0].l = bad == 0 ? nullptr : handle;
        if (shape == 0 && bad == 1) args[1].l = nullptr;
        if (shape == 2) {
          args[1].l = bad == 1 ? nullptr : array;
          args[2].i = bad == 2 ? -1 : bad == 3 ? 5 : kIndex;
        }
        int sig_kind = signature_kind(kind);
        if (sig_kind == 1) env->CallStaticVoidMethodA(java_owner, ids[kind], args.data());
        else if (sig_kind == 2) env->CallStaticBooleanMethodA(java_owner, ids[kind], args.data());
        else env->CallStaticObjectMethodA(java_owner, ids[kind], args.data());
        jthrowable thrown = env->ExceptionOccurred();
        if (thrown) env->ExceptionClear();
        bool correct = thrown && env->IsInstanceOf(thrown, bad < 2 ? npe : bounds);
        if (thrown) env->DeleteLocalRef(thrown);
        if (!correct) return false;
        ++exception_groups;
      }
    }
    if (shape == 2) for (int kind = 3; kind < 14; ++kind) {
      jvalue args[5]{};
      args[0].l = handle;
      args[1].l = covariant_array;
      args[2].i = 0;
      if (kind < 6 || kind >= 12) args[3].l = receiver;
      else { args[3].l = nullptr; args[4].l = receiver; }
      int sig_kind = signature_kind(kind);
      if (sig_kind == 1) env->CallStaticVoidMethodA(java_owner, ids[kind], args);
      else if (sig_kind == 2) env->CallStaticBooleanMethodA(java_owner, ids[kind], args);
      else env->CallStaticObjectMethodA(java_owner, ids[kind], args);
      jthrowable thrown = env->ExceptionOccurred();
      if (thrown) env->ExceptionClear();
      jobject retained = env->GetObjectArrayElement(covariant_array, 0);
      bool correct = thrown && env->IsInstanceOf(thrown, array_store) && retained == nullptr;
      if (retained) env->DeleteLocalRef(retained);
      if (thrown) env->DeleteLocalRef(thrown);
      if (!correct || env->ExceptionCheck()) return false;
      ++exception_groups;
    }
  }
  const char* label = shape == 0 ? "reference instance" :
      shape == 1 ? "reference static" : "reference array";
  std::cerr << "ART JIT VarHandle " << label << " ordering: operation-groups="
            << operation_groups << " exception-groups=" << exception_groups << " PASS\n";
  set_state(nullptr);
  env->DeleteLocalRef(handle);
  if (array) env->DeleteLocalRef(array);
  if (covariant_array) env->DeleteLocalRef(covariant_array);
  if (bounds) env->DeleteLocalRef(bounds);
  if (array_store) env->DeleteLocalRef(array_store);
  return !env->ExceptionCheck();
}
inline bool CheckJitVarHandleReferenceOrdering(JNIEnv* env, art::Thread* self,
    art::jit::Jit* jit, art::Handle<art::mirror::Class> owner, jclass java_owner,
    jobject receiver, jclass npe) {
  for (int shape = 0; shape < 3; ++shape) {
    if (!CheckJitVarHandleReferenceOrderingShape(
        env, self, jit, owner, java_owner, receiver, npe, shape)) return false;
  }
  return true;
}
}  // namespace aim_jni_acceptance_phase
