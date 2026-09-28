#pragma once
namespace aim_jni_acceptance_phase {
inline bool CheckJitVarHandleByteBufferViewFp(JNIEnv* env, art::Thread* self,
    art::jit::Jit* jit, art::Handle<art::mirror::Class> owner, jclass java_owner, jclass npe) {
  auto factory = env->GetStaticMethodID(java_owner, "jitVarByteBufferViewHandle",
      "(Ljava/lang/Class;Z)Ljava/lang/invoke/VarHandle;");
  jclass float_array_class = env->FindClass("[F");
  jclass double_array_class = env->FindClass("[D");
  jobject handles[4] = {
      factory ? env->CallStaticObjectMethod(java_owner, factory, float_array_class, JNI_TRUE) : nullptr,
      factory ? env->CallStaticObjectMethod(java_owner, factory, float_array_class, JNI_FALSE) : nullptr,
      factory ? env->CallStaticObjectMethod(java_owner, factory, double_array_class, JNI_TRUE) : nullptr,
      factory ? env->CallStaticObjectMethod(java_owner, factory, double_array_class, JNI_FALSE) : nullptr};
  const char* names[] = {"jitVarBufferViewFloatGet", "jitVarBufferViewFloatSet",
                         "jitVarBufferViewFloatCas", "jitVarBufferViewFloatAdd",
                         "jitVarBufferViewDoubleGet", "jitVarBufferViewDoubleSet",
                         "jitVarBufferViewDoubleCas", "jitVarBufferViewDoubleAdd"};
  const char* sigs[] = {
      "(Ljava/lang/invoke/VarHandle;Ljava/nio/ByteBuffer;I)F",
      "(Ljava/lang/invoke/VarHandle;Ljava/nio/ByteBuffer;IF)V",
      "(Ljava/lang/invoke/VarHandle;Ljava/nio/ByteBuffer;IFF)Z",
      "(Ljava/lang/invoke/VarHandle;Ljava/nio/ByteBuffer;IF)F",
      "(Ljava/lang/invoke/VarHandle;Ljava/nio/ByteBuffer;I)D",
      "(Ljava/lang/invoke/VarHandle;Ljava/nio/ByteBuffer;ID)V",
      "(Ljava/lang/invoke/VarHandle;Ljava/nio/ByteBuffer;IDD)Z",
      "(Ljava/lang/invoke/VarHandle;Ljava/nio/ByteBuffer;ID)D"};
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
  auto allocate = env->GetStaticMethodID(byte_buffer_class, "allocate", "(I)Ljava/nio/ByteBuffer;");
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
  jclass unsupported = env->FindClass("java/lang/UnsupportedOperationException");
  if (!byte_buffer_class || !get_byte || !put_byte || !as_read_only || !buffers[0] || !buffers[1] ||
      !bounds || !illegal_state || !read_only || !unsupported || env->ExceptionCheck()) return false;
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
    jbyte encoded[8]{};
    encode(bits, size, big, encoded);
    for (size_t index = 0; index < size; ++index) {
      jobject result = env->CallObjectMethod(
          buffer, put_byte, offset + static_cast<jint>(index), encoded[index]);
      if (result) env->DeleteLocalRef(result);
      if (env->ExceptionCheck()) return false;
    }
    return true;
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
    for (int storage = 0; storage < 2; ++storage) for (int group = 0; group < 2; ++group)
        for (int order = 0; order < 2; ++order) {
      int base = group * 4;
      size_t size = group == 0 ? 4u : 8u;
      jint index = group == 0 || storage == 1 ? 0 : 4;
      uint64_t mask = group == 0 ? UINT32_MAX : UINT64_MAX;
      bool big = order == 1;
      jobject buffer = buffers[storage];
      for (uint64_t seed : values[group]) {
        uint64_t update = (seed ^ (group == 0 ? UINT64_C(0x80000000) :
                                             UINT64_C(0x8000000000000000))) & mask;
        jvalue args[5]{};
        args[0].l = handles[group * 2 + order]; args[1].l = buffer; args[2].i = index;
        set_arg(args[3], group, seed);
        env->CallStaticVoidMethodA(java_owner, ids[base + 1], args);
        if (!check_bytes(buffer, index, seed, size, big)) return false;
        force_gc();
        if (get_bits(group, ids[base], args) != seed || env->ExceptionCheck()) return false;
        ++operations[group];
        set_arg(args[3], group, (seed ^ 1u) & mask); set_arg(args[4], group, update);
        if (env->CallStaticBooleanMethodA(java_owner, ids[base + 2], args) != JNI_FALSE ||
            !check_bytes(buffer, index, seed, size, big)) return false;
        ++operations[group];
        set_arg(args[3], group, seed);
        if (env->CallStaticBooleanMethodA(java_owner, ids[base + 2], args) != JNI_TRUE ||
            !check_bytes(buffer, index, update, size, big)) return false;
        ++operations[group];
        if (!write_bytes(buffer, index, seed, size, big) ||
            get_bits(group, ids[base], args) != seed || env->ExceptionCheck()) return false;
        ++operations[group];
      }
      jvalue unsupported_args[5]{};
      unsupported_args[0].l = handles[group * 2 + order]; unsupported_args[1].l = buffer;
      get_bits(group, ids[base + 3], unsupported_args);
      if (!take_exception(unsupported)) return false;
      ++contract_failures;

      jvalue args[5]{};
      args[0].l = handles[group * 2 + order]; args[1].l = buffer; args[2].i = 1;
      set_arg(args[3], group, values[group][4]);
      env->CallStaticVoidMethodA(java_owner, ids[base + 1], args);
      if (env->ExceptionCheck() || get_bits(group, ids[base], args) != values[group][4] ||
          env->ExceptionCheck()) return false;
      ++unaligned_operations;
      set_arg(args[4], group, values[group][0]);
      env->CallStaticBooleanMethodA(java_owner, ids[base + 2], args);
      if (!take_exception(illegal_state)) return false;
      ++contract_failures;

      jobject read_only_buffer = env->CallObjectMethod(buffer, as_read_only);
      if (!read_only_buffer || env->ExceptionCheck()) return false;
      jvalue ro_args[5]{};
      ro_args[0].l = handles[group * 2 + order]; ro_args[1].l = read_only_buffer;
      ro_args[2].i = index;
      get_bits(group, ids[base], ro_args);
      if (env->ExceptionCheck()) return false;
      env->CallStaticVoidMethodA(java_owner, ids[base + 1], ro_args);
      if (!take_exception(read_only)) return false;
      ++contract_failures;
      env->CallStaticBooleanMethodA(java_owner, ids[base + 2], ro_args);
      if (!take_exception(read_only)) return false;
      ++contract_failures;
      env->DeleteLocalRef(read_only_buffer);
    }
    for (int group = 0; group < 2; ++group) for (int operation = 0; operation < 3; ++operation)
        for (int order = 0; order < 2; ++order) for (int storage = 0; storage < 2; ++storage) {
      int kind = group * 4 + operation;
      auto invoke = [&](jvalue* args) {
        if (operation == 0) get_bits(group, ids[kind], args);
        else if (operation == 1) env->CallStaticVoidMethodA(java_owner, ids[kind], args);
        else env->CallStaticBooleanMethodA(java_owner, ids[kind], args);
      };
      for (int failure = 0; failure < 2; ++failure) {
        jvalue args[5]{};
        args[0].l = failure == 0 ? nullptr : handles[group * 2 + order];
        args[1].l = failure == 1 ? nullptr : buffers[storage];
        invoke(args);
        if (!take_exception(npe)) return false;
        ++contract_failures;
      }
      jint bad = group == 0 ? 29 : 25;
      for (jint bad_index : {-1, bad}) {
        jvalue args[5]{};
        args[0].l = handles[group * 2 + order]; args[1].l = buffers[storage];
        args[2].i = bad_index;
        invoke(args);
        if (!take_exception(bounds)) return false;
        ++contract_failures;
      }
    }
  }
  for (jobject handle : handles) env->DeleteLocalRef(handle);
  for (jobject buffer : buffers) env->DeleteLocalRef(buffer);
  env->DeleteLocalRef(float_array_class);
  env->DeleteLocalRef(double_array_class);
  env->DeleteLocalRef(byte_buffer_class);
  env->DeleteLocalRef(bounds);
  env->DeleteLocalRef(illegal_state);
  env->DeleteLocalRef(read_only);
  env->DeleteLocalRef(unsupported);
  std::cerr << "ART JIT ByteBuffer FP view VarHandle: float-operations=" << operations[0]
            << " double-operations=" << operations[1]
            << " unaligned-plain=" << unaligned_operations
            << " contract-failures=" << contract_failures << " PASS\n";
  return !env->ExceptionCheck();
}
}  // namespace aim_jni_acceptance_phase
