#pragma once
namespace aim_jni_acceptance_phase {
inline bool CheckJitVarHandleViewNarrow16Ordering(JNIEnv* env, art::Thread* self,
    art::jit::Jit* jit, art::Handle<art::mirror::Class> owner, jclass java_owner, jclass npe) {
  const char* suffixes[] = {
      "GetOpaque", "GetAcquire", "GetVolatile", "SetOpaque", "SetRelease", "SetVolatile",
      "WeakPlain", "WeakAcquire", "WeakRelease", "WeakVolatile", "ExchangeAcquire",
      "ExchangeRelease", "SwapAcquire", "SwapRelease", "AddAcquire", "AddRelease", "Or",
      "OrAcquire", "OrRelease", "And", "AndAcquire", "AndRelease", "XorAcquire", "XorRelease"};
  auto signature_kind = [](int kind) {
    return kind < 3 ? 0 : kind < 6 ? 1 : kind < 10 ? 2 : kind < 12 ? 3 : 4;
  };
  constexpr const char* kByteShortSigs[] = {
      "(Ljava/lang/invoke/VarHandle;[BI)S", "(Ljava/lang/invoke/VarHandle;[BIS)V",
      "(Ljava/lang/invoke/VarHandle;[BISS)Z", "(Ljava/lang/invoke/VarHandle;[BISS)S",
      "(Ljava/lang/invoke/VarHandle;[BIS)S"};
  constexpr const char* kByteCharSigs[] = {
      "(Ljava/lang/invoke/VarHandle;[BI)C", "(Ljava/lang/invoke/VarHandle;[BIC)V",
      "(Ljava/lang/invoke/VarHandle;[BICC)Z", "(Ljava/lang/invoke/VarHandle;[BICC)C",
      "(Ljava/lang/invoke/VarHandle;[BIC)C"};
  constexpr const char* kBufferShortSigs[] = {
      "(Ljava/lang/invoke/VarHandle;Ljava/nio/ByteBuffer;I)S",
      "(Ljava/lang/invoke/VarHandle;Ljava/nio/ByteBuffer;IS)V",
      "(Ljava/lang/invoke/VarHandle;Ljava/nio/ByteBuffer;ISS)Z",
      "(Ljava/lang/invoke/VarHandle;Ljava/nio/ByteBuffer;ISS)S",
      "(Ljava/lang/invoke/VarHandle;Ljava/nio/ByteBuffer;IS)S"};
  constexpr const char* kBufferCharSigs[] = {
      "(Ljava/lang/invoke/VarHandle;Ljava/nio/ByteBuffer;I)C",
      "(Ljava/lang/invoke/VarHandle;Ljava/nio/ByteBuffer;IC)V",
      "(Ljava/lang/invoke/VarHandle;Ljava/nio/ByteBuffer;ICC)Z",
      "(Ljava/lang/invoke/VarHandle;Ljava/nio/ByteBuffer;ICC)C",
      "(Ljava/lang/invoke/VarHandle;Ljava/nio/ByteBuffer;IC)C"};
  jmethodID ids[2][2][24]{};
  art::ArtMethod* methods[2][2][24]{};
  for (int target = 0; target < 2; ++target) for (int group = 0; group < 2; ++group)
    for (int kind = 0; kind < 24; ++kind) {
      std::string name = target == 0 ? "jitVarByteView" : "jitVarBufferView";
      name += group == 0 ? "Short" : "Char";
      name += suffixes[kind];
      int sk = signature_kind(kind);
      const char* sig = target == 0 ? (group == 0 ? kByteShortSigs[sk] : kByteCharSigs[sk])
          : (group == 0 ? kBufferShortSigs[sk] : kBufferCharSigs[sk]);
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
  jclass short_class = env->FindClass("[S");
  jclass char_class = env->FindClass("[C");
  jobject handles[2][2][2]{};
  for (int target = 0; target < 2; ++target) for (int group = 0; group < 2; ++group)
    for (int order = 0; order < 2; ++order)
      handles[target][group][order] = env->CallStaticObjectMethod(java_owner,
          target == 0 ? byte_factory : buffer_factory, group == 0 ? short_class : char_class,
          order ? JNI_FALSE : JNI_TRUE);
  jbyteArray bytes = env->NewByteArray(32);
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
  if (!bytes || !buffers[0] || !buffers[1] || !get_byte || !put_byte || !as_read_only ||
      !bounds || !illegal_state || !unsupported || !read_only || env->ExceptionCheck()) return false;
  auto write_bits = [&](int target, jobject backing, uint16_t bits, bool big) {
    jbyte encoded[2] = {static_cast<jbyte>(bits >> (big ? 8 : 0)),
                        static_cast<jbyte>(bits >> (big ? 0 : 8))};
    if (target == 0) env->SetByteArrayRegion(static_cast<jbyteArray>(backing), 0, 2, encoded);
    else for (int n = 0; n < 2; ++n) {
      jobject result = env->CallObjectMethod(backing, put_byte, n, encoded[n]);
      if (result) env->DeleteLocalRef(result);
    }
    return !env->ExceptionCheck();
  };
  auto read_bits = [&](int target, jobject backing, bool big) {
    jbyte data[2]{};
    if (target == 0) env->GetByteArrayRegion(static_cast<jbyteArray>(backing), 0, 2, data);
    else for (int n = 0; n < 2; ++n) data[n] = env->CallByteMethod(backing, get_byte, n);
    return static_cast<uint16_t>((static_cast<uint8_t>(data[big ? 1 : 0]) << 0) |
                                 (static_cast<uint8_t>(data[big ? 0 : 1]) << 8));
  };
  auto set_arg = [](jvalue& arg, int group, uint16_t bits) {
    if (group == 0) arg.s = static_cast<jshort>(bits); else arg.c = static_cast<jchar>(bits);
  };
  auto invoke = [&](int target, int group, int kind, jvalue* args) -> uint16_t {
    int sk = signature_kind(kind);
    if (sk == 1) { env->CallStaticVoidMethodA(java_owner, ids[target][group][kind], args); return 0; }
    if (sk == 2) return env->CallStaticBooleanMethodA(java_owner, ids[target][group][kind], args);
    return group == 0
        ? static_cast<uint16_t>(env->CallStaticShortMethodA(java_owner, ids[target][group][kind], args))
        : static_cast<uint16_t>(env->CallStaticCharMethodA(java_owner, ids[target][group][kind], args));
  };
  auto take_exception = [&](jclass expected) {
    jthrowable thrown = env->ExceptionOccurred(); if (thrown) env->ExceptionClear();
    bool correct = thrown && env->IsInstanceOf(thrown, expected);
    if (thrown) env->DeleteLocalRef(thrown); return correct;
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
      int storage_count = target == 0 ? 1 : 2;
      for (int storage = 0; storage < storage_count; ++storage) for (int order = 0; order < 2; ++order) {
        jobject backing = target == 0 ? jobject(bytes) : buffers[storage];
        jobject handle = handles[target][group][order];
        bool big = order == 1;
        for (uint16_t seed : {uint16_t(0), uint16_t(UINT16_MAX),
                              uint16_t(0x8000), uint16_t(0x1234)}) {
          uint16_t update = seed ^ UINT16_C(0xa55a);
          for (int kind = 0; kind < 3; ++kind) {
            if (!write_bits(target, backing, seed, big)) return false;
            jvalue args[5]{}; args[0].l = handle; args[1].l = backing;
            if (invoke(target, group, kind, args) != seed || env->ExceptionCheck()) return false;
            ++operation_groups;
          }
          for (int kind = 3; kind < 6; ++kind) {
            if (!write_bits(target, backing, seed, big)) return false;
            jvalue args[5]{}; args[0].l = handle; args[1].l = backing;
            set_arg(args[3], group, update); invoke(target, group, kind, args);
            if (read_bits(target, backing, big) != update || env->ExceptionCheck()) return false;
            ++operation_groups;
          }
        }
        for (int kind = 6; kind < 24; ++kind) {
          jvalue args[5]{}; args[0].l = handle; args[1].l = backing;
          invoke(target, group, kind, args);
          if (!take_exception(unsupported)) return false;
          ++contract_groups;
        }
        for (int kind = 0; kind < 6; ++kind) for (int bad = 0; bad < 5; ++bad) {
          jvalue args[5]{}; args[0].l = bad == 0 ? nullptr : handle;
          args[1].l = bad == 1 ? nullptr : backing;
          args[2].i = bad == 2 ? -1 : bad == 3 ? 31 : bad == 4 ? 1 : 0;
          invoke(target, group, kind, args);
          if (!take_exception(bad < 2 ? npe : bad < 4 ? bounds : illegal_state)) return false;
          ++contract_groups;
        }
        if (target == 1) {
          jobject ro = env->CallObjectMethod(backing, as_read_only);
          for (int kind = 3; kind < 6; ++kind) {
            jvalue args[5]{}; args[0].l = handle; args[1].l = ro;
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
  env->DeleteLocalRef(bytes); for (jobject buffer : buffers) env->DeleteLocalRef(buffer);
  env->DeleteLocalRef(short_class); env->DeleteLocalRef(char_class);
  env->DeleteLocalRef(buffer_class); env->DeleteLocalRef(bounds);
  env->DeleteLocalRef(illegal_state); env->DeleteLocalRef(unsupported); env->DeleteLocalRef(read_only);
  std::cerr << "ART JIT VarHandle narrow16 byte-array+ByteBuffer ordering: operation-groups="
            << operation_groups << " contract-groups=" << contract_groups << " PASS\n";
  return !env->ExceptionCheck();
}
}  // namespace aim_jni_acceptance_phase
