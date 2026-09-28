#pragma once

namespace aim_jni_acceptance_phase {

inline bool CheckJitVarHandleNarrowOrdering(JNIEnv* env, art::Thread* self,
    art::jit::Jit* jit, art::Handle<art::mirror::Class> owner, jclass java_owner,
    jobject receiver, jclass npe) {
  const char* type_names[] = {"Byte", "Boolean", "Short", "Char"};
  const char descriptors[] = {'B', 'Z', 'S', 'C'};
  const char* suffixes[] = {
      "GetOpaque", "GetAcquire", "GetVolatile", "SetOpaque", "SetRelease", "SetVolatile",
      "WeakPlain", "WeakAcquire", "WeakRelease", "WeakVolatile", "ExchangeAcquire",
      "ExchangeRelease", "SwapAcquire", "SwapRelease", "AddAcquire", "AddRelease", "Or",
      "OrAcquire", "OrRelease", "And", "AndAcquire", "AndRelease", "XorAcquire", "XorRelease"};
  auto signature_kind = [](int kind) {
    return kind < 3 ? 0 : kind < 6 ? 1 : kind < 10 ? 2 : kind < 12 ? 3 : 4;
  };
  auto signature = [&](int group, int shape, int kind) {
    std::string sig = "(Ljava/lang/invoke/VarHandle;";
    if (shape == 0) sig += "Ldev/aim/probe/Hello;";
    else if (shape == 2) { sig += '['; sig += descriptors[group]; sig += 'I'; }
    int values = signature_kind(kind) == 0 ? 0 :
        signature_kind(kind) == 1 || signature_kind(kind) == 4 ? 1 : 2;
    sig.append(values, descriptors[group]);
    sig += ')';
    sig += signature_kind(kind) == 1 ? 'V' :
        signature_kind(kind) == 2 ? 'Z' : descriptors[group];
    return sig;
  };
  auto supported = [](int group, int kind) { return group != 1 || kind < 14 || kind >= 16; };

  jmethodID ids[4][3][24]{};
  art::ArtMethod* methods[4][3][24]{};
  for (int group = 0; group < 4; ++group) for (int shape = 0; shape < 3; ++shape)
    for (int kind = 0; kind < 24; ++kind) {
      std::string name = shape == 1 ? "jitVarStatic" : "jitVar";
      name += type_names[group];
      if (shape == 2) name += "Array";
      name += suffixes[kind];
      std::string sig = signature(group, shape, kind);
      ids[group][shape][kind] = env->GetStaticMethodID(java_owner, name.c_str(), sig.c_str());
      methods[group][shape][kind] =
          owner->FindClassMethod(name.c_str(), sig.c_str(), art::kRuntimePointerSize);
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

  const char* factory_names[] = {
      "jitVarByteHandle", "jitVarBooleanHandle", "jitVarShortHandle", "jitVarCharHandle"};
  const char* field_names[] = {
      "jitVarByte", "jitVarBoolean", "jitVarShort", "jitVarChar"};
  const char* static_field_names[] = {
      "jitVarStaticByte", "jitVarStaticBoolean", "jitVarStaticShort", "jitVarStaticChar"};
  auto array_factory = env->GetStaticMethodID(
      java_owner, "jitVarArrayHandle", "(Ljava/lang/Class;)Ljava/lang/invoke/VarHandle;");
  jobject handles[4][3]{};
  jclass array_classes[4]{};
  jfieldID fields[4][2]{};
  jobject arrays[4]{};
  for (int group = 0; group < 4; ++group) {
    auto factory = env->GetStaticMethodID(
        java_owner, factory_names[group], "(Z)Ljava/lang/invoke/VarHandle;");
    std::string array_name = "[";
    array_name += descriptors[group];
    array_classes[group] = env->FindClass(array_name.c_str());
    handles[group][0] = env->CallStaticObjectMethod(java_owner, factory, JNI_FALSE);
    handles[group][1] = env->CallStaticObjectMethod(java_owner, factory, JNI_TRUE);
    handles[group][2] =
        env->CallStaticObjectMethod(java_owner, array_factory, array_classes[group]);
    std::string desc(1, descriptors[group]);
    fields[group][0] = env->GetFieldID(java_owner, field_names[group], desc.c_str());
    fields[group][1] = env->GetStaticFieldID(java_owner, static_field_names[group], desc.c_str());
  }
  arrays[0] = env->NewByteArray(5);
  arrays[1] = env->NewBooleanArray(5);
  arrays[2] = env->NewShortArray(5);
  arrays[3] = env->NewCharArray(5);
  jclass bounds = env->FindClass("java/lang/ArrayIndexOutOfBoundsException");
  jclass unsupported_class = env->FindClass("java/lang/UnsupportedOperationException");
  if (!array_factory || !bounds || !unsupported_class || env->ExceptionCheck()) return false;
  for (int group = 0; group < 4; ++group) {
    if (!array_classes[group] || !fields[group][0] || !fields[group][1] || !arrays[group])
      return false;
    for (jobject handle : handles[group]) if (!handle) return false;
  }

  constexpr jint kIndex = 2;
  auto set_arg = [](jvalue& arg, int group, uint32_t bits) {
    if (group == 0) arg.b = static_cast<jbyte>(bits);
    else if (group == 1) arg.z = bits ? JNI_TRUE : JNI_FALSE;
    else if (group == 2) arg.s = static_cast<jshort>(bits);
    else arg.c = static_cast<jchar>(bits);
  };
  auto state_set = [&](int group, int shape, uint32_t bits) {
    if (group == 0) {
      jbyte value = static_cast<jbyte>(bits);
      if (shape == 0) env->SetByteField(receiver, fields[group][shape], value);
      else if (shape == 1) env->SetStaticByteField(java_owner, fields[group][shape], value);
      else env->SetByteArrayRegion(static_cast<jbyteArray>(arrays[group]), kIndex, 1, &value);
    } else if (group == 1) {
      jboolean value = bits ? JNI_TRUE : JNI_FALSE;
      if (shape == 0) env->SetBooleanField(receiver, fields[group][shape], value);
      else if (shape == 1) env->SetStaticBooleanField(java_owner, fields[group][shape], value);
      else env->SetBooleanArrayRegion(
          static_cast<jbooleanArray>(arrays[group]), kIndex, 1, &value);
    } else if (group == 2) {
      jshort value = static_cast<jshort>(bits);
      if (shape == 0) env->SetShortField(receiver, fields[group][shape], value);
      else if (shape == 1) env->SetStaticShortField(java_owner, fields[group][shape], value);
      else env->SetShortArrayRegion(static_cast<jshortArray>(arrays[group]), kIndex, 1, &value);
    } else {
      jchar value = static_cast<jchar>(bits);
      if (shape == 0) env->SetCharField(receiver, fields[group][shape], value);
      else if (shape == 1) env->SetStaticCharField(java_owner, fields[group][shape], value);
      else env->SetCharArrayRegion(static_cast<jcharArray>(arrays[group]), kIndex, 1, &value);
    }
  };
  auto state_get = [&](int group, int shape) -> uint32_t {
    if (group == 0) {
      jbyte value = 0;
      if (shape == 0) value = env->GetByteField(receiver, fields[group][shape]);
      else if (shape == 1) value = env->GetStaticByteField(java_owner, fields[group][shape]);
      else env->GetByteArrayRegion(static_cast<jbyteArray>(arrays[group]), kIndex, 1, &value);
      return static_cast<uint8_t>(value);
    }
    if (group == 1) {
      jboolean value = JNI_FALSE;
      if (shape == 0) value = env->GetBooleanField(receiver, fields[group][shape]);
      else if (shape == 1) value = env->GetStaticBooleanField(java_owner, fields[group][shape]);
      else env->GetBooleanArrayRegion(
          static_cast<jbooleanArray>(arrays[group]), kIndex, 1, &value);
      return value == JNI_FALSE ? 0u : 1u;
    }
    if (group == 2) {
      jshort value = 0;
      if (shape == 0) value = env->GetShortField(receiver, fields[group][shape]);
      else if (shape == 1) value = env->GetStaticShortField(java_owner, fields[group][shape]);
      else env->GetShortArrayRegion(static_cast<jshortArray>(arrays[group]), kIndex, 1, &value);
      return static_cast<uint16_t>(value);
    }
    jchar value = 0;
    if (shape == 0) value = env->GetCharField(receiver, fields[group][shape]);
    else if (shape == 1) value = env->GetStaticCharField(java_owner, fields[group][shape]);
    else env->GetCharArrayRegion(static_cast<jcharArray>(arrays[group]), kIndex, 1, &value);
    return value;
  };
  auto valid_args = [&](int group, int shape) {
    std::array<jvalue, 5> args{};
    args[0].l = handles[group][shape];
    if (shape == 0) args[1].l = receiver;
    else if (shape == 2) { args[1].l = arrays[group]; args[2].i = kIndex; }
    return args;
  };
  auto value_slot = [](int shape) { return shape == 0 ? 2 : shape == 1 ? 1 : 3; };
  auto invoke = [&](int group, int shape, int kind, jvalue* args) -> uint32_t {
    int sk = signature_kind(kind);
    if (sk == 1) {
      env->CallStaticVoidMethodA(java_owner, ids[group][shape][kind], args);
      return 0;
    }
    if (sk == 2 || group == 1) return env->CallStaticBooleanMethodA(
        java_owner, ids[group][shape][kind], args) == JNI_FALSE ? 0u : 1u;
    if (group == 0) return static_cast<uint8_t>(
        env->CallStaticByteMethodA(java_owner, ids[group][shape][kind], args));
    if (group == 2) return static_cast<uint16_t>(
        env->CallStaticShortMethodA(java_owner, ids[group][shape][kind], args));
    return env->CallStaticCharMethodA(java_owner, ids[group][shape][kind], args);
  };
  auto take_exception = [&](jclass expected) {
    jthrowable thrown = env->ExceptionOccurred();
    if (thrown) env->ExceptionClear();
    bool correct = thrown && env->IsInstanceOf(thrown, expected);
    if (thrown) env->DeleteLocalRef(thrown);
    return correct;
  };
  const uint32_t values[4][5] = {
      {0u, 1u, UINT8_C(0xff), UINT8_C(0x80), UINT8_C(0x5a)},
      {0u, 1u, 0u, 1u, 0u},
      {0u, 1u, UINT16_C(0xffff), UINT16_C(0x8000), UINT16_C(0x5aa5)},
      {0u, 1u, UINT16_C(0x7fff), UINT16_C(0x8000), UINT16_C(0xffff)}};
  const uint32_t masks[] = {UINT8_MAX, 1u, UINT16_MAX, UINT16_MAX};
  int operation_groups = 0;
  int contract_groups = 0;
  for (int phase = 0; phase < 3; ++phase) {
    if (phase) for (auto& group : methods) for (auto& shape : group) for (auto* method : shape) {
      if (!jit->CompileMethod(method, self,
          phase == 1 ? art::CompilationKind::kBaseline : art::CompilationKind::kOptimized, false) ||
          !jit->GetCodeCache()->ContainsPc(method->GetEntryPointFromQuickCompiledCode())) return false;
    }
    for (int group = 0; group < 4; ++group) for (int shape = 0; shape < 3; ++shape) {
      int slot = value_slot(shape);
      for (uint32_t seed : values[group]) {
        uint32_t update = (seed ^ (group == 1 ? 1u : group == 0 ? UINT8_C(0xa5) :
                                    UINT16_C(0xa55a))) & masks[group];
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
          set_arg(args[slot], group, (seed ^ 1u) & masks[group]);
          set_arg(args[slot + 1], group, update);
          if (invoke(group, shape, kind, args.data()) != 0u || state_get(group, shape) != seed)
            return false;
          ++operation_groups;
          set_arg(args[slot], group, seed);
          bool success = false;
          for (int retry = 0; retry < 100 && !success; ++retry)
            success = invoke(group, shape, kind, args.data()) != 0u;
          if (!success || state_get(group, shape) != update || env->ExceptionCheck()) return false;
          ++operation_groups;
        }
        for (int kind = 10; kind < 12; ++kind) {
          state_set(group, shape, seed);
          auto args = valid_args(group, shape);
          set_arg(args[slot], group, (seed ^ 1u) & masks[group]);
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
          if (invoke(group, shape, kind, args.data()) != seed || state_get(group, shape) != update ||
              env->ExceptionCheck()) return false;
          ++operation_groups;
        }
        if (group != 1) for (int kind = 14; kind < 16; ++kind) {
          constexpr uint32_t kDelta = UINT16_C(0x1234);
          uint32_t delta = group == 0 ? UINT8_C(0x25) : kDelta;
          state_set(group, shape, seed);
          auto args = valid_args(group, shape); set_arg(args[slot], group, delta);
          if (invoke(group, shape, kind, args.data()) != seed ||
              state_get(group, shape) != ((seed + delta) & masks[group]) ||
              env->ExceptionCheck()) return false;
          ++operation_groups;
        }
        for (int kind = 16; kind < 24; ++kind) {
          uint32_t mask = group == 1 ? 1u : group == 0 ? UINT8_C(0x3c) : UINT16_C(0x3cc3);
          uint32_t expected = kind < 19 ? seed | mask : kind < 22 ? seed & mask : seed ^ mask;
          expected &= masks[group];
          state_set(group, shape, seed);
          auto args = valid_args(group, shape); set_arg(args[slot], group, mask);
          if (invoke(group, shape, kind, args.data()) != seed ||
              state_get(group, shape) != expected || env->ExceptionCheck()) return false;
          ++operation_groups;
        }
      }
      for (int kind = 0; kind < 24; ++kind) {
        if (!supported(group, kind)) {
          auto args = valid_args(group, shape);
          invoke(group, shape, kind, args.data());
          if (!take_exception(unsupported_class)) return false;
          ++contract_groups;
          continue;
        }
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
  env->DeleteLocalRef(bounds); env->DeleteLocalRef(unsupported_class);
  std::cerr << "ART JIT VarHandle narrow field/static/array ordering: operation-groups="
            << operation_groups << " contract-groups=" << contract_groups << " PASS\n";
  return !env->ExceptionCheck();
}

}  // namespace aim_jni_acceptance_phase
