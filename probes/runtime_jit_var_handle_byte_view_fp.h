#pragma once
namespace aim_jni_acceptance_phase {
inline uint32_t JitVarHandleViewFloatBits(jfloat value) {
  uint32_t bits;
  std::memcpy(&bits, &value, sizeof(bits));
  return bits;
}
inline jfloat JitVarHandleViewFloatFromBits(uint32_t bits) {
  jfloat value;
  std::memcpy(&value, &bits, sizeof(value));
  return value;
}
inline uint64_t JitVarHandleViewDoubleBits(jdouble value) {
  uint64_t bits;
  std::memcpy(&bits, &value, sizeof(bits));
  return bits;
}
inline jdouble JitVarHandleViewDoubleFromBits(uint64_t bits) {
  jdouble value;
  std::memcpy(&value, &bits, sizeof(value));
  return value;
}
inline bool CheckJitVarHandleByteViewFp(JNIEnv* env, art::Thread* self, art::jit::Jit* jit,
    art::Handle<art::mirror::Class> owner, jclass java_owner, jclass npe) {
  auto factory = env->GetStaticMethodID(java_owner, "jitVarByteArrayViewHandle",
      "(Ljava/lang/Class;Z)Ljava/lang/invoke/VarHandle;");
  jclass float_array_class = env->FindClass("[F");
  jclass double_array_class = env->FindClass("[D");
  jobject handles[4] = {
      factory ? env->CallStaticObjectMethod(java_owner, factory, float_array_class, JNI_TRUE) : nullptr,
      factory ? env->CallStaticObjectMethod(java_owner, factory, float_array_class, JNI_FALSE) : nullptr,
      factory ? env->CallStaticObjectMethod(java_owner, factory, double_array_class, JNI_TRUE) : nullptr,
      factory ? env->CallStaticObjectMethod(java_owner, factory, double_array_class, JNI_FALSE) : nullptr};
  const char* names[] = {"jitVarByteViewFloatGet", "jitVarByteViewFloatSet",
                         "jitVarByteViewFloatCas", "jitVarByteViewFloatAdd",
                         "jitVarByteViewDoubleGet", "jitVarByteViewDoubleSet",
                         "jitVarByteViewDoubleCas", "jitVarByteViewDoubleAdd"};
  const char* sigs[] = {
      "(Ljava/lang/invoke/VarHandle;[BI)F", "(Ljava/lang/invoke/VarHandle;[BIF)V",
      "(Ljava/lang/invoke/VarHandle;[BIFF)Z", "(Ljava/lang/invoke/VarHandle;[BIF)F",
      "(Ljava/lang/invoke/VarHandle;[BI)D", "(Ljava/lang/invoke/VarHandle;[BID)V",
      "(Ljava/lang/invoke/VarHandle;[BIDD)Z", "(Ljava/lang/invoke/VarHandle;[BID)D"};
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
  auto encode = [](uint64_t bits, size_t size, bool big, jbyte* output) {
    for (size_t index = 0; index < size; ++index) {
      size_t shift_index = big ? size - 1u - index : index;
      output[index] = static_cast<jbyte>(bits >> (shift_index * 8u));
    }
  };
  auto check_bytes = [&](jint offset, uint64_t bits, size_t size, bool big) {
    jbyte actual[8]{};
    jbyte expected[8]{};
    encode(bits, size, big, expected);
    env->GetByteArrayRegion(bytes, offset, static_cast<jsize>(size), actual);
    return !env->ExceptionCheck() && std::memcmp(actual, expected, size) == 0;
  };
  auto write_bytes = [&](jint offset, uint64_t bits, size_t size, bool big) {
    jbyte encoded[8]{};
    encode(bits, size, big, encoded);
    env->SetByteArrayRegion(bytes, offset, static_cast<jsize>(size), encoded);
    return !env->ExceptionCheck();
  };
  auto force_gc = [&]() {
    art::ScopedThreadStateChange native(self, art::ThreadState::kNative);
    art::Runtime::Current()->GetHeap()->CollectGarbage(false);
  };
  auto set_arg = [](jvalue& arg, int group, uint64_t bits) {
    if (group == 0) arg.f = JitVarHandleViewFloatFromBits(static_cast<uint32_t>(bits));
    else arg.d = JitVarHandleViewDoubleFromBits(bits);
  };
  auto get_bits = [&](int group, jmethodID id, jvalue* args) {
    return group == 0
        ? static_cast<uint64_t>(JitVarHandleViewFloatBits(
              env->CallStaticFloatMethodA(java_owner, id, args)))
        : JitVarHandleViewDoubleBits(env->CallStaticDoubleMethodA(java_owner, id, args));
  };
  auto take_exception = [&](jclass expected) {
    jthrowable thrown = env->ExceptionOccurred();
    if (thrown) env->ExceptionClear();
    bool correct = thrown && env->IsInstanceOf(thrown, expected);
    if (thrown) env->DeleteLocalRef(thrown);
    return correct;
  };
  const uint64_t values[2][5] = {
      {UINT32_C(0x00000000), UINT32_C(0x80000000), UINT32_C(0x7f800000),
       UINT32_C(0xff7fffff), UINT32_C(0x7fc12345)},
      {UINT64_C(0x0000000000000000), UINT64_C(0x8000000000000000),
       UINT64_C(0x7ff0000000000000), UINT64_C(0xffefffffffffffff),
       UINT64_C(0x7ff8123456789abc)}};
  int operations[2]{};
  int unaligned_operations = 0;
  int contract_failures = 0;
  for (int phase = 0; phase < 3; ++phase) {
    if (phase) for (auto* method : methods) {
      if (!jit->CompileMethod(method, self,
          phase == 1 ? art::CompilationKind::kBaseline : art::CompilationKind::kOptimized, false) ||
          !jit->GetCodeCache()->ContainsPc(method->GetEntryPointFromQuickCompiledCode())) return false;
    }
    for (int group = 0; group < 2; ++group) for (int order = 0; order < 2; ++order) {
      int base = group * 4;
      size_t size = group == 0 ? 4u : 8u;
      jint index = group == 0 ? 0 : 4;
      uint64_t mask = group == 0 ? UINT32_MAX : UINT64_MAX;
      bool big = order == 1;
      for (uint64_t seed : values[group]) {
        uint64_t update = (seed ^ (group == 0 ? UINT64_C(0x80000000) :
                                             UINT64_C(0x8000000000000000))) & mask;
        jvalue args[5]{};
        args[0].l = handles[group * 2 + order]; args[1].l = bytes; args[2].i = index;
        set_arg(args[3], group, seed);
        env->CallStaticVoidMethodA(java_owner, ids[base + 1], args);
        if (!check_bytes(index, seed, size, big)) return false;
        force_gc();
        if (get_bits(group, ids[base], args) != seed || env->ExceptionCheck()) return false;
        ++operations[group];
        set_arg(args[3], group, (seed ^ 1u) & mask); set_arg(args[4], group, update);
        if (env->CallStaticBooleanMethodA(java_owner, ids[base + 2], args) != JNI_FALSE ||
            !check_bytes(index, seed, size, big)) return false;
        ++operations[group];
        set_arg(args[3], group, seed);
        if (env->CallStaticBooleanMethodA(java_owner, ids[base + 2], args) != JNI_TRUE ||
            !check_bytes(index, update, size, big)) return false;
        ++operations[group];
        if (!write_bytes(index, seed, size, big) || get_bits(group, ids[base], args) != seed ||
            env->ExceptionCheck()) return false;
        ++operations[group];
      }
      jvalue unsupported_args[5]{};
      unsupported_args[0].l = handles[group * 2 + order]; unsupported_args[1].l = bytes;
      get_bits(group, ids[base + 3], unsupported_args);
      if (!take_exception(unsupported)) return false;
      ++contract_failures;
      jvalue args[5]{};
      args[0].l = handles[group * 2 + order]; args[1].l = bytes; args[2].i = 1;
      set_arg(args[3], group, values[group][4]);
      env->CallStaticVoidMethodA(java_owner, ids[base + 1], args);
      if (env->ExceptionCheck() || get_bits(group, ids[base], args) != values[group][4] ||
          env->ExceptionCheck()) return false;
      ++unaligned_operations;
      set_arg(args[4], group, values[group][0]);
      env->CallStaticBooleanMethodA(java_owner, ids[base + 2], args);
      if (!take_exception(illegal_state)) return false;
      ++contract_failures;
    }
    for (int group = 0; group < 2; ++group) for (int operation = 0; operation < 3; ++operation)
        for (int order = 0; order < 2; ++order) {
      int kind = group * 4 + operation;
      auto invoke = [&](jvalue* args) {
        if (operation == 0) get_bits(group, ids[kind], args);
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
      jint bad = group == 0 ? 29 : 25;
      for (jint bad_index : {-1, bad}) {
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
  env->DeleteLocalRef(float_array_class);
  env->DeleteLocalRef(double_array_class);
  env->DeleteLocalRef(bounds);
  env->DeleteLocalRef(illegal_state);
  env->DeleteLocalRef(unsupported);
  std::cerr << "ART JIT byte-array FP view VarHandle: float-operations=" << operations[0]
            << " double-operations=" << operations[1]
            << " unaligned-plain=" << unaligned_operations
            << " contract-failures=" << contract_failures << " PASS\n";
  return !env->ExceptionCheck();
}
}  // namespace aim_jni_acceptance_phase
