#pragma once
namespace aim_jni_acceptance_phase {
inline bool CheckJitVarHandleOrderingModes(JNIEnv* env, art::Thread* self, art::jit::Jit* jit,
    art::Handle<art::mirror::Class> owner, jclass java_owner, jobject receiver,
    jobject handle, jfieldID field, jclass npe) {
  const char* names[] = {
      "jitVarIntGetOpaque", "jitVarIntGetAcquire", "jitVarIntGetVolatile",
      "jitVarIntSetOpaque", "jitVarIntSetRelease", "jitVarIntSetVolatile",
      "jitVarIntWeakPlain", "jitVarIntWeakAcquire", "jitVarIntWeakRelease",
      "jitVarIntWeakVolatile", "jitVarIntExchangeAcquire", "jitVarIntExchangeRelease",
      "jitVarIntSwapAcquire", "jitVarIntSwapRelease", "jitVarIntAddAcquire",
      "jitVarIntAddRelease", "jitVarIntOr", "jitVarIntOrAcquire", "jitVarIntOrRelease",
      "jitVarIntAnd", "jitVarIntAndAcquire", "jitVarIntAndRelease",
      "jitVarIntXorAcquire", "jitVarIntXorRelease"};
  constexpr const char* kGetSig =
      "(Ljava/lang/invoke/VarHandle;Ldev/aim/probe/Hello;)I";
  constexpr const char* kSetSig =
      "(Ljava/lang/invoke/VarHandle;Ldev/aim/probe/Hello;I)V";
  constexpr const char* kCasSig =
      "(Ljava/lang/invoke/VarHandle;Ldev/aim/probe/Hello;II)Z";
  constexpr const char* kExchangeSig =
      "(Ljava/lang/invoke/VarHandle;Ldev/aim/probe/Hello;II)I";
  constexpr const char* kUpdateSig =
      "(Ljava/lang/invoke/VarHandle;Ldev/aim/probe/Hello;I)I";
  auto signature = [&](int kind) {
    if (kind < 3) return kGetSig;
    if (kind < 6) return kSetSig;
    if (kind < 10) return kCasSig;
    if (kind < 12) return kExchangeSig;
    return kUpdateSig;
  };
  jmethodID ids[24]{};
  art::ArtMethod* methods[24]{};
  for (int kind = 0; kind < 24; ++kind) {
    const char* sig = signature(kind);
    ids[kind] = env->GetStaticMethodID(java_owner, names[kind], sig);
    methods[kind] = owner->FindClassMethod(names[kind], sig, art::kRuntimePointerSize);
    if (!ids[kind] || !methods[kind] || env->ExceptionCheck()) return false;
    art::CodeItemDataAccessor code(*methods[kind]->GetDexFile(), methods[kind]->GetCodeItem());
    bool polymorphic = false;
    for (const auto& pair : code) {
      polymorphic |= pair.Inst().Opcode() == art::Instruction::INVOKE_POLYMORPHIC ||
          pair.Inst().Opcode() == art::Instruction::INVOKE_POLYMORPHIC_RANGE;
    }
    if (!polymorphic) return false;
  }
  auto state_is = [&](uint32_t expected) {
    return !env->ExceptionCheck() &&
        static_cast<uint32_t>(env->GetIntField(receiver, field)) == expected;
  };
  int operation_groups = 0;
  int null_failures = 0;
  for (int phase = 0; phase < 3; ++phase) {
    if (phase) for (auto* method : methods) {
      if (!jit->CompileMethod(method, self,
          phase == 1 ? art::CompilationKind::kBaseline : art::CompilationKind::kOptimized, false) ||
          !jit->GetCodeCache()->ContainsPc(method->GetEntryPointFromQuickCompiledCode())) {
        std::cerr << "ART JIT VarHandle ordering compile failed phase=" << phase << "\n";
        return false;
      }
    }
    for (jint seed : {jint(0), jint(-1), INT32_MIN, INT32_MAX, jint(0x12345678)}) {
      jint update = seed ^ jint(0x5a5aa5a5);
      for (int kind = 0; kind < 3; ++kind) {
        env->SetIntField(receiver, field, seed);
        if (env->CallStaticIntMethod(java_owner, ids[kind], handle, receiver) != seed ||
            !state_is(static_cast<uint32_t>(seed))) return false;
        ++operation_groups;
      }
      for (int kind = 3; kind < 6; ++kind) {
        env->SetIntField(receiver, field, seed);
        env->CallStaticVoidMethod(java_owner, ids[kind], handle, receiver, update);
        if (!state_is(static_cast<uint32_t>(update))) return false;
        ++operation_groups;
      }
      for (int kind = 6; kind < 10; ++kind) {
        env->SetIntField(receiver, field, seed);
        if (env->CallStaticBooleanMethod(
                java_owner, ids[kind], handle, receiver, seed ^ 1, update) != JNI_FALSE ||
            !state_is(static_cast<uint32_t>(seed))) return false;
        ++operation_groups;
        bool success = false;
        for (int retry = 0; retry < 100 && !success; ++retry) {
          success = env->CallStaticBooleanMethod(
              java_owner, ids[kind], handle, receiver, seed, update) == JNI_TRUE;
          if (env->ExceptionCheck()) return false;
        }
        if (!success || !state_is(static_cast<uint32_t>(update))) return false;
        ++operation_groups;
      }
      for (int kind = 10; kind < 12; ++kind) {
        env->SetIntField(receiver, field, seed);
        if (env->CallStaticIntMethod(
                java_owner, ids[kind], handle, receiver, seed ^ 1, update) != seed ||
            !state_is(static_cast<uint32_t>(seed))) return false;
        ++operation_groups;
        if (env->CallStaticIntMethod(
                java_owner, ids[kind], handle, receiver, seed, update) != seed ||
            !state_is(static_cast<uint32_t>(update))) return false;
        ++operation_groups;
      }
      for (int kind = 12; kind < 14; ++kind) {
        env->SetIntField(receiver, field, seed);
        if (env->CallStaticIntMethod(java_owner, ids[kind], handle, receiver, update) != seed ||
            !state_is(static_cast<uint32_t>(update))) return false;
        ++operation_groups;
      }
      constexpr uint32_t kDelta = UINT32_C(0x13579bdf);
      for (int kind = 14; kind < 16; ++kind) {
        env->SetIntField(receiver, field, seed);
        if (env->CallStaticIntMethod(
                java_owner, ids[kind], handle, receiver, static_cast<jint>(kDelta)) != seed ||
            !state_is(static_cast<uint32_t>(seed) + kDelta)) return false;
        ++operation_groups;
      }
      for (int kind = 16; kind < 24; ++kind) {
        env->SetIntField(receiver, field, seed);
        constexpr uint32_t kMask = UINT32_C(0x0f0ff0f0);
        uint32_t expected = kind < 19 ? static_cast<uint32_t>(seed) | kMask
            : kind < 22 ? static_cast<uint32_t>(seed) & kMask
                        : static_cast<uint32_t>(seed) ^ kMask;
        if (env->CallStaticIntMethod(
                java_owner, ids[kind], handle, receiver, static_cast<jint>(kMask)) != seed ||
            !state_is(expected)) return false;
        ++operation_groups;
      }
    }
    for (int kind = 0; kind < 24; ++kind) for (int null_case = 0; null_case < 2; ++null_case) {
      jvalue args[4]{};
      args[0].l = null_case == 0 ? nullptr : handle;
      args[1].l = null_case == 1 ? nullptr : receiver;
      if (kind < 3 || kind >= 10) {
        env->CallStaticIntMethodA(java_owner, ids[kind], args);
      } else if (kind < 6) {
        env->CallStaticVoidMethodA(java_owner, ids[kind], args);
      } else {
        env->CallStaticBooleanMethodA(java_owner, ids[kind], args);
      }
      jthrowable thrown = env->ExceptionOccurred();
      if (thrown) env->ExceptionClear();
      bool correct = thrown && env->IsInstanceOf(thrown, npe);
      if (thrown) env->DeleteLocalRef(thrown);
      if (!correct) return false;
      ++null_failures;
    }
  }
  std::cerr << "ART JIT VarHandle int ordering modes: operation-groups=" << operation_groups
            << " null-failures=" << null_failures << " PASS\n";
  return !env->ExceptionCheck();
}
}  // namespace aim_jni_acceptance_phase
