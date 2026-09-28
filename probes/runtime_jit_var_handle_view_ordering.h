#pragma once
namespace aim_jni_acceptance_phase {
inline bool CheckJitVarHandleViewOrdering(JNIEnv* env, art::Thread* self, art::jit::Jit* jit,
    art::Handle<art::mirror::Class> owner, jclass java_owner, jclass npe) {
  const char* suffixes[] = {
      "GetOpaque", "GetAcquire", "GetVolatile", "SetOpaque", "SetRelease", "SetVolatile",
      "WeakPlain", "WeakAcquire", "WeakRelease", "WeakVolatile", "ExchangeAcquire",
      "ExchangeRelease", "SwapAcquire", "SwapRelease", "AddAcquire", "AddRelease", "Or",
      "OrAcquire", "OrRelease", "And", "AndAcquire", "AndRelease", "XorAcquire", "XorRelease"};
  auto signature_kind = [](int kind) {
    return kind < 3 ? 0 : kind < 6 ? 1 : kind < 10 ? 2 : kind < 12 ? 3 : 4;
  };
  constexpr const char* kByteIntSigs[] = {
      "(Ljava/lang/invoke/VarHandle;[BI)I", "(Ljava/lang/invoke/VarHandle;[BII)V",
      "(Ljava/lang/invoke/VarHandle;[BIII)Z", "(Ljava/lang/invoke/VarHandle;[BIII)I",
      "(Ljava/lang/invoke/VarHandle;[BII)I"};
  constexpr const char* kByteLongSigs[] = {
      "(Ljava/lang/invoke/VarHandle;[BI)J", "(Ljava/lang/invoke/VarHandle;[BIJ)V",
      "(Ljava/lang/invoke/VarHandle;[BIJJ)Z", "(Ljava/lang/invoke/VarHandle;[BIJJ)J",
      "(Ljava/lang/invoke/VarHandle;[BIJ)J"};
  constexpr const char* kBufferIntSigs[] = {
      "(Ljava/lang/invoke/VarHandle;Ljava/nio/ByteBuffer;I)I",
      "(Ljava/lang/invoke/VarHandle;Ljava/nio/ByteBuffer;II)V",
      "(Ljava/lang/invoke/VarHandle;Ljava/nio/ByteBuffer;III)Z",
      "(Ljava/lang/invoke/VarHandle;Ljava/nio/ByteBuffer;III)I",
      "(Ljava/lang/invoke/VarHandle;Ljava/nio/ByteBuffer;II)I"};
  constexpr const char* kBufferLongSigs[] = {
      "(Ljava/lang/invoke/VarHandle;Ljava/nio/ByteBuffer;I)J",
      "(Ljava/lang/invoke/VarHandle;Ljava/nio/ByteBuffer;IJ)V",
      "(Ljava/lang/invoke/VarHandle;Ljava/nio/ByteBuffer;IJJ)Z",
      "(Ljava/lang/invoke/VarHandle;Ljava/nio/ByteBuffer;IJJ)J",
      "(Ljava/lang/invoke/VarHandle;Ljava/nio/ByteBuffer;IJ)J"};
  jmethodID ids[2][2][24]{};
  art::ArtMethod* methods[2][2][24]{};
  for (int target = 0; target < 2; ++target) for (int group = 0; group < 2; ++group) {
    for (int kind = 0; kind < 24; ++kind) {
      std::string name = target == 0 ? "jitVarByteView" : "jitVarBufferView";
      name += group == 0 ? "Int" : "Long";
      name += suffixes[kind];
      int sk = signature_kind(kind);
      const char* sig = target == 0 ? (group == 0 ? kByteIntSigs[sk] : kByteLongSigs[sk])
          : (group == 0 ? kBufferIntSigs[sk] : kBufferLongSigs[sk]);
      ids[target][group][kind] = env->GetStaticMethodID(java_owner, name.c_str(), sig);
      methods[target][group][kind] =
          owner->FindClassMethod(name.c_str(), sig, art::kRuntimePointerSize);
      if (!ids[target][group][kind] || !methods[target][group][kind] || env->ExceptionCheck()) {
        std::cerr << "ART JIT view ordering lookup failed name=" << name << " sig=" << sig << "\n";
        if (env->ExceptionCheck()) env->ExceptionDescribe();
        return false;
      }
      art::CodeItemDataAccessor code(
          *methods[target][group][kind]->GetDexFile(), methods[target][group][kind]->GetCodeItem());
      bool polymorphic = false;
      for (const auto& pair : code) {
        polymorphic |= pair.Inst().Opcode() == art::Instruction::INVOKE_POLYMORPHIC ||
            pair.Inst().Opcode() == art::Instruction::INVOKE_POLYMORPHIC_RANGE;
      }
      if (!polymorphic) {
        std::cerr << "ART JIT view ordering non-polymorphic name=" << name << "\n";
        return false;
      }
    }
  }
  auto byte_factory = env->GetStaticMethodID(java_owner, "jitVarByteArrayViewHandle",
      "(Ljava/lang/Class;Z)Ljava/lang/invoke/VarHandle;");
  auto buffer_factory = env->GetStaticMethodID(java_owner, "jitVarByteBufferViewHandle",
      "(Ljava/lang/Class;Z)Ljava/lang/invoke/VarHandle;");
  jclass int_array_class = env->FindClass("[I");
  jclass long_array_class = env->FindClass("[J");
  jobject handles[2][2][2]{};
  for (int target = 0; target < 2; ++target) for (int group = 0; group < 2; ++group) {
    for (int order = 0; order < 2; ++order) {
      handles[target][group][order] = env->CallStaticObjectMethod(java_owner,
          target == 0 ? byte_factory : buffer_factory,
          group == 0 ? int_array_class : long_array_class, order ? JNI_FALSE : JNI_TRUE);
    }
  }
  jbyteArray byte_arrays[2] = {env->NewByteArray(32), env->NewByteArray(32)};
  jclass buffer_class = env->FindClass("java/nio/ByteBuffer");
  auto allocate = env->GetStaticMethodID(buffer_class, "allocate", "(I)Ljava/nio/ByteBuffer;");
  auto allocate_direct =
      env->GetStaticMethodID(buffer_class, "allocateDirect", "(I)Ljava/nio/ByteBuffer;");
  auto get_byte = env->GetMethodID(buffer_class, "get", "(I)B");
  auto put_byte = env->GetMethodID(buffer_class, "put", "(IB)Ljava/nio/ByteBuffer;");
  auto as_read_only = env->GetMethodID(
      buffer_class, "asReadOnlyBuffer", "()Ljava/nio/ByteBuffer;");
  jobject buffers[2] = {
      env->CallStaticObjectMethod(buffer_class, allocate, 32),
      env->CallStaticObjectMethod(buffer_class, allocate_direct, 32)};
  jclass bounds = env->FindClass("java/lang/IndexOutOfBoundsException");
  jclass illegal_state = env->FindClass("java/lang/IllegalStateException");
  jclass read_only = env->FindClass("java/nio/ReadOnlyBufferException");
  for (int target = 0; target < 2; ++target) for (int group = 0; group < 2; ++group)
    for (int order = 0; order < 2; ++order)
      if (!handles[target][group][order]) return false;
  if (!byte_arrays[0] || !byte_arrays[1] || !buffers[0] || !buffers[1] || !get_byte ||
      !put_byte || !as_read_only || !bounds || !illegal_state || !read_only ||
      env->ExceptionCheck()) return false;
  auto encode = [](uint64_t bits, size_t size, bool big, jbyte* output) {
    for (size_t n = 0; n < size; ++n) {
      size_t shift = big ? size - 1u - n : n;
      output[n] = static_cast<jbyte>(bits >> (shift * 8u));
    }
  };
  auto write_bits = [&](int target, jobject backing, jint index, uint64_t bits,
                        size_t size, bool big) {
    jbyte bytes[8]{}; encode(bits, size, big, bytes);
    if (target == 0) env->SetByteArrayRegion(static_cast<jbyteArray>(backing), index, size, bytes);
    else for (size_t n = 0; n < size; ++n) {
      jobject result = env->CallObjectMethod(backing, put_byte, index + n, bytes[n]);
      if (result) env->DeleteLocalRef(result);
    }
    return !env->ExceptionCheck();
  };
  auto read_bits = [&](int target, jobject backing, jint index, size_t size, bool big) {
    jbyte bytes[8]{};
    if (target == 0) env->GetByteArrayRegion(static_cast<jbyteArray>(backing), index, size, bytes);
    else for (size_t n = 0; n < size; ++n)
      bytes[n] = env->CallByteMethod(backing, get_byte, index + n);
    uint64_t bits = 0;
    for (size_t n = 0; n < size; ++n) {
      size_t shift = big ? size - 1u - n : n;
      bits |= static_cast<uint64_t>(static_cast<uint8_t>(bytes[n])) << (shift * 8u);
    }
    return bits;
  };
  auto take_exception = [&](jclass expected) {
    jthrowable thrown = env->ExceptionOccurred();
    if (thrown) env->ExceptionClear();
    bool correct = thrown && env->IsInstanceOf(thrown, expected);
    if (thrown) env->DeleteLocalRef(thrown);
    return correct;
  };
  auto invoke = [&](int target, int group, int kind, jvalue* args) -> uint64_t {
    int sk = signature_kind(kind);
    if (sk == 1) { env->CallStaticVoidMethodA(java_owner, ids[target][group][kind], args); return 0; }
    if (sk == 2) return env->CallStaticBooleanMethodA(java_owner, ids[target][group][kind], args);
    return group == 0
        ? static_cast<uint32_t>(env->CallStaticIntMethodA(java_owner, ids[target][group][kind], args))
        : JitVarHandleByteViewLongBits(
              env->CallStaticLongMethodA(java_owner, ids[target][group][kind], args));
  };
  auto set_arg = [](jvalue* args, int slot, int group, uint64_t bits) {
    if (group == 0) args[slot].i = static_cast<jint>(bits);
    else args[slot].j = JitVarHandleByteViewLongFromBits(bits);
  };
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
      int storage_count = target == 0 ? 1 : 2;
      for (int storage = 0; storage < storage_count; ++storage) for (int order = 0; order < 2; ++order) {
        jobject backing = target == 0 ? jobject(byte_arrays[group]) : buffers[storage];
        jobject handle = handles[target][group][order];
        bool big = order == 1;
        jint index = group == 0 ? 0 : target == 0 ? 4 : storage == 0 ? 4 : 0;
        uint64_t mask = group == 0 ? UINT64_C(0xffffffff) : UINT64_MAX;
        const uint64_t values[] = {0, mask, group == 0 ? UINT64_C(0x12345678)
                                                         : UINT64_C(0x0123456789abcdef)};
        for (uint64_t seed : values) {
          uint64_t update = (seed ^ UINT64_C(0xa55a5aa53cc3c33c)) & mask;
          for (int kind = 0; kind < 3; ++kind) {
            if (!write_bits(target, backing, index, seed, size, big)) return false;
            jvalue args[5]{}; args[0].l = handle; args[1].l = backing; args[2].i = index;
            if (invoke(target, group, kind, args) != seed || env->ExceptionCheck()) return false;
            ++operation_groups;
          }
          for (int kind = 3; kind < 6; ++kind) {
            if (!write_bits(target, backing, index, seed, size, big)) return false;
            jvalue args[5]{}; args[0].l = handle; args[1].l = backing; args[2].i = index;
            set_arg(args, 3, group, update); invoke(target, group, kind, args);
            if (read_bits(target, backing, index, size, big) != update || env->ExceptionCheck()) return false;
            ++operation_groups;
          }
          for (int kind = 6; kind < 10; ++kind) {
            if (!write_bits(target, backing, index, seed, size, big)) return false;
            jvalue args[5]{}; args[0].l = handle; args[1].l = backing; args[2].i = index;
            set_arg(args, 3, group, seed ^ 1); set_arg(args, 4, group, update);
            if (invoke(target, group, kind, args) != JNI_FALSE ||
                read_bits(target, backing, index, size, big) != seed) return false;
            ++operation_groups;
            set_arg(args, 3, group, seed);
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
            set_arg(args, 3, group, seed ^ 1); set_arg(args, 4, group, update);
            if (invoke(target, group, kind, args) != seed ||
                read_bits(target, backing, index, size, big) != seed) return false;
            ++operation_groups;
            set_arg(args, 3, group, seed);
            if (invoke(target, group, kind, args) != seed ||
                read_bits(target, backing, index, size, big) != update) return false;
            ++operation_groups;
          }
          for (int kind = 12; kind < 24; ++kind) {
            if (!write_bits(target, backing, index, seed, size, big)) return false;
            uint64_t operand = kind < 14 ? update : kind < 16 ? UINT64_C(0x13579bdf)
                : UINT64_C(0x0f0ff0f00ff00ff0) & mask;
            uint64_t expected = kind < 14 ? update : kind < 16 ? (seed + operand) & mask
                : kind < 19 ? seed | operand : kind < 22 ? seed & operand : seed ^ operand;
            jvalue args[5]{}; args[0].l = handle; args[1].l = backing; args[2].i = index;
            set_arg(args, 3, group, operand);
            if (invoke(target, group, kind, args) != seed ||
                read_bits(target, backing, index, size, big) != expected || env->ExceptionCheck())
              return false;
            ++operation_groups;
          }
        }
        for (int kind = 0; kind < 24; ++kind) {
          for (int bad = 0; bad < 5; ++bad) {
            jvalue args[5]{};
            args[0].l = bad == 0 ? nullptr : handle;
            args[1].l = bad == 1 ? nullptr : backing;
            args[2].i = bad == 2 ? -1 : bad == 3 ? 33 - size : bad == 4 ? 1 : index;
            invoke(target, group, kind, args);
            jclass expected = bad < 2 ? npe : bad < 4 ? bounds : illegal_state;
            if (!take_exception(expected)) return false;
            ++contract_groups;
          }
        }
        if (target == 1) {
          jobject ro = env->CallObjectMethod(backing, as_read_only);
          if (!ro || env->ExceptionCheck()) return false;
          for (int kind = 3; kind < 24; ++kind) {
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
  env->DeleteLocalRef(int_array_class); env->DeleteLocalRef(long_array_class);
  env->DeleteLocalRef(buffer_class); env->DeleteLocalRef(bounds);
  env->DeleteLocalRef(illegal_state); env->DeleteLocalRef(read_only);
  std::cerr << "ART JIT VarHandle int/long byte-array+ByteBuffer ordering: operation-groups="
            << operation_groups << " contract-groups=" << contract_groups << " PASS\n";
  return !env->ExceptionCheck();
}
}  // namespace aim_jni_acceptance_phase
