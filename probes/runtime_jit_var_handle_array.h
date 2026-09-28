#pragma once
namespace aim_jni_acceptance_phase {
inline bool CheckJitVarHandleArray(JNIEnv* env, art::Thread* self, art::jit::Jit* jit,
    art::Handle<art::mirror::Class> owner, jclass java_owner, jobject receiver, jclass npe) {
  auto factory = env->GetStaticMethodID(
      java_owner, "jitVarArrayHandle", "(Ljava/lang/Class;)Ljava/lang/invoke/VarHandle;");
  jclass int_array_class = env->FindClass("[I");
  jclass object_array_class = env->FindClass("[Ljava/lang/Object;");
  jclass object_class = env->FindClass("java/lang/Object");
  jobject handles[2] = {
      factory ? env->CallStaticObjectMethod(java_owner, factory, int_array_class) : nullptr,
      factory ? env->CallStaticObjectMethod(java_owner, factory, object_array_class) : nullptr};
  const char* names[] = {"jitVarIntArrayGet", "jitVarIntArraySet", "jitVarIntArrayCas",
                         "jitVarIntArrayAdd", "jitVarObjectArrayGet", "jitVarObjectArraySet",
                         "jitVarObjectArrayCas", "jitVarObjectArraySwap"};
  const char* sigs[] = {
      "(Ljava/lang/invoke/VarHandle;[II)I", "(Ljava/lang/invoke/VarHandle;[III)V",
      "(Ljava/lang/invoke/VarHandle;[IIII)Z", "(Ljava/lang/invoke/VarHandle;[III)I",
      "(Ljava/lang/invoke/VarHandle;[Ljava/lang/Object;I)Ljava/lang/Object;",
      "(Ljava/lang/invoke/VarHandle;[Ljava/lang/Object;ILjava/lang/Object;)V",
      "(Ljava/lang/invoke/VarHandle;[Ljava/lang/Object;ILjava/lang/Object;Ljava/lang/Object;)Z",
      "(Ljava/lang/invoke/VarHandle;[Ljava/lang/Object;ILjava/lang/Object;)Ljava/lang/Object;"};
  jmethodID ids[8]{};
  art::ArtMethod* methods[8]{};
  for (int kind = 0; kind < 8; ++kind) {
    ids[kind] = env->GetStaticMethodID(java_owner, names[kind], sigs[kind]);
    methods[kind] = owner->FindClassMethod(names[kind], sigs[kind], art::kRuntimePointerSize);
    if (!handles[kind / 4] || !ids[kind] || !methods[kind] || env->ExceptionCheck()) return false;
    art::CodeItemDataAccessor code(*methods[kind]->GetDexFile(), methods[kind]->GetCodeItem());
    bool polymorphic = false;
    for (const auto& pair : code) {
      polymorphic |= pair.Inst().Opcode() == art::Instruction::INVOKE_POLYMORPHIC ||
          pair.Inst().Opcode() == art::Instruction::INVOKE_POLYMORPHIC_RANGE;
    }
    if (!polymorphic) return false;
  }
  jintArray ints = env->NewIntArray(5);
  jobjectArray objects = env->NewObjectArray(5, object_class, nullptr);
  jclass string_class = env->FindClass("java/lang/String");
  jobjectArray strings = env->NewObjectArray(2, string_class, nullptr);
  jstring string_value = env->NewStringUTF("darwin-array-varhandle");
  jclass bounds = env->FindClass("java/lang/ArrayIndexOutOfBoundsException");
  jclass array_store = env->FindClass("java/lang/ArrayStoreException");
  if (!ints || !objects || !strings || !string_value || !bounds || !array_store ||
      env->ExceptionCheck()) return false;
  auto force_gc = [&]() {
    art::ScopedThreadStateChange native(self, art::ThreadState::kNative);
    art::Runtime::Current()->GetHeap()->CollectGarbage(false);
  };
  int int_operations = 0;
  int reference_operations = 0;
  int covariant_operations = 0;
  int contract_failures = 0;
  for (int phase = 0; phase < 3; ++phase) {
    if (phase) for (auto* method : methods) {
      if (!jit->CompileMethod(method, self,
          phase == 1 ? art::CompilationKind::kBaseline : art::CompilationKind::kOptimized, false) ||
          !jit->GetCodeCache()->ContainsPc(method->GetEntryPointFromQuickCompiledCode())) {
        std::cerr << "ART JIT array VarHandle compile failed phase=" << phase << "\n";
        return false;
      }
    }
    for (jint index = 0; index < 5; ++index) {
      jint seed = index == 0 ? INT32_MIN : index == 4 ? INT32_MAX : jint(0x12345678 * index);
      jint update = seed ^ jint(0x5a5aa5a5);
      jvalue args[5]{};
      args[0].l = handles[0];
      args[1].l = ints;
      args[2].i = index;
      args[3].i = seed;
      env->CallStaticVoidMethodA(java_owner, ids[1], args);
      jint direct = 0;
      env->GetIntArrayRegion(ints, index, 1, &direct);
      force_gc();
      jint actual = env->CallStaticIntMethodA(java_owner, ids[0], args);
      if (env->ExceptionCheck() || direct != seed || actual != seed) return false;
      ++int_operations;
      args[3].i = seed ^ 1;
      args[4].i = update;
      if (env->CallStaticBooleanMethodA(java_owner, ids[2], args) != JNI_FALSE) return false;
      env->GetIntArrayRegion(ints, index, 1, &direct);
      if (env->ExceptionCheck() || direct != seed) return false;
      ++int_operations;
      args[3].i = seed;
      if (env->CallStaticBooleanMethodA(java_owner, ids[2], args) != JNI_TRUE) return false;
      env->GetIntArrayRegion(ints, index, 1, &direct);
      if (env->ExceptionCheck() || direct != update) return false;
      ++int_operations;
      args[3].i = jint(0x3456789);
      uint32_t sum = static_cast<uint32_t>(update) + UINT32_C(0x3456789);
      if (env->CallStaticIntMethodA(java_owner, ids[3], args) != update) return false;
      env->GetIntArrayRegion(ints, index, 1, &direct);
      if (env->ExceptionCheck() || static_cast<uint32_t>(direct) != sum) return false;
      ++int_operations;
      direct = seed ^ jint(0x13579bdf);
      env->SetIntArrayRegion(ints, index, 1, &direct);
      if (env->CallStaticIntMethodA(java_owner, ids[0], args) != direct || env->ExceptionCheck()) return false;
      ++int_operations;
    }
    const jobject inputs[] = {nullptr, receiver, java_owner};
    for (jint index = 0; index < 3; ++index) for (jobject input : inputs) {
      jobject replacement = env->IsSameObject(input, java_owner) ? receiver : java_owner;
      jvalue args[6]{};
      args[0].l = handles[1];
      args[1].l = objects;
      args[2].i = index;
      args[3].l = input;
      env->CallStaticVoidMethodA(java_owner, ids[5], args);
      force_gc();
      jobject direct = env->GetObjectArrayElement(objects, index);
      jobject actual = env->CallStaticObjectMethodA(java_owner, ids[4], args);
      bool correct = !env->ExceptionCheck() && env->IsSameObject(direct, input) &&
          env->IsSameObject(actual, input);
      if (direct) env->DeleteLocalRef(direct);
      if (actual) env->DeleteLocalRef(actual);
      if (!correct) return false;
      ++reference_operations;
      args[3].l = replacement;
      args[4].l = input;
      if (env->CallStaticBooleanMethodA(java_owner, ids[6], args) != JNI_FALSE) return false;
      direct = env->GetObjectArrayElement(objects, index);
      correct = !env->ExceptionCheck() && env->IsSameObject(direct, input);
      if (direct) env->DeleteLocalRef(direct);
      if (!correct) return false;
      ++reference_operations;
      args[3].l = input;
      args[4].l = replacement;
      if (env->CallStaticBooleanMethodA(java_owner, ids[6], args) != JNI_TRUE) return false;
      direct = env->GetObjectArrayElement(objects, index);
      correct = !env->ExceptionCheck() && env->IsSameObject(direct, replacement);
      if (direct) env->DeleteLocalRef(direct);
      if (!correct) return false;
      ++reference_operations;
      args[3].l = input;
      actual = env->CallStaticObjectMethodA(java_owner, ids[7], args);
      correct = !env->ExceptionCheck() && env->IsSameObject(actual, replacement);
      if (actual) env->DeleteLocalRef(actual);
      direct = env->GetObjectArrayElement(objects, index);
      correct &= env->IsSameObject(direct, input);
      if (direct) env->DeleteLocalRef(direct);
      if (!correct) return false;
      ++reference_operations;
    }
    {
      jvalue args[6]{};
      args[0].l = handles[1];
      args[1].l = strings;
      args[2].i = 0;
      args[3].l = string_value;
      env->CallStaticVoidMethodA(java_owner, ids[5], args);
      jobject actual = env->CallStaticObjectMethodA(java_owner, ids[4], args);
      bool correct = !env->ExceptionCheck() && env->IsSameObject(actual, string_value);
      if (actual) env->DeleteLocalRef(actual);
      if (!correct) return false;
      ++covariant_operations;
      args[3].l = string_value;
      args[4].l = nullptr;
      if (env->CallStaticBooleanMethodA(java_owner, ids[6], args) != JNI_TRUE ||
          env->ExceptionCheck()) return false;
      ++covariant_operations;
      args[3].l = nullptr;
      args[4].l = string_value;
      if (env->CallStaticBooleanMethodA(java_owner, ids[6], args) != JNI_TRUE ||
          env->ExceptionCheck()) return false;
      ++covariant_operations;
      args[3].l = receiver;
      env->CallStaticVoidMethodA(java_owner, ids[5], args);
      jthrowable thrown = env->ExceptionOccurred();
      if (thrown) env->ExceptionClear();
      correct = thrown && env->IsInstanceOf(thrown, array_store);
      if (thrown) env->DeleteLocalRef(thrown);
      actual = env->GetObjectArrayElement(strings, 0);
      correct &= !env->ExceptionCheck() && env->IsSameObject(actual, string_value);
      if (actual) env->DeleteLocalRef(actual);
      if (!correct) return false;
      ++contract_failures;
    }
    for (int kind = 0; kind < 8; ++kind) {
      for (int failure = 0; failure < 2; ++failure) {
        jvalue args[6]{};
        args[0].l = failure == 0 ? nullptr : handles[kind / 4];
        args[1].l = failure == 1 ? nullptr : (kind < 4 ? jobject(ints) : jobject(objects));
        args[2].i = 0;
        int operation = kind % 4;
        if (operation == 0 || operation == 3) {
          if (kind < 4) env->CallStaticIntMethodA(java_owner, ids[kind], args);
          else env->CallStaticObjectMethodA(java_owner, ids[kind], args);
        } else if (operation == 1) {
          env->CallStaticVoidMethodA(java_owner, ids[kind], args);
        } else {
          env->CallStaticBooleanMethodA(java_owner, ids[kind], args);
        }
        jthrowable thrown = env->ExceptionOccurred();
        if (thrown) env->ExceptionClear();
        bool correct = thrown && env->IsInstanceOf(thrown, npe);
        if (thrown) env->DeleteLocalRef(thrown);
        if (!correct) return false;
        ++contract_failures;
      }
      for (jint bad_index : {jint(-1), jint(5)}) {
        jvalue args[6]{};
        args[0].l = handles[kind / 4];
        args[1].l = kind < 4 ? jobject(ints) : jobject(objects);
        args[2].i = bad_index;
        int operation = kind % 4;
        if (operation == 0 || operation == 3) {
          if (kind < 4) env->CallStaticIntMethodA(java_owner, ids[kind], args);
          else env->CallStaticObjectMethodA(java_owner, ids[kind], args);
        } else if (operation == 1) {
          env->CallStaticVoidMethodA(java_owner, ids[kind], args);
        } else {
          env->CallStaticBooleanMethodA(java_owner, ids[kind], args);
        }
        jthrowable thrown = env->ExceptionOccurred();
        if (thrown) env->ExceptionClear();
        bool correct = thrown && env->IsInstanceOf(thrown, bounds);
        if (thrown) env->DeleteLocalRef(thrown);
        if (!correct) return false;
        ++contract_failures;
      }
    }
  }
  env->DeleteLocalRef(handles[0]);
  env->DeleteLocalRef(handles[1]);
  env->DeleteLocalRef(ints);
  env->DeleteLocalRef(objects);
  env->DeleteLocalRef(strings);
  env->DeleteLocalRef(string_value);
  env->DeleteLocalRef(int_array_class);
  env->DeleteLocalRef(object_array_class);
  env->DeleteLocalRef(object_class);
  env->DeleteLocalRef(string_class);
  env->DeleteLocalRef(bounds);
  env->DeleteLocalRef(array_store);
  std::cerr << "ART JIT array VarHandle: int-operations=" << int_operations
            << " reference-operations=" << reference_operations
            << " covariant-slow-path=" << covariant_operations
            << " null/bounds-failures=" << contract_failures << " PASS\n";
  return !env->ExceptionCheck();
}
}  // namespace aim_jni_acceptance_phase
