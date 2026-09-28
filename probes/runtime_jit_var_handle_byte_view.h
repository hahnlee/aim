#pragma once
namespace aim_jni_acceptance_phase {
inline uint64_t JitVarHandleByteViewLongBits(jlong value) {
  uint64_t bits;
  std::memcpy(&bits, &value, sizeof(bits));
  return bits;
}
inline jlong JitVarHandleByteViewLongFromBits(uint64_t bits) {
  jlong value;
  std::memcpy(&value, &bits, sizeof(value));
  return value;
}
inline bool CheckJitVarHandleByteView(JNIEnv* env, art::Thread* self, art::jit::Jit* jit,
    art::Handle<art::mirror::Class> owner, jclass java_owner, jclass npe) {
  auto factory = env->GetStaticMethodID(java_owner, "jitVarByteArrayViewHandle",
      "(Ljava/lang/Class;Z)Ljava/lang/invoke/VarHandle;");
  jclass int_array_class = env->FindClass("[I");
  jclass long_array_class = env->FindClass("[J");
  jobject handles[4] = {
      factory ? env->CallStaticObjectMethod(java_owner, factory, int_array_class, JNI_TRUE) : nullptr,
      factory ? env->CallStaticObjectMethod(java_owner, factory, int_array_class, JNI_FALSE) : nullptr,
      factory ? env->CallStaticObjectMethod(java_owner, factory, long_array_class, JNI_TRUE) : nullptr,
      factory ? env->CallStaticObjectMethod(java_owner, factory, long_array_class, JNI_FALSE) : nullptr};
  const char* names[] = {"jitVarByteViewIntGet", "jitVarByteViewIntSet",
                         "jitVarByteViewIntCas", "jitVarByteViewIntAdd",
                         "jitVarByteViewLongGet", "jitVarByteViewLongSet",
                         "jitVarByteViewLongCas", "jitVarByteViewLongAdd"};
  const char* sigs[] = {
      "(Ljava/lang/invoke/VarHandle;[BI)I", "(Ljava/lang/invoke/VarHandle;[BII)V",
      "(Ljava/lang/invoke/VarHandle;[BIII)Z", "(Ljava/lang/invoke/VarHandle;[BII)I",
      "(Ljava/lang/invoke/VarHandle;[BI)J", "(Ljava/lang/invoke/VarHandle;[BIJ)V",
      "(Ljava/lang/invoke/VarHandle;[BIJJ)Z", "(Ljava/lang/invoke/VarHandle;[BIJ)J"};
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
  jbyteArray int_bytes = env->NewByteArray(32);
  jbyteArray long_bytes = env->NewByteArray(32);
  jclass bounds = env->FindClass("java/lang/IndexOutOfBoundsException");
  jclass illegal_state = env->FindClass("java/lang/IllegalStateException");
  if (!int_bytes || !long_bytes || !bounds || !illegal_state || env->ExceptionCheck()) return false;
  auto encode = [](uint64_t bits, size_t size, bool big, jbyte* output) {
    for (size_t index = 0; index < size; ++index) {
      size_t shift_index = big ? size - 1u - index : index;
      output[index] = static_cast<jbyte>(bits >> (shift_index * 8u));
    }
  };
  auto check_bytes = [&](jbyteArray array, jint offset, uint64_t bits, size_t size, bool big) {
    jbyte actual[8]{};
    jbyte expected[8]{};
    encode(bits, size, big, expected);
    env->GetByteArrayRegion(array, offset, static_cast<jsize>(size), actual);
    return !env->ExceptionCheck() && std::memcmp(actual, expected, size) == 0;
  };
  auto force_gc = [&]() {
    art::ScopedThreadStateChange native(self, art::ThreadState::kNative);
    art::Runtime::Current()->GetHeap()->CollectGarbage(false);
  };
  int int_operations = 0;
  int long_operations = 0;
  int unaligned_operations = 0;
  int contract_failures = 0;
  for (int phase = 0; phase < 3; ++phase) {
    if (phase) for (auto* method : methods) {
      if (!jit->CompileMethod(method, self,
          phase == 1 ? art::CompilationKind::kBaseline : art::CompilationKind::kOptimized, false) ||
          !jit->GetCodeCache()->ContainsPc(method->GetEntryPointFromQuickCompiledCode())) {
        std::cerr << "ART JIT byte-view VarHandle compile failed phase=" << phase << "\n";
        return false;
      }
    }
    const jint int_values[] = {jint(0), jint(-1), INT32_MIN, INT32_MAX, jint(0x12345678)};
    for (int order = 0; order < 2; ++order) for (jint seed : int_values) {
      bool big = order == 1;
      jobject handle = handles[order];
      jint update = seed ^ jint(0xa55a5aa5);
      jvalue args[5]{};
      args[0].l = handle; args[1].l = int_bytes; args[2].i = 0; args[3].i = seed;
      env->CallStaticVoidMethodA(java_owner, ids[1], args);
      if (!check_bytes(int_bytes, 0, static_cast<uint32_t>(seed), 4, big)) return false;
      force_gc();
      if (env->CallStaticIntMethodA(java_owner, ids[0], args) != seed || env->ExceptionCheck()) return false;
      ++int_operations;
      args[3].i = seed ^ 1; args[4].i = update;
      if (env->CallStaticBooleanMethodA(java_owner, ids[2], args) != JNI_FALSE ||
          !check_bytes(int_bytes, 0, static_cast<uint32_t>(seed), 4, big)) return false;
      ++int_operations;
      args[3].i = seed;
      if (env->CallStaticBooleanMethodA(java_owner, ids[2], args) != JNI_TRUE ||
          !check_bytes(int_bytes, 0, static_cast<uint32_t>(update), 4, big)) return false;
      ++int_operations;
      constexpr uint32_t kDelta = UINT32_C(0x1234567);
      args[3].i = static_cast<jint>(kDelta);
      uint32_t sum = static_cast<uint32_t>(update) + kDelta;
      if (env->CallStaticIntMethodA(java_owner, ids[3], args) != update ||
          !check_bytes(int_bytes, 0, sum, 4, big)) return false;
      ++int_operations;
      jbyte encoded[8]{};
      encode(static_cast<uint32_t>(seed), 4, big, encoded);
      env->SetByteArrayRegion(int_bytes, 0, 4, encoded);
      if (env->CallStaticIntMethodA(java_owner, ids[0], args) != seed || env->ExceptionCheck()) return false;
      ++int_operations;
    }
    const jlong long_values[] = {jlong(0), jlong(-1), INT64_MIN, INT64_MAX,
                                 JitVarHandleByteViewLongFromBits(UINT64_C(0x0123456789abcdef))};
    for (int order = 0; order < 2; ++order) for (jlong seed : long_values) {
      bool big = order == 1;
      jobject handle = handles[2 + order];
      uint64_t seed_bits = JitVarHandleByteViewLongBits(seed);
      jlong update = JitVarHandleByteViewLongFromBits(seed_bits ^ UINT64_C(0xa55a5aa53cc3c33c));
      jvalue args[5]{};
      args[0].l = handle; args[1].l = long_bytes; args[2].i = 4; args[3].j = seed;
      env->CallStaticVoidMethodA(java_owner, ids[5], args);
      if (!check_bytes(long_bytes, 4, seed_bits, 8, big)) return false;
      force_gc();
      if (JitVarHandleByteViewLongBits(env->CallStaticLongMethodA(java_owner, ids[4], args)) != seed_bits ||
          env->ExceptionCheck()) return false;
      ++long_operations;
      args[3].j = JitVarHandleByteViewLongFromBits(seed_bits ^ 1u); args[4].j = update;
      if (env->CallStaticBooleanMethodA(java_owner, ids[6], args) != JNI_FALSE ||
          !check_bytes(long_bytes, 4, seed_bits, 8, big)) return false;
      ++long_operations;
      args[3].j = seed;
      if (env->CallStaticBooleanMethodA(java_owner, ids[6], args) != JNI_TRUE ||
          !check_bytes(long_bytes, 4, JitVarHandleByteViewLongBits(update), 8, big)) return false;
      ++long_operations;
      constexpr uint64_t kDelta = UINT64_C(0x123456789abcdef);
      args[3].j = JitVarHandleByteViewLongFromBits(kDelta);
      uint64_t sum = JitVarHandleByteViewLongBits(update) + kDelta;
      if (JitVarHandleByteViewLongBits(env->CallStaticLongMethodA(java_owner, ids[7], args)) !=
              JitVarHandleByteViewLongBits(update) || !check_bytes(long_bytes, 4, sum, 8, big)) return false;
      ++long_operations;
      jbyte encoded[8]{};
      encode(seed_bits, 8, big, encoded);
      env->SetByteArrayRegion(long_bytes, 4, 8, encoded);
      if (JitVarHandleByteViewLongBits(env->CallStaticLongMethodA(java_owner, ids[4], args)) != seed_bits ||
          env->ExceptionCheck()) return false;
      ++long_operations;
    }
    for (int group = 0; group < 2; ++group) for (int order = 0; order < 2; ++order) {
      int base = group * 4;
      jobject handle = handles[group * 2 + order];
      jbyteArray array = group == 0 ? int_bytes : long_bytes;
      jvalue args[5]{};
      args[0].l = handle; args[1].l = array; args[2].i = 1;
      if (group == 0) args[3].i = jint(0x12345678);
      else args[3].j = JitVarHandleByteViewLongFromBits(UINT64_C(0x0123456789abcdef));
      env->CallStaticVoidMethodA(java_owner, ids[base + 1], args);
      if (env->ExceptionCheck()) return false;
      if (group == 0) {
        if (env->CallStaticIntMethodA(java_owner, ids[base], args) != jint(0x12345678)) return false;
      } else if (JitVarHandleByteViewLongBits(env->CallStaticLongMethodA(java_owner, ids[base], args)) !=
                     UINT64_C(0x0123456789abcdef)) return false;
      if (env->ExceptionCheck()) return false;
      ++unaligned_operations;
      if (group == 0) args[4].i = jint(7); else args[4].j = jlong(7);
      env->CallStaticBooleanMethodA(java_owner, ids[base + 2], args);
      jthrowable thrown = env->ExceptionOccurred();
      if (thrown) env->ExceptionClear();
      bool correct = thrown && env->IsInstanceOf(thrown, illegal_state);
      if (thrown) env->DeleteLocalRef(thrown);
      if (!correct) return false;
      ++contract_failures;
    }
    for (int kind = 0; kind < 8; ++kind) for (int order = 0; order < 2; ++order) {
      int group = kind / 4;
      int operation = kind % 4;
      jobject handle = handles[group * 2 + order];
      jbyteArray array = group == 0 ? int_bytes : long_bytes;
      auto invoke = [&](jvalue* args) {
        if (operation == 0 || operation == 3) {
          if (group == 0) env->CallStaticIntMethodA(java_owner, ids[kind], args);
          else env->CallStaticLongMethodA(java_owner, ids[kind], args);
        } else if (operation == 1) env->CallStaticVoidMethodA(java_owner, ids[kind], args);
        else env->CallStaticBooleanMethodA(java_owner, ids[kind], args);
      };
      for (int failure = 0; failure < 2; ++failure) {
        jvalue args[5]{};
        args[0].l = failure == 0 ? nullptr : handle;
        args[1].l = failure == 1 ? nullptr : array;
        invoke(args);
        jthrowable thrown = env->ExceptionOccurred();
        if (thrown) env->ExceptionClear();
        bool correct = thrown && env->IsInstanceOf(thrown, npe);
        if (thrown) env->DeleteLocalRef(thrown);
        if (!correct) return false;
        ++contract_failures;
      }
      jint bad_indexes[] = {-1, group == 0 ? 29 : 25};
      for (jint bad_index : bad_indexes) {
        jvalue args[5]{};
        args[0].l = handle; args[1].l = array; args[2].i = bad_index;
        invoke(args);
        jthrowable thrown = env->ExceptionOccurred();
        if (thrown) env->ExceptionClear();
        bool correct = thrown && env->IsInstanceOf(thrown, bounds);
        if (thrown) env->DeleteLocalRef(thrown);
        if (!correct) return false;
        ++contract_failures;
      }
    }
  }
  for (jobject handle : handles) env->DeleteLocalRef(handle);
  env->DeleteLocalRef(int_bytes);
  env->DeleteLocalRef(long_bytes);
  env->DeleteLocalRef(int_array_class);
  env->DeleteLocalRef(long_array_class);
  env->DeleteLocalRef(bounds);
  env->DeleteLocalRef(illegal_state);
  std::cerr << "ART JIT byte-array view VarHandle: int-operations=" << int_operations
            << " long-operations=" << long_operations
            << " unaligned-plain=" << unaligned_operations
            << " contract-failures=" << contract_failures << " PASS\n";
  return !env->ExceptionCheck();
}
}  // namespace aim_jni_acceptance_phase
