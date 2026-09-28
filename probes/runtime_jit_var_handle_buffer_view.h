#pragma once
namespace aim_jni_acceptance_phase {
inline bool CheckJitVarHandleByteBufferView(JNIEnv* env, art::Thread* self, art::jit::Jit* jit,
    art::Handle<art::mirror::Class> owner, jclass java_owner, jclass npe) {
  auto factory = env->GetStaticMethodID(java_owner, "jitVarByteBufferViewHandle",
      "(Ljava/lang/Class;Z)Ljava/lang/invoke/VarHandle;");
  jclass int_array_class = env->FindClass("[I");
  jclass long_array_class = env->FindClass("[J");
  jobject handles[4] = {
      factory ? env->CallStaticObjectMethod(java_owner, factory, int_array_class, JNI_TRUE) : nullptr,
      factory ? env->CallStaticObjectMethod(java_owner, factory, int_array_class, JNI_FALSE) : nullptr,
      factory ? env->CallStaticObjectMethod(java_owner, factory, long_array_class, JNI_TRUE) : nullptr,
      factory ? env->CallStaticObjectMethod(java_owner, factory, long_array_class, JNI_FALSE) : nullptr};
  const char* names[] = {"jitVarBufferViewIntGet", "jitVarBufferViewIntSet",
                         "jitVarBufferViewIntCas", "jitVarBufferViewIntAdd",
                         "jitVarBufferViewLongGet", "jitVarBufferViewLongSet",
                         "jitVarBufferViewLongCas", "jitVarBufferViewLongAdd"};
  const char* sigs[] = {
      "(Ljava/lang/invoke/VarHandle;Ljava/nio/ByteBuffer;I)I",
      "(Ljava/lang/invoke/VarHandle;Ljava/nio/ByteBuffer;II)V",
      "(Ljava/lang/invoke/VarHandle;Ljava/nio/ByteBuffer;III)Z",
      "(Ljava/lang/invoke/VarHandle;Ljava/nio/ByteBuffer;II)I",
      "(Ljava/lang/invoke/VarHandle;Ljava/nio/ByteBuffer;I)J",
      "(Ljava/lang/invoke/VarHandle;Ljava/nio/ByteBuffer;IJ)V",
      "(Ljava/lang/invoke/VarHandle;Ljava/nio/ByteBuffer;IJJ)Z",
      "(Ljava/lang/invoke/VarHandle;Ljava/nio/ByteBuffer;IJ)J"};
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

  jclass byte_buffer_class = env->FindClass("java/nio/ByteBuffer");
  auto allocate = env->GetStaticMethodID(
      byte_buffer_class, "allocate", "(I)Ljava/nio/ByteBuffer;");
  auto allocate_direct = env->GetStaticMethodID(
      byte_buffer_class, "allocateDirect", "(I)Ljava/nio/ByteBuffer;");
  auto get_byte = env->GetMethodID(byte_buffer_class, "get", "(I)B");
  auto put_byte = env->GetMethodID(byte_buffer_class, "put", "(IB)Ljava/nio/ByteBuffer;");
  auto as_read_only = env->GetMethodID(
      byte_buffer_class, "asReadOnlyBuffer", "()Ljava/nio/ByteBuffer;");
  jobject buffers[2] = {
      allocate ? env->CallStaticObjectMethod(byte_buffer_class, allocate, 32) : nullptr,
      allocate_direct ? env->CallStaticObjectMethod(byte_buffer_class, allocate_direct, 32) : nullptr};
  jclass bounds = env->FindClass("java/lang/IndexOutOfBoundsException");
  jclass illegal_state = env->FindClass("java/lang/IllegalStateException");
  jclass read_only = env->FindClass("java/nio/ReadOnlyBufferException");
  if (!byte_buffer_class || !get_byte || !put_byte || !as_read_only || !buffers[0] || !buffers[1] ||
      !bounds || !illegal_state || !read_only || env->ExceptionCheck()) return false;
  if (env->GetDirectBufferAddress(buffers[0]) != nullptr ||
      env->GetDirectBufferAddress(buffers[1]) == nullptr ||
      env->GetDirectBufferCapacity(buffers[1]) != 32) return false;

  auto encode = [](uint64_t bits, size_t size, bool big, jbyte* output) {
    for (size_t index = 0; index < size; ++index) {
      size_t shift_index = big ? size - 1u - index : index;
      output[index] = static_cast<jbyte>(bits >> (shift_index * 8u));
    }
  };
  auto check_bytes = [&](jobject buffer, jint offset, uint64_t bits, size_t size, bool big) {
    jbyte expected[8]{};
    encode(bits, size, big, expected);
    for (size_t index = 0; index < size; ++index) {
      if (env->CallByteMethod(buffer, get_byte, offset + static_cast<jint>(index)) != expected[index] ||
          env->ExceptionCheck()) return false;
    }
    return true;
  };
  auto write_bytes = [&](jobject buffer, jint offset, uint64_t bits, size_t size, bool big) {
    jbyte bytes[8]{};
    encode(bits, size, big, bytes);
    for (size_t index = 0; index < size; ++index) {
      jobject result = env->CallObjectMethod(
          buffer, put_byte, offset + static_cast<jint>(index), bytes[index]);
      if (result) env->DeleteLocalRef(result);
      if (env->ExceptionCheck()) return false;
    }
    return true;
  };
  auto force_gc = [&]() {
    art::ScopedThreadStateChange native(self, art::ThreadState::kNative);
    art::Runtime::Current()->GetHeap()->CollectGarbage(false);
  };
  auto take_exception = [&](jclass expected) {
    jthrowable thrown = env->ExceptionOccurred();
    if (thrown) env->ExceptionClear();
    bool correct = thrown && env->IsInstanceOf(thrown, expected);
    if (thrown) env->DeleteLocalRef(thrown);
    return correct;
  };

  int int_operations = 0;
  int long_operations = 0;
  int unaligned_operations = 0;
  int contract_failures = 0;
  for (int phase = 0; phase < 3; ++phase) {
    if (phase) for (auto* method : methods) {
      if (!jit->CompileMethod(method, self,
          phase == 1 ? art::CompilationKind::kBaseline : art::CompilationKind::kOptimized, false) ||
          !jit->GetCodeCache()->ContainsPc(method->GetEntryPointFromQuickCompiledCode())) return false;
    }
    for (int storage = 0; storage < 2; ++storage) for (int order = 0; order < 2; ++order) {
      bool big = order == 1;
      jobject buffer = buffers[storage];
      const jint int_values[] = {jint(0), jint(-1), jint(0x12345678)};
      for (jint seed : int_values) {
        jint update = seed ^ jint(0xa55a5aa5);
        jvalue args[5]{};
        args[0].l = handles[order]; args[1].l = buffer; args[2].i = 0; args[3].i = seed;
        env->CallStaticVoidMethodA(java_owner, ids[1], args);
        if (!check_bytes(buffer, 0, static_cast<uint32_t>(seed), 4, big)) return false;
        force_gc();
        if (env->CallStaticIntMethodA(java_owner, ids[0], args) != seed || env->ExceptionCheck()) return false;
        ++int_operations;
        args[3].i = seed ^ 1; args[4].i = update;
        if (env->CallStaticBooleanMethodA(java_owner, ids[2], args) != JNI_FALSE ||
            !check_bytes(buffer, 0, static_cast<uint32_t>(seed), 4, big)) return false;
        ++int_operations;
        args[3].i = seed;
        if (env->CallStaticBooleanMethodA(java_owner, ids[2], args) != JNI_TRUE ||
            !check_bytes(buffer, 0, static_cast<uint32_t>(update), 4, big)) return false;
        ++int_operations;
        constexpr uint32_t kIntDelta = UINT32_C(0x1234567);
        args[3].i = static_cast<jint>(kIntDelta);
        if (env->CallStaticIntMethodA(java_owner, ids[3], args) != update ||
            !check_bytes(buffer, 0, static_cast<uint32_t>(update) + kIntDelta, 4, big)) return false;
        ++int_operations;
        if (!write_bytes(buffer, 0, static_cast<uint32_t>(seed), 4, big) ||
            env->CallStaticIntMethodA(java_owner, ids[0], args) != seed || env->ExceptionCheck()) return false;
        ++int_operations;
      }

      jint long_index = storage == 0 ? 4 : 0;
      const jlong long_values[] = {jlong(0), jlong(-1),
          JitVarHandleByteViewLongFromBits(UINT64_C(0x0123456789abcdef))};
      for (jlong seed : long_values) {
        uint64_t seed_bits = JitVarHandleByteViewLongBits(seed);
        jlong update = JitVarHandleByteViewLongFromBits(seed_bits ^ UINT64_C(0xa55a5aa53cc3c33c));
        jvalue args[5]{};
        args[0].l = handles[2 + order]; args[1].l = buffer; args[2].i = long_index; args[3].j = seed;
        env->CallStaticVoidMethodA(java_owner, ids[5], args);
        if (!check_bytes(buffer, long_index, seed_bits, 8, big)) return false;
        force_gc();
        if (JitVarHandleByteViewLongBits(env->CallStaticLongMethodA(java_owner, ids[4], args)) != seed_bits ||
            env->ExceptionCheck()) return false;
        ++long_operations;
        args[3].j = JitVarHandleByteViewLongFromBits(seed_bits ^ 1u); args[4].j = update;
        if (env->CallStaticBooleanMethodA(java_owner, ids[6], args) != JNI_FALSE ||
            !check_bytes(buffer, long_index, seed_bits, 8, big)) return false;
        ++long_operations;
        args[3].j = seed;
        if (env->CallStaticBooleanMethodA(java_owner, ids[6], args) != JNI_TRUE ||
            !check_bytes(buffer, long_index, JitVarHandleByteViewLongBits(update), 8, big)) return false;
        ++long_operations;
        constexpr uint64_t kLongDelta = UINT64_C(0x123456789abcdef);
        args[3].j = JitVarHandleByteViewLongFromBits(kLongDelta);
        if (JitVarHandleByteViewLongBits(env->CallStaticLongMethodA(java_owner, ids[7], args)) !=
                JitVarHandleByteViewLongBits(update) ||
            !check_bytes(buffer, long_index, JitVarHandleByteViewLongBits(update) + kLongDelta, 8, big)) return false;
        ++long_operations;
        if (!write_bytes(buffer, long_index, seed_bits, 8, big) ||
            JitVarHandleByteViewLongBits(env->CallStaticLongMethodA(java_owner, ids[4], args)) != seed_bits ||
            env->ExceptionCheck()) return false;
        ++long_operations;
      }

      for (int group = 0; group < 2; ++group) {
        int base = group * 4;
        jvalue args[5]{};
        args[0].l = handles[group * 2 + order]; args[1].l = buffer; args[2].i = 1;
        if (group == 0) args[3].i = jint(0x12345678);
        else args[3].j = JitVarHandleByteViewLongFromBits(UINT64_C(0x0123456789abcdef));
        env->CallStaticVoidMethodA(java_owner, ids[base + 1], args);
        if (env->ExceptionCheck()) return false;
        if ((group == 0 && env->CallStaticIntMethodA(java_owner, ids[base], args) != jint(0x12345678)) ||
            (group == 1 && JitVarHandleByteViewLongBits(
                env->CallStaticLongMethodA(java_owner, ids[base], args)) != UINT64_C(0x0123456789abcdef)) ||
            env->ExceptionCheck()) return false;
        ++unaligned_operations;
        if (group == 0) args[4].i = 7; else args[4].j = 7;
        env->CallStaticBooleanMethodA(java_owner, ids[base + 2], args);
        if (!take_exception(illegal_state)) return false;
        ++contract_failures;
      }

      jobject read_only_buffer = env->CallObjectMethod(buffer, as_read_only);
      if (!read_only_buffer || env->ExceptionCheck()) return false;
      for (int group = 0; group < 2; ++group) for (int operation = 1; operation < 4; ++operation) {
        int kind = group * 4 + operation;
        jvalue args[5]{};
        args[0].l = handles[group * 2 + order]; args[1].l = read_only_buffer;
        args[2].i = group == 0 ? 0 : long_index;
        if (operation == 1) env->CallStaticVoidMethodA(java_owner, ids[kind], args);
        else if (operation == 2) env->CallStaticBooleanMethodA(java_owner, ids[kind], args);
        else if (group == 0) env->CallStaticIntMethodA(java_owner, ids[kind], args);
        else env->CallStaticLongMethodA(java_owner, ids[kind], args);
        if (!take_exception(read_only)) return false;
        ++contract_failures;
      }
      env->DeleteLocalRef(read_only_buffer);
    }

    for (int kind = 0; kind < 8; ++kind) for (int order = 0; order < 2; ++order) {
      int group = kind / 4;
      int operation = kind % 4;
      for (int storage = 0; storage < 2; ++storage) {
        jobject handle = handles[group * 2 + order];
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
          args[1].l = failure == 1 ? nullptr : buffers[storage];
          invoke(args);
          if (!take_exception(npe)) return false;
          ++contract_failures;
        }
        jint bad_indexes[] = {-1, group == 0 ? 29 : 25};
        for (jint bad_index : bad_indexes) {
          jvalue args[5]{};
          args[0].l = handle; args[1].l = buffers[storage]; args[2].i = bad_index;
          invoke(args);
          if (!take_exception(bounds)) return false;
          ++contract_failures;
        }
      }
    }
  }

  for (jobject handle : handles) env->DeleteLocalRef(handle);
  for (jobject buffer : buffers) env->DeleteLocalRef(buffer);
  env->DeleteLocalRef(int_array_class);
  env->DeleteLocalRef(long_array_class);
  env->DeleteLocalRef(byte_buffer_class);
  env->DeleteLocalRef(bounds);
  env->DeleteLocalRef(illegal_state);
  env->DeleteLocalRef(read_only);
  std::cerr << "ART JIT ByteBuffer view VarHandle: int-operations=" << int_operations
            << " long-operations=" << long_operations
            << " unaligned-plain=" << unaligned_operations
            << " contract-failures=" << contract_failures << " PASS\n";
  return !env->ExceptionCheck();
}
}  // namespace aim_jni_acceptance_phase
