#pragma once
namespace aim_jni_acceptance_phase {
inline bool CheckJitVarHandleViewFpOrdering(JNIEnv* env, art::Thread* self,
    art::jit::Jit* jit, art::Handle<art::mirror::Class> owner, jclass java_owner, jclass npe) {
  const char* suffixes[] = {
      "GetOpaque", "GetAcquire", "GetVolatile", "SetOpaque", "SetRelease", "SetVolatile",
      "WeakPlain", "WeakAcquire", "WeakRelease", "WeakVolatile", "ExchangeAcquire",
      "ExchangeRelease", "SwapAcquire", "SwapRelease", "AddAcquire", "AddRelease", "Or",
      "OrAcquire", "OrRelease", "And", "AndAcquire", "AndRelease", "XorAcquire", "XorRelease"};
  auto signature_kind = [](int kind) {
    return kind < 3 ? 0 : kind < 6 ? 1 : kind < 10 ? 2 : kind < 12 ? 3 : 4;
  };
  constexpr const char* kByteFloatSigs[] = {
      "(Ljava/lang/invoke/VarHandle;[BI)F", "(Ljava/lang/invoke/VarHandle;[BIF)V",
      "(Ljava/lang/invoke/VarHandle;[BIFF)Z", "(Ljava/lang/invoke/VarHandle;[BIFF)F",
      "(Ljava/lang/invoke/VarHandle;[BIF)F"};
  constexpr const char* kByteDoubleSigs[] = {
      "(Ljava/lang/invoke/VarHandle;[BI)D", "(Ljava/lang/invoke/VarHandle;[BID)V",
      "(Ljava/lang/invoke/VarHandle;[BIDD)Z", "(Ljava/lang/invoke/VarHandle;[BIDD)D",
      "(Ljava/lang/invoke/VarHandle;[BID)D"};
  constexpr const char* kBufferFloatSigs[] = {
      "(Ljava/lang/invoke/VarHandle;Ljava/nio/ByteBuffer;I)F",
      "(Ljava/lang/invoke/VarHandle;Ljava/nio/ByteBuffer;IF)V",
      "(Ljava/lang/invoke/VarHandle;Ljava/nio/ByteBuffer;IFF)Z",
      "(Ljava/lang/invoke/VarHandle;Ljava/nio/ByteBuffer;IFF)F",
      "(Ljava/lang/invoke/VarHandle;Ljava/nio/ByteBuffer;IF)F"};
  constexpr const char* kBufferDoubleSigs[] = {
      "(Ljava/lang/invoke/VarHandle;Ljava/nio/ByteBuffer;I)D",
      "(Ljava/lang/invoke/VarHandle;Ljava/nio/ByteBuffer;ID)V",
      "(Ljava/lang/invoke/VarHandle;Ljava/nio/ByteBuffer;IDD)Z",
      "(Ljava/lang/invoke/VarHandle;Ljava/nio/ByteBuffer;IDD)D",
      "(Ljava/lang/invoke/VarHandle;Ljava/nio/ByteBuffer;ID)D"};
  jmethodID ids[2][2][24]{};
  art::ArtMethod* methods[2][2][24]{};
  for (int target = 0; target < 2; ++target) for (int group = 0; group < 2; ++group)
    for (int kind = 0; kind < 24; ++kind) {
      std::string name = target == 0 ? "jitVarByteView" : "jitVarBufferView";
      name += group == 0 ? "Float" : "Double";
      name += suffixes[kind];
      int sk = signature_kind(kind);
      const char* sig = target == 0 ? (group == 0 ? kByteFloatSigs[sk] : kByteDoubleSigs[sk])
          : (group == 0 ? kBufferFloatSigs[sk] : kBufferDoubleSigs[sk]);
      ids[target][group][kind] = env->GetStaticMethodID(java_owner, name.c_str(), sig);
      methods[target][group][kind] =
          owner->FindClassMethod(name.c_str(), sig, art::kRuntimePointerSize);
      if (!ids[target][group][kind] || !methods[target][group][kind] || env->ExceptionCheck())
        return false;
      art::CodeItemDataAccessor code(
          *methods[target][group][kind]->GetDexFile(), methods[target][group][kind]->GetCodeItem());
      bool polymorphic = false;
      for (const auto& pair : code) polymorphic |=
          pair.Inst().Opcode() == art::Instruction::INVOKE_POLYMORPHIC ||
          pair.Inst().Opcode() == art::Instruction::INVOKE_POLYMORPHIC_RANGE;
      if (!polymorphic) return false;
    }
  auto byte_factory = env->GetStaticMethodID(java_owner, "jitVarByteArrayViewHandle",
      "(Ljava/lang/Class;Z)Ljava/lang/invoke/VarHandle;");
  auto buffer_factory = env->GetStaticMethodID(java_owner, "jitVarByteBufferViewHandle",
      "(Ljava/lang/Class;Z)Ljava/lang/invoke/VarHandle;");
  jclass float_class = env->FindClass("[F");
  jclass double_class = env->FindClass("[D");
  jobject handles[2][2][2]{};
  for (int target = 0; target < 2; ++target) for (int group = 0; group < 2; ++group)
    for (int order = 0; order < 2; ++order)
      handles[target][group][order] = env->CallStaticObjectMethod(java_owner,
          target == 0 ? byte_factory : buffer_factory, group == 0 ? float_class : double_class,
          order ? JNI_FALSE : JNI_TRUE);
  jbyteArray byte_arrays[2] = {env->NewByteArray(32), env->NewByteArray(32)};
  jclass buffer_class = env->FindClass("java/nio/ByteBuffer");
  auto allocate = env->GetStaticMethodID(buffer_class, "allocate", "(I)Ljava/nio/ByteBuffer;");
  auto allocate_direct =
      env->GetStaticMethodID(buffer_class, "allocateDirect", "(I)Ljava/nio/ByteBuffer;");
  auto get_byte = env->GetMethodID(buffer_class, "get", "(I)B");
  auto put_byte = env->GetMethodID(buffer_class, "put", "(IB)Ljava/nio/ByteBuffer;");
  auto as_read_only = env->GetMethodID(
      buffer_class, "asReadOnlyBuffer", "()Ljava/nio/ByteBuffer;");
  jobject buffers[2] = {env->CallStaticObjectMethod(buffer_class, allocate, 32),
                        env->CallStaticObjectMethod(buffer_class, allocate_direct, 32)};
  jclass bounds = env->FindClass("java/lang/IndexOutOfBoundsException");
  jclass illegal_state = env->FindClass("java/lang/IllegalStateException");
  jclass unsupported = env->FindClass("java/lang/UnsupportedOperationException");
  jclass read_only = env->FindClass("java/nio/ReadOnlyBufferException");
  if (!byte_arrays[0] || !byte_arrays[1] || !buffers[0] || !buffers[1] || !get_byte ||
      !put_byte || !as_read_only || !bounds || !illegal_state || !unsupported || !read_only ||
      env->ExceptionCheck()) return false;
  auto encode = [](uint64_t bits, size_t size, bool big, jbyte* output) {
    for (size_t n = 0; n < size; ++n) {
      size_t shift = big ? size - 1u - n : n;
      output[n] = static_cast<jbyte>(bits >> (shift * 8u));
    }
  };
  auto write_bits = [&](int target, jobject backing, jint index, uint64_t bits,
                        size_t size, bool big) {
    jbyte data[8]{}; encode(bits, size, big, data);
    if (target == 0) env->SetByteArrayRegion(static_cast<jbyteArray>(backing), index, size, data);
    else for (size_t n = 0; n < size; ++n) {
      jobject result = env->CallObjectMethod(backing, put_byte, index + n, data[n]);
      if (result) env->DeleteLocalRef(result);
    }
    return !env->ExceptionCheck();
  };
  auto read_bits = [&](int target, jobject backing, jint index, size_t size, bool big) {
    jbyte data[8]{};
    if (target == 0) env->GetByteArrayRegion(static_cast<jbyteArray>(backing), index, size, data);
    else for (size_t n = 0; n < size; ++n) data[n] = env->CallByteMethod(backing, get_byte, index + n);
    uint64_t bits = 0;
    for (size_t n = 0; n < size; ++n) {
      size_t shift = big ? size - 1u - n : n;
      bits |= static_cast<uint64_t>(static_cast<uint8_t>(data[n])) << (shift * 8u);
    }
    return bits;
  };
  auto set_arg = [](jvalue& arg, int group, uint64_t bits) {
    if (group == 0) arg.f = JitVarHandleViewFloatFromBits(static_cast<uint32_t>(bits));
    else arg.d = JitVarHandleViewDoubleFromBits(bits);
  };
  auto invoke = [&](int target, int group, int kind, jvalue* args) -> uint64_t {
    int sk = signature_kind(kind);
    if (sk == 1) { env->CallStaticVoidMethodA(java_owner, ids[target][group][kind], args); return 0; }
    if (sk == 2) return env->CallStaticBooleanMethodA(java_owner, ids[target][group][kind], args);
    return group == 0 ? JitVarHandleViewFloatBits(
        env->CallStaticFloatMethodA(java_owner, ids[target][group][kind], args))
        : JitVarHandleViewDoubleBits(
            env->CallStaticDoubleMethodA(java_owner, ids[target][group][kind], args));
  };
  auto take_exception = [&](jclass expected) {
    jthrowable thrown = env->ExceptionOccurred(); if (thrown) env->ExceptionClear();
    bool correct = thrown && env->IsInstanceOf(thrown, expected);
    if (thrown) env->DeleteLocalRef(thrown); return correct;
  };
  const uint64_t values[2][5] = {
      {UINT32_C(0), UINT32_C(0x80000000), UINT32_C(0x7f800000),
       UINT32_C(0xff7fffff), UINT32_C(0x7fc12345)},
      {UINT64_C(0), UINT64_C(0x8000000000000000), UINT64_C(0x7ff0000000000000),
       UINT64_C(0xffefffffffffffff), UINT64_C(0x7ff8123456789abc)}};
  int operation_groups = 0;
  int contract_groups = 0;
  for (int phase = 0; phase < 3; ++phase) {
    if (phase) for (int target = 0; target < 2; ++target)
      for (int group = 0; group < 2; ++group) for (int kind = 0; kind < 24; ++kind) {
        auto* method = methods[target][group][kind];
        if (!jit->CompileMethod(method, self,
            phase == 1 ? art::CompilationKind::kBaseline : art::CompilationKind::kOptimized, false) ||
            !jit->GetCodeCache()->ContainsPc(method->GetEntryPointFromQuickCompiledCode())) return false;
      }
    for (int target = 0; target < 2; ++target) for (int group = 0; group < 2; ++group) {
      size_t size = group == 0 ? 4 : 8;
      uint64_t mask = group == 0 ? UINT32_MAX : UINT64_MAX;
      int storage_count = target == 0 ? 1 : 2;
      for (int storage = 0; storage < storage_count; ++storage) for (int order = 0; order < 2; ++order) {
        jobject backing = target == 0 ? jobject(byte_arrays[group]) : buffers[storage];
        jobject handle = handles[target][group][order]; bool big = order == 1;
        jint index = group == 0 ? 0 : target == 0 ? 4 : storage == 0 ? 4 : 0;
        for (uint64_t seed : values[group]) {
          uint64_t update = (seed ^ (group == 0 ? UINT64_C(0x80000000)
                                                : UINT64_C(0x8000000000000000))) & mask;
          for (int kind = 0; kind < 3; ++kind) {
            if (!write_bits(target, backing, index, seed, size, big)) return false;
            jvalue args[5]{}; args[0].l = handle; args[1].l = backing; args[2].i = index;
            if (invoke(target, group, kind, args) != seed || env->ExceptionCheck()) return false;
            ++operation_groups;
          }
          for (int kind = 3; kind < 6; ++kind) {
            if (!write_bits(target, backing, index, seed, size, big)) return false;
            jvalue args[5]{}; args[0].l = handle; args[1].l = backing; args[2].i = index;
            set_arg(args[3], group, update); invoke(target, group, kind, args);
            if (read_bits(target, backing, index, size, big) != update || env->ExceptionCheck()) return false;
            ++operation_groups;
          }
          for (int kind = 6; kind < 10; ++kind) {
            if (!write_bits(target, backing, index, seed, size, big)) return false;
            jvalue args[5]{}; args[0].l = handle; args[1].l = backing; args[2].i = index;
            set_arg(args[3], group, (seed ^ 1u) & mask); set_arg(args[4], group, update);
            if (invoke(target, group, kind, args) != JNI_FALSE ||
                read_bits(target, backing, index, size, big) != seed) return false;
            ++operation_groups;
            set_arg(args[3], group, seed);
            bool success = false;
            for (int retry = 0; retry < 100 && !success; ++retry)
              success = invoke(target, group, kind, args) == JNI_TRUE;
            if (!success || read_bits(target, backing, index, size, big) != update ||
                env->ExceptionCheck()) return false;
            ++operation_groups;
          }
          for (int kind = 10; kind < 12; ++kind) {
            if (!write_bits(target, backing, index, seed, size, big)) return false;
            jvalue args[5]{}; args[0].l = handle; args[1].l = backing; args[2].i = index;
            set_arg(args[3], group, (seed ^ 1u) & mask); set_arg(args[4], group, update);
            if (invoke(target, group, kind, args) != seed ||
                read_bits(target, backing, index, size, big) != seed) return false;
            ++operation_groups;
            set_arg(args[3], group, seed);
            if (invoke(target, group, kind, args) != seed ||
                read_bits(target, backing, index, size, big) != update) return false;
            ++operation_groups;
          }
          for (int kind = 12; kind < 14; ++kind) {
            if (!write_bits(target, backing, index, seed, size, big)) return false;
            jvalue args[5]{}; args[0].l = handle; args[1].l = backing; args[2].i = index;
            set_arg(args[3], group, update);
            if (invoke(target, group, kind, args) != seed ||
                read_bits(target, backing, index, size, big) != update || env->ExceptionCheck()) return false;
            ++operation_groups;
          }
        }
        for (int kind = 14; kind < 24; ++kind) {
          jvalue args[5]{}; args[0].l = handle; args[1].l = backing; args[2].i = index;
          invoke(target, group, kind, args);
          if (!take_exception(unsupported)) return false;
          ++contract_groups;
        }
        for (int kind = 0; kind < 14; ++kind) for (int bad = 0; bad < 5; ++bad) {
          jvalue args[5]{}; args[0].l = bad == 0 ? nullptr : handle;
          args[1].l = bad == 1 ? nullptr : backing;
          args[2].i = bad == 2 ? -1 : bad == 3 ? 33 - size : bad == 4 ? 1 : index;
          invoke(target, group, kind, args);
          if (!take_exception(bad < 2 ? npe : bad < 4 ? bounds : illegal_state)) return false;
          ++contract_groups;
        }
        if (target == 1) {
          jobject ro = env->CallObjectMethod(backing, as_read_only);
          for (int kind = 3; kind < 14; ++kind) {
            jvalue args[5]{}; args[0].l = handle; args[1].l = ro; args[2].i = index;
            invoke(target, group, kind, args);
            if (!take_exception(read_only)) return false;
            ++contract_groups;
          }
          env->DeleteLocalRef(ro);
        }
      }
    }
  }
  for (auto& target : handles) for (auto& group : target)
    for (jobject handle : group) env->DeleteLocalRef(handle);
  for (jbyteArray array : byte_arrays) env->DeleteLocalRef(array);
  for (jobject buffer : buffers) env->DeleteLocalRef(buffer);
  env->DeleteLocalRef(float_class); env->DeleteLocalRef(double_class);
  env->DeleteLocalRef(buffer_class); env->DeleteLocalRef(bounds);
  env->DeleteLocalRef(illegal_state); env->DeleteLocalRef(unsupported); env->DeleteLocalRef(read_only);
  std::cerr << "ART JIT VarHandle FP byte-array+ByteBuffer ordering: operation-groups="
            << operation_groups << " contract-groups=" << contract_groups << " PASS\n";
  return !env->ExceptionCheck();
}
}  // namespace aim_jni_acceptance_phase
