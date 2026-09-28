#pragma once

namespace aim_jni_acceptance_phase {

inline bool CheckJitVarHandleFpOrdering(JNIEnv* env, art::Thread* self, art::jit::Jit* jit,
    art::Handle<art::mirror::Class> owner, jclass java_owner, jobject receiver, jclass npe) {
  const char* suffixes[] = {
      "GetOpaque", "GetAcquire", "GetVolatile", "SetOpaque", "SetRelease", "SetVolatile",
      "WeakPlain", "WeakAcquire", "WeakRelease", "WeakVolatile", "ExchangeAcquire",
      "ExchangeRelease", "SwapAcquire", "SwapRelease", "AddAcquire", "AddRelease", "Or",
      "OrAcquire", "OrRelease", "And", "AndAcquire", "AndRelease", "XorAcquire", "XorRelease"};
  auto signature_kind = [](int kind) {
    return kind < 3 ? 0 : kind < 6 ? 1 : kind < 10 ? 2 : kind < 12 ? 3 : 4;
  };
  constexpr const char* kFloatInstanceSigs[] = {
      "(Ljava/lang/invoke/VarHandle;Ldev/aim/probe/Hello;)F",
      "(Ljava/lang/invoke/VarHandle;Ldev/aim/probe/Hello;F)V",
      "(Ljava/lang/invoke/VarHandle;Ldev/aim/probe/Hello;FF)Z",
      "(Ljava/lang/invoke/VarHandle;Ldev/aim/probe/Hello;FF)F",
      "(Ljava/lang/invoke/VarHandle;Ldev/aim/probe/Hello;F)F"};
  constexpr const char* kDoubleInstanceSigs[] = {
      "(Ljava/lang/invoke/VarHandle;Ldev/aim/probe/Hello;)D",
      "(Ljava/lang/invoke/VarHandle;Ldev/aim/probe/Hello;D)V",
      "(Ljava/lang/invoke/VarHandle;Ldev/aim/probe/Hello;DD)Z",
      "(Ljava/lang/invoke/VarHandle;Ldev/aim/probe/Hello;DD)D",
      "(Ljava/lang/invoke/VarHandle;Ldev/aim/probe/Hello;D)D"};
  constexpr const char* kFloatStaticSigs[] = {
      "(Ljava/lang/invoke/VarHandle;)F", "(Ljava/lang/invoke/VarHandle;F)V",
      "(Ljava/lang/invoke/VarHandle;FF)Z", "(Ljava/lang/invoke/VarHandle;FF)F",
      "(Ljava/lang/invoke/VarHandle;F)F"};
  constexpr const char* kDoubleStaticSigs[] = {
      "(Ljava/lang/invoke/VarHandle;)D", "(Ljava/lang/invoke/VarHandle;D)V",
      "(Ljava/lang/invoke/VarHandle;DD)Z", "(Ljava/lang/invoke/VarHandle;DD)D",
      "(Ljava/lang/invoke/VarHandle;D)D"};
  constexpr const char* kFloatArraySigs[] = {
      "(Ljava/lang/invoke/VarHandle;[FI)F", "(Ljava/lang/invoke/VarHandle;[FIF)V",
      "(Ljava/lang/invoke/VarHandle;[FIFF)Z", "(Ljava/lang/invoke/VarHandle;[FIFF)F",
      "(Ljava/lang/invoke/VarHandle;[FIF)F"};
  constexpr const char* kDoubleArraySigs[] = {
      "(Ljava/lang/invoke/VarHandle;[DI)D", "(Ljava/lang/invoke/VarHandle;[DID)V",
      "(Ljava/lang/invoke/VarHandle;[DIDD)Z", "(Ljava/lang/invoke/VarHandle;[DIDD)D",
      "(Ljava/lang/invoke/VarHandle;[DID)D"};
  auto signature = [&](int group, int shape, int kind) {
    int sk = signature_kind(kind);
    if (group == 0) return shape == 0 ? kFloatInstanceSigs[sk] :
        shape == 1 ? kFloatStaticSigs[sk] : kFloatArraySigs[sk];
    return shape == 0 ? kDoubleInstanceSigs[sk] :
        shape == 1 ? kDoubleStaticSigs[sk] : kDoubleArraySigs[sk];
  };

  jmethodID ids[2][3][24]{};
  art::ArtMethod* methods[2][3][24]{};
  for (int group = 0; group < 2; ++group) for (int shape = 0; shape < 3; ++shape)
    for (int kind = 0; kind < 24; ++kind) {
      std::string name = shape == 1 ? "jitVarStatic" : "jitVar";
      name += group == 0 ? "Float" : "Double";
      if (shape == 2) name += "Array";
      name += suffixes[kind];
      const char* sig = signature(group, shape, kind);
      ids[group][shape][kind] = env->GetStaticMethodID(java_owner, name.c_str(), sig);
      methods[group][shape][kind] =
          owner->FindClassMethod(name.c_str(), sig, art::kRuntimePointerSize);
      if (!ids[group][shape][kind] || !methods[group][shape][kind] || env->ExceptionCheck())
        return false;
      art::CodeItemDataAccessor code(*methods[group][shape][kind]->GetDexFile(),
                                     methods[group][shape][kind]->GetCodeItem());
      bool polymorphic = false;
      for (const auto& pair : code) polymorphic |=
          pair.Inst().Opcode() == art::Instruction::INVOKE_POLYMORPHIC ||
          pair.Inst().Opcode() == art::Instruction::INVOKE_POLYMORPHIC_RANGE;
      if (!polymorphic) return false;
    }

  auto float_factory = env->GetStaticMethodID(
      java_owner, "jitVarFloatHandle", "(Z)Ljava/lang/invoke/VarHandle;");
  auto double_factory = env->GetStaticMethodID(
      java_owner, "jitVarDoubleHandle", "(Z)Ljava/lang/invoke/VarHandle;");
  auto array_factory = env->GetStaticMethodID(
      java_owner, "jitVarArrayHandle", "(Ljava/lang/Class;)Ljava/lang/invoke/VarHandle;");
  jclass array_classes[2] = {env->FindClass("[F"), env->FindClass("[D")};
  jobject handles[2][3]{};
  for (int group = 0; group < 2; ++group) {
    auto factory = group == 0 ? float_factory : double_factory;
    handles[group][0] = env->CallStaticObjectMethod(java_owner, factory, JNI_FALSE);
    handles[group][1] = env->CallStaticObjectMethod(java_owner, factory, JNI_TRUE);
    handles[group][2] =
        env->CallStaticObjectMethod(java_owner, array_factory, array_classes[group]);
  }
  jfieldID fields[2][2] = {
      {env->GetFieldID(java_owner, "jitVarFloat", "F"),
       env->GetStaticFieldID(java_owner, "jitVarStaticFloat", "F")},
      {env->GetFieldID(java_owner, "jitVarDouble", "D"),
       env->GetStaticFieldID(java_owner, "jitVarStaticDouble", "D")}};
  jobject arrays[2] = {env->NewFloatArray(5), env->NewDoubleArray(5)};
  jclass bounds = env->FindClass("java/lang/ArrayIndexOutOfBoundsException");
  jclass unsupported = env->FindClass("java/lang/UnsupportedOperationException");
  if (!float_factory || !double_factory || !array_factory || !array_classes[0] ||
      !array_classes[1] || !fields[0][0] || !fields[0][1] || !fields[1][0] ||
      !fields[1][1] || !arrays[0] || !arrays[1] || !bounds || !unsupported ||
      env->ExceptionCheck()) return false;
  for (const auto& group : handles) for (jobject handle : group) if (!handle) return false;

  constexpr jint kIndex = 2;
  auto set_arg = [](jvalue& arg, int group, uint64_t bits) {
    if (group == 0) arg.f = JitVarHandleFloatFromBits(static_cast<uint32_t>(bits));
    else arg.d = JitVarHandleDoubleFromBits(bits);
  };
  auto state_set = [&](int group, int shape, uint64_t bits) {
    if (group == 0) {
      jfloat value = JitVarHandleFloatFromBits(static_cast<uint32_t>(bits));
      if (shape == 0) env->SetFloatField(receiver, fields[group][shape], value);
      else if (shape == 1) env->SetStaticFloatField(java_owner, fields[group][shape], value);
      else env->SetFloatArrayRegion(static_cast<jfloatArray>(arrays[group]), kIndex, 1, &value);
    } else {
      jdouble value = JitVarHandleDoubleFromBits(bits);
      if (shape == 0) env->SetDoubleField(receiver, fields[group][shape], value);
      else if (shape == 1) env->SetStaticDoubleField(java_owner, fields[group][shape], value);
      else env->SetDoubleArrayRegion(static_cast<jdoubleArray>(arrays[group]), kIndex, 1, &value);
    }
  };
  auto state_get = [&](int group, int shape) -> uint64_t {
    if (group == 0) {
      jfloat value = 0.0f;
      if (shape == 0) value = env->GetFloatField(receiver, fields[group][shape]);
      else if (shape == 1) value = env->GetStaticFloatField(java_owner, fields[group][shape]);
      else env->GetFloatArrayRegion(static_cast<jfloatArray>(arrays[group]), kIndex, 1, &value);
      return JitVarHandleFloatBits(value);
    }
    jdouble value = 0.0;
    if (shape == 0) value = env->GetDoubleField(receiver, fields[group][shape]);
    else if (shape == 1) value = env->GetStaticDoubleField(java_owner, fields[group][shape]);
    else env->GetDoubleArrayRegion(static_cast<jdoubleArray>(arrays[group]), kIndex, 1, &value);
    return JitVarHandleDoubleBits(value);
  };
  auto valid_args = [&](int group, int shape) {
    std::array<jvalue, 5> args{};
    args[0].l = handles[group][shape];
    if (shape == 0) args[1].l = receiver;
    else if (shape == 2) { args[1].l = arrays[group]; args[2].i = kIndex; }
    return args;
  };
  auto value_slot = [](int shape) { return shape == 0 ? 2 : shape == 1 ? 1 : 3; };
  auto invoke = [&](int group, int shape, int kind, jvalue* args) -> uint64_t {
    int sk = signature_kind(kind);
    if (sk == 1) {
      env->CallStaticVoidMethodA(java_owner, ids[group][shape][kind], args);
      return 0;
    }
    if (sk == 2) return env->CallStaticBooleanMethodA(
        java_owner, ids[group][shape][kind], args);
    return group == 0 ? JitVarHandleFloatBits(env->CallStaticFloatMethodA(
        java_owner, ids[group][shape][kind], args)) :
        JitVarHandleDoubleBits(env->CallStaticDoubleMethodA(
            java_owner, ids[group][shape][kind], args));
  };
  auto take_exception = [&](jclass expected) {
    jthrowable thrown = env->ExceptionOccurred();
    if (thrown) env->ExceptionClear();
    bool correct = thrown && env->IsInstanceOf(thrown, expected);
    if (thrown) env->DeleteLocalRef(thrown);
    return correct;
  };
  const uint64_t values[2][5] = {
      {UINT32_C(0), UINT32_C(0x80000000), UINT32_C(0x7f800000),
       UINT32_C(0xc2f78000), UINT32_C(0x7fc12345)},
      {UINT64_C(0), UINT64_C(0x8000000000000000), UINT64_C(0x7ff0000000000000),
       UINT64_C(0xc05ef00000000000), UINT64_C(0x7ff8123456789abc)}};
  const uint64_t finite_values[2][4] = {
      {UINT32_C(0), UINT32_C(0x80000000), UINT32_C(0x3fc00000), UINT32_C(0xc2f78000)},
      {UINT64_C(0), UINT64_C(0x8000000000000000), UINT64_C(0x3ff8000000000000),
       UINT64_C(0xc05ef00000000000)}};
  int operation_groups = 0;
  int contract_groups = 0;
  for (int phase = 0; phase < 3; ++phase) {
    if (phase) for (auto& group : methods) for (auto& shape : group) for (auto* method : shape) {
      if (!jit->CompileMethod(method, self,
          phase == 1 ? art::CompilationKind::kBaseline : art::CompilationKind::kOptimized, false) ||
          !jit->GetCodeCache()->ContainsPc(method->GetEntryPointFromQuickCompiledCode())) return false;
    }
    for (int group = 0; group < 2; ++group) for (int shape = 0; shape < 3; ++shape) {
      uint64_t width_mask = group == 0 ? UINT32_MAX : UINT64_MAX;
      int slot = value_slot(shape);
      for (uint64_t seed : values[group]) {
        uint64_t update = (seed ^ (group == 0 ? UINT64_C(0x80000000) :
                                                UINT64_C(0x8000000000000000))) & width_mask;
        for (int kind = 0; kind < 3; ++kind) {
          state_set(group, shape, seed);
          auto args = valid_args(group, shape);
          if (invoke(group, shape, kind, args.data()) != seed || state_get(group, shape) != seed ||
              env->ExceptionCheck()) return false;
          ++operation_groups;
        }
        for (int kind = 3; kind < 6; ++kind) {
          state_set(group, shape, seed);
          auto args = valid_args(group, shape); set_arg(args[slot], group, update);
          invoke(group, shape, kind, args.data());
          if (state_get(group, shape) != update || env->ExceptionCheck()) return false;
          ++operation_groups;
        }
        for (int kind = 6; kind < 10; ++kind) {
          state_set(group, shape, seed);
          auto args = valid_args(group, shape);
          set_arg(args[slot], group, (seed ^ 1u) & width_mask);
          set_arg(args[slot + 1], group, update);
          if (invoke(group, shape, kind, args.data()) != JNI_FALSE ||
              state_get(group, shape) != seed) return false;
          ++operation_groups;
          set_arg(args[slot], group, seed);
          bool success = false;
          for (int retry = 0; retry < 100 && !success; ++retry)
            success = invoke(group, shape, kind, args.data()) == JNI_TRUE;
          if (!success || state_get(group, shape) != update || env->ExceptionCheck()) return false;
          ++operation_groups;
        }
        for (int kind = 10; kind < 12; ++kind) {
          state_set(group, shape, seed);
          auto args = valid_args(group, shape);
          set_arg(args[slot], group, (seed ^ 1u) & width_mask);
          set_arg(args[slot + 1], group, update);
          if (invoke(group, shape, kind, args.data()) != seed || state_get(group, shape) != seed)
            return false;
          ++operation_groups;
          set_arg(args[slot], group, seed);
          if (invoke(group, shape, kind, args.data()) != seed || state_get(group, shape) != update)
            return false;
          ++operation_groups;
        }
        for (int kind = 12; kind < 14; ++kind) {
          state_set(group, shape, seed);
          auto args = valid_args(group, shape); set_arg(args[slot], group, update);
          if (invoke(group, shape, kind, args.data()) != seed ||
              state_get(group, shape) != update || env->ExceptionCheck()) return false;
          ++operation_groups;
        }
      }
      for (uint64_t seed : finite_values[group]) for (int kind = 14; kind < 16; ++kind) {
        state_set(group, shape, seed);
        auto args = valid_args(group, shape);
        uint64_t expected;
        if (group == 0) {
          constexpr jfloat delta = 1.25f;
          set_arg(args[slot], group, JitVarHandleFloatBits(delta));
          expected = JitVarHandleFloatBits(JitVarHandleFloatFromBits(
              static_cast<uint32_t>(seed)) + delta);
        } else {
          constexpr jdouble delta = 1.25;
          set_arg(args[slot], group, JitVarHandleDoubleBits(delta));
          expected = JitVarHandleDoubleBits(JitVarHandleDoubleFromBits(seed) + delta);
        }
        if (invoke(group, shape, kind, args.data()) != seed ||
            state_get(group, shape) != expected || env->ExceptionCheck()) return false;
        ++operation_groups;
      }
      for (int kind = 16; kind < 24; ++kind) {
        auto args = valid_args(group, shape);
        invoke(group, shape, kind, args.data());
        if (!take_exception(unsupported)) return false;
        ++contract_groups;
      }
      for (int kind = 0; kind < 16; ++kind) {
        int cases = shape == 0 ? 2 : shape == 1 ? 1 : 4;
        for (int bad = 0; bad < cases; ++bad) {
          auto args = valid_args(group, shape);
          args[0].l = bad == 0 ? nullptr : handles[group][shape];
          if (shape == 0) args[1].l = bad == 1 ? nullptr : receiver;
          if (shape == 2) {
            args[1].l = bad == 1 ? nullptr : arrays[group];
            args[2].i = bad == 2 ? -1 : bad == 3 ? 5 : kIndex;
          }
          invoke(group, shape, kind, args.data());
          if (!take_exception(shape == 2 && bad >= 2 ? bounds : npe)) return false;
          ++contract_groups;
        }
      }
    }
  }
  for (auto& group : handles) for (jobject handle : group) env->DeleteLocalRef(handle);
  for (jobject array : arrays) env->DeleteLocalRef(array);
  for (jclass array_class : array_classes) env->DeleteLocalRef(array_class);
  env->DeleteLocalRef(bounds); env->DeleteLocalRef(unsupported);
  std::cerr << "ART JIT VarHandle FP field/static/array ordering: operation-groups="
            << operation_groups << " contract-groups=" << contract_groups << " PASS\n";
  return !env->ExceptionCheck();
}

}  // namespace aim_jni_acceptance_phase
