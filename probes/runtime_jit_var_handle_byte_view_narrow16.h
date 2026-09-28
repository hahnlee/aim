#pragma once
namespace aim_jni_acceptance_phase {
inline bool CheckJitVarHandleByteViewNarrow16(JNIEnv* env, art::Thread* self,
    art::jit::Jit* jit, art::Handle<art::mirror::Class> owner, jclass java_owner, jclass npe) {
  auto factory = env->GetStaticMethodID(java_owner, "jitVarByteArrayViewHandle",
      "(Ljava/lang/Class;Z)Ljava/lang/invoke/VarHandle;");
  jclass short_array_class = env->FindClass("[S");
  jclass char_array_class = env->FindClass("[C");
  jobject handles[4] = {
      factory ? env->CallStaticObjectMethod(java_owner, factory, short_array_class, JNI_TRUE) : nullptr,
      factory ? env->CallStaticObjectMethod(java_owner, factory, short_array_class, JNI_FALSE) : nullptr,
      factory ? env->CallStaticObjectMethod(java_owner, factory, char_array_class, JNI_TRUE) : nullptr,
      factory ? env->CallStaticObjectMethod(java_owner, factory, char_array_class, JNI_FALSE) : nullptr};
  const char* names[] = {"jitVarByteViewShortGet", "jitVarByteViewShortSet",
                         "jitVarByteViewShortCas", "jitVarByteViewShortAdd",
                         "jitVarByteViewCharGet", "jitVarByteViewCharSet",
                         "jitVarByteViewCharCas", "jitVarByteViewCharAdd"};
  const char* sigs[] = {
      "(Ljava/lang/invoke/VarHandle;[BI)S", "(Ljava/lang/invoke/VarHandle;[BIS)V",
      "(Ljava/lang/invoke/VarHandle;[BISS)Z", "(Ljava/lang/invoke/VarHandle;[BIS)S",
      "(Ljava/lang/invoke/VarHandle;[BI)C", "(Ljava/lang/invoke/VarHandle;[BIC)V",
      "(Ljava/lang/invoke/VarHandle;[BICC)Z", "(Ljava/lang/invoke/VarHandle;[BIC)C"};
  jmethodID ids[8]{};
  art::ArtMethod* methods[8]{};
  for (int kind = 0; kind < 8; ++kind) {
    ids[kind] = env->GetStaticMethodID(java_owner, names[kind], sigs[kind]);
    methods[kind] = owner->FindClassMethod(names[kind], sigs[kind], art::kRuntimePointerSize);
    if (!ids[kind] || !methods[kind] || env->ExceptionCheck()) return false;
    art::CodeItemDataAccessor code(*methods[kind]->GetDexFile(), methods[kind]->GetCodeItem());
    bool polymorphic = false;
    for (const auto& pair : code) {
      polymorphic |= pair.Inst().Opcode() == art::Instruction::INVOKE_POLYMORPHIC ||
          pair.Inst().Opcode() == art::Instruction::INVOKE_POLYMORPHIC_RANGE;
    }
    if (!polymorphic) return false;
  }
  jbyteArray bytes = env->NewByteArray(32);
  jclass bounds = env->FindClass("java/lang/IndexOutOfBoundsException");
  jclass illegal_state = env->FindClass("java/lang/IllegalStateException");
  jclass unsupported = env->FindClass("java/lang/UnsupportedOperationException");
  if (!bytes || !bounds || !illegal_state || !unsupported || env->ExceptionCheck()) return false;
  auto encode = [](uint16_t bits, bool big, jbyte* output) {
    output[0] = static_cast<jbyte>(bits >> (big ? 8 : 0));
    output[1] = static_cast<jbyte>(bits >> (big ? 0 : 8));
  };
  auto check_bytes = [&](jint offset, uint16_t bits, bool big) {
    jbyte actual[2]{};
    jbyte expected[2]{};
    encode(bits, big, expected);
    env->GetByteArrayRegion(bytes, offset, 2, actual);
    return !env->ExceptionCheck() && std::memcmp(actual, expected, 2) == 0;
  };
  auto write_bytes = [&](uint16_t bits, bool big) {
    jbyte encoded[2]{};
    encode(bits, big, encoded);
    env->SetByteArrayRegion(bytes, 0, 2, encoded);
    return !env->ExceptionCheck();
  };
  auto force_gc = [&]() {
    art::ScopedThreadStateChange native(self, art::ThreadState::kNative);
    art::Runtime::Current()->GetHeap()->CollectGarbage(false);
  };
  auto set_arg = [](jvalue& arg, int group, uint16_t bits) {
    if (group == 0) arg.s = static_cast<jshort>(bits);
    else arg.c = static_cast<jchar>(bits);
  };
  auto get_value = [&](int group, jmethodID id, jvalue* args) {
    return group == 0 ? static_cast<uint16_t>(env->CallStaticShortMethodA(java_owner, id, args))
                      : static_cast<uint16_t>(env->CallStaticCharMethodA(java_owner, id, args));
  };
  auto take_exception = [&](jclass expected) {
    jthrowable thrown = env->ExceptionOccurred();
    if (thrown) env->ExceptionClear();
    bool correct = thrown && env->IsInstanceOf(thrown, expected);
    if (thrown) env->DeleteLocalRef(thrown);
    return correct;
  };
  int operations[2]{};
  int unaligned_operations = 0;
  int contract_failures = 0;
  const uint16_t values[2][5] = {
      {0, UINT16_MAX, UINT16_C(0x8000), UINT16_C(0x7fff), UINT16_C(0x1234)},
      {0, UINT16_C(0x7fff), UINT16_C(0x8000), UINT16_MAX, UINT16_C(0x1234)}};
  for (int phase = 0; phase < 3; ++phase) {
    if (phase) for (auto* method : methods) {
      if (!jit->CompileMethod(method, self,
          phase == 1 ? art::CompilationKind::kBaseline : art::CompilationKind::kOptimized, false) ||
          !jit->GetCodeCache()->ContainsPc(method->GetEntryPointFromQuickCompiledCode())) return false;
    }
    for (int group = 0; group < 2; ++group) for (int order = 0; order < 2; ++order) {
      int base = group * 4;
      bool big = order == 1;
      for (uint16_t seed : values[group]) {
        jvalue args[5]{};
        args[0].l = handles[group * 2 + order]; args[1].l = bytes; args[2].i = 0;
        set_arg(args[3], group, seed);
        env->CallStaticVoidMethodA(java_owner, ids[base + 1], args);
        if (!check_bytes(0, seed, big)) return false;
        force_gc();
        if (get_value(group, ids[base], args) != seed || env->ExceptionCheck()) return false;
        ++operations[group];
        if (!write_bytes(seed, big) || get_value(group, ids[base], args) != seed ||
            env->ExceptionCheck()) return false;
        ++operations[group];
      }
      for (int operation = 2; operation < 4; ++operation) {
        jvalue unsupported_args[5]{};
        unsupported_args[0].l = handles[group * 2 + order];
        unsupported_args[1].l = bytes;
        set_arg(unsupported_args[3], group, 1);
        set_arg(unsupported_args[4], group, 2);
        if (operation == 2) {
          env->CallStaticBooleanMethodA(java_owner, ids[base + operation], unsupported_args);
        } else {
          get_value(group, ids[base + operation], unsupported_args);
        }
        if (!take_exception(unsupported)) return false;
        ++contract_failures;
      }
      jvalue args[5]{};
      args[0].l = handles[group * 2 + order]; args[1].l = bytes; args[2].i = 1;
      set_arg(args[3], group, UINT16_C(0x1234));
      env->CallStaticVoidMethodA(java_owner, ids[base + 1], args);
      if (env->ExceptionCheck() || get_value(group, ids[base], args) != UINT16_C(0x1234) ||
          env->ExceptionCheck()) return false;
      ++unaligned_operations;
    }
    for (int group = 0; group < 2; ++group) for (int operation = 0; operation < 2; ++operation)
        for (int order = 0; order < 2; ++order) {
      int kind = group * 4 + operation;
      auto invoke = [&](jvalue* args) {
        if (operation == 0 || operation == 3) get_value(group, ids[kind], args);
        else if (operation == 1) env->CallStaticVoidMethodA(java_owner, ids[kind], args);
        else env->CallStaticBooleanMethodA(java_owner, ids[kind], args);
      };
      for (int failure = 0; failure < 2; ++failure) {
        jvalue args[5]{};
        args[0].l = failure == 0 ? nullptr : handles[group * 2 + order];
        args[1].l = failure == 1 ? nullptr : bytes;
        invoke(args);
        if (!take_exception(npe)) return false;
        ++contract_failures;
      }
      for (jint bad_index : {-1, 31}) {
        jvalue args[5]{};
        args[0].l = handles[group * 2 + order]; args[1].l = bytes; args[2].i = bad_index;
        invoke(args);
        if (!take_exception(bounds)) return false;
        ++contract_failures;
      }
    }
  }
  for (jobject handle : handles) env->DeleteLocalRef(handle);
  env->DeleteLocalRef(bytes);
  env->DeleteLocalRef(short_array_class);
  env->DeleteLocalRef(char_array_class);
  env->DeleteLocalRef(bounds);
  env->DeleteLocalRef(illegal_state);
  env->DeleteLocalRef(unsupported);
  std::cerr << "ART JIT byte-array narrow16 view VarHandle: short-operations=" << operations[0]
            << " char-operations=" << operations[1]
            << " unaligned-plain=" << unaligned_operations
            << " contract-failures=" << contract_failures << " PASS\n";
  return !env->ExceptionCheck();
}
}  // namespace aim_jni_acceptance_phase
