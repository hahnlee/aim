#pragma once
namespace aim_jni_acceptance_phase {
inline bool CheckJitVarHandleByteBufferViewNarrow16(JNIEnv* env, art::Thread* self,
    art::jit::Jit* jit, art::Handle<art::mirror::Class> owner, jclass java_owner, jclass npe) {
  auto factory = env->GetStaticMethodID(java_owner, "jitVarByteBufferViewHandle",
      "(Ljava/lang/Class;Z)Ljava/lang/invoke/VarHandle;");
  jclass short_array_class = env->FindClass("[S");
  jclass char_array_class = env->FindClass("[C");
  jobject handles[4] = {
      factory ? env->CallStaticObjectMethod(java_owner, factory, short_array_class, JNI_TRUE) : nullptr,
      factory ? env->CallStaticObjectMethod(java_owner, factory, short_array_class, JNI_FALSE) : nullptr,
      factory ? env->CallStaticObjectMethod(java_owner, factory, char_array_class, JNI_TRUE) : nullptr,
      factory ? env->CallStaticObjectMethod(java_owner, factory, char_array_class, JNI_FALSE) : nullptr};
  const char* names[] = {"jitVarBufferViewShortGet", "jitVarBufferViewShortSet",
                         "jitVarBufferViewShortCas", "jitVarBufferViewShortAdd",
                         "jitVarBufferViewCharGet", "jitVarBufferViewCharSet",
                         "jitVarBufferViewCharCas", "jitVarBufferViewCharAdd"};
  const char* sigs[] = {
      "(Ljava/lang/invoke/VarHandle;Ljava/nio/ByteBuffer;I)S",
      "(Ljava/lang/invoke/VarHandle;Ljava/nio/ByteBuffer;IS)V",
      "(Ljava/lang/invoke/VarHandle;Ljava/nio/ByteBuffer;ISS)Z",
      "(Ljava/lang/invoke/VarHandle;Ljava/nio/ByteBuffer;IS)S",
      "(Ljava/lang/invoke/VarHandle;Ljava/nio/ByteBuffer;I)C",
      "(Ljava/lang/invoke/VarHandle;Ljava/nio/ByteBuffer;IC)V",
      "(Ljava/lang/invoke/VarHandle;Ljava/nio/ByteBuffer;ICC)Z",
      "(Ljava/lang/invoke/VarHandle;Ljava/nio/ByteBuffer;IC)C"};
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
  auto encode = [](uint16_t bits, bool big, jbyte* output) {
    output[0] = static_cast<jbyte>(bits >> (big ? 8 : 0));
    output[1] = static_cast<jbyte>(bits >> (big ? 0 : 8));
  };
  auto check_bytes = [&](jobject buffer, jint offset, uint16_t bits, bool big) {
    jbyte expected[2]{};
    encode(bits, big, expected);
    for (int index = 0; index < 2; ++index) {
      if (env->CallByteMethod(buffer, get_byte, offset + index) != expected[index] ||
          env->ExceptionCheck()) return false;
    }
    return true;
  };
  auto write_bytes = [&](jobject buffer, uint16_t bits, bool big) {
    jbyte encoded[2]{};
    encode(bits, big, encoded);
    for (int index = 0; index < 2; ++index) {
      jobject result = env->CallObjectMethod(buffer, put_byte, index, encoded[index]);
      if (result) env->DeleteLocalRef(result);
      if (env->ExceptionCheck()) return false;
    }
    return true;
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
    for (int storage = 0; storage < 2; ++storage) for (int group = 0; group < 2; ++group)
        for (int order = 0; order < 2; ++order) {
      int base = group * 4;
      bool big = order == 1;
      jobject buffer = buffers[storage];
      for (uint16_t seed : values[group]) {
        jvalue args[5]{};
        args[0].l = handles[group * 2 + order]; args[1].l = buffer; args[2].i = 0;
        set_arg(args[3], group, seed);
        env->CallStaticVoidMethodA(java_owner, ids[base + 1], args);
        if (!check_bytes(buffer, 0, seed, big)) return false;
        force_gc();
        if (get_value(group, ids[base], args) != seed || env->ExceptionCheck()) return false;
        ++operations[group];
        if (!write_bytes(buffer, seed, big) || get_value(group, ids[base], args) != seed ||
            env->ExceptionCheck()) return false;
        ++operations[group];
      }
      for (int operation = 2; operation < 4; ++operation) {
        jvalue unsupported_args[5]{};
        unsupported_args[0].l = handles[group * 2 + order];
        unsupported_args[1].l = buffer;
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
      args[0].l = handles[group * 2 + order]; args[1].l = buffer; args[2].i = 1;
      set_arg(args[3], group, UINT16_C(0x1234));
      env->CallStaticVoidMethodA(java_owner, ids[base + 1], args);
      if (env->ExceptionCheck() || get_value(group, ids[base], args) != UINT16_C(0x1234) ||
          env->ExceptionCheck()) return false;
      ++unaligned_operations;

      jobject read_only_buffer = env->CallObjectMethod(buffer, as_read_only);
      if (!read_only_buffer || env->ExceptionCheck()) return false;
      jvalue ro_args[5]{};
      ro_args[0].l = handles[group * 2 + order]; ro_args[1].l = read_only_buffer;
      get_value(group, ids[base], ro_args);
      if (env->ExceptionCheck()) return false;
      env->CallStaticVoidMethodA(java_owner, ids[base + 1], ro_args);
      if (!take_exception(read_only)) return false;
      ++contract_failures;
      env->DeleteLocalRef(read_only_buffer);
    }
    for (int group = 0; group < 2; ++group) for (int operation = 0; operation < 2; ++operation)
        for (int order = 0; order < 2; ++order) for (int storage = 0; storage < 2; ++storage) {
      int kind = group * 4 + operation;
      auto invoke = [&](jvalue* args) {
        if (operation == 0 || operation == 3) get_value(group, ids[kind], args);
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
      for (jint bad_index : {-1, 31}) {
        jvalue args[5]{};
        args[0].l = handles[group * 2 + order]; args[1].l = buffers[storage]; args[2].i = bad_index;
        invoke(args);
        if (!take_exception(bounds)) return false;
        ++contract_failures;
      }
    }
  }
  for (jobject handle : handles) env->DeleteLocalRef(handle);
  for (jobject buffer : buffers) env->DeleteLocalRef(buffer);
  env->DeleteLocalRef(short_array_class);
  env->DeleteLocalRef(char_array_class);
  env->DeleteLocalRef(byte_buffer_class);
  env->DeleteLocalRef(bounds);
  env->DeleteLocalRef(illegal_state);
  env->DeleteLocalRef(read_only);
  env->DeleteLocalRef(unsupported);
  std::cerr << "ART JIT ByteBuffer narrow16 view VarHandle: short-operations=" << operations[0]
            << " char-operations=" << operations[1]
            << " unaligned-plain=" << unaligned_operations
            << " contract-failures=" << contract_failures << " PASS\n";
  return !env->ExceptionCheck();
}
}  // namespace aim_jni_acceptance_phase
