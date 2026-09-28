#pragma once
namespace aim_jni_acceptance_phase {
inline bool CheckJitVarHandleOrderingShape(JNIEnv* env, art::Thread* self, art::jit::Jit* jit,
    art::Handle<art::mirror::Class> owner, jclass java_owner, jclass npe, bool array_shape) {
  const char* suffixes[] = {
      "GetOpaque", "GetAcquire", "GetVolatile", "SetOpaque", "SetRelease", "SetVolatile",
      "WeakPlain", "WeakAcquire", "WeakRelease", "WeakVolatile", "ExchangeAcquire",
      "ExchangeRelease", "SwapAcquire", "SwapRelease", "AddAcquire", "AddRelease", "Or",
      "OrAcquire", "OrRelease", "And", "AndAcquire", "AndRelease", "XorAcquire", "XorRelease"};
  constexpr const char* kStaticSigs[] = {
      "(Ljava/lang/invoke/VarHandle;)I", "(Ljava/lang/invoke/VarHandle;I)V",
      "(Ljava/lang/invoke/VarHandle;II)Z", "(Ljava/lang/invoke/VarHandle;II)I",
      "(Ljava/lang/invoke/VarHandle;I)I"};
  constexpr const char* kArraySigs[] = {
      "(Ljava/lang/invoke/VarHandle;[II)I", "(Ljava/lang/invoke/VarHandle;[III)V",
      "(Ljava/lang/invoke/VarHandle;[IIII)Z", "(Ljava/lang/invoke/VarHandle;[IIII)I",
      "(Ljava/lang/invoke/VarHandle;[III)I"};
  auto signature_kind = [](int kind) {
    return kind < 3 ? 0 : kind < 6 ? 1 : kind < 10 ? 2 : kind < 12 ? 3 : 4;
  };
  jmethodID ids[24]{};
  art::ArtMethod* methods[24]{};
  for (int kind = 0; kind < 24; ++kind) {
    std::string name = array_shape ? "jitVarIntArray" : "jitVarStaticInt";
    name += suffixes[kind];
    const char* sig = array_shape ? kArraySigs[signature_kind(kind)]
                                  : kStaticSigs[signature_kind(kind)];
    ids[kind] = env->GetStaticMethodID(java_owner, name.c_str(), sig);
    methods[kind] = owner->FindClassMethod(name.c_str(), sig, art::kRuntimePointerSize);
    if (!ids[kind] || !methods[kind] || env->ExceptionCheck()) return false;
    art::CodeItemDataAccessor code(*methods[kind]->GetDexFile(), methods[kind]->GetCodeItem());
    bool polymorphic = false;
    for (const auto& pair : code) {
      polymorphic |= pair.Inst().Opcode() == art::Instruction::INVOKE_POLYMORPHIC ||
          pair.Inst().Opcode() == art::Instruction::INVOKE_POLYMORPHIC_RANGE;
    }
    if (!polymorphic) return false;
  }
  jobject handle = nullptr;
  jintArray array = nullptr;
  jfieldID static_field = nullptr;
  jclass bounds = array_shape ? env->FindClass("java/lang/ArrayIndexOutOfBoundsException") : nullptr;
  if (array_shape) {
    jclass int_array_class = env->FindClass("[I");
    auto factory = env->GetStaticMethodID(
        java_owner, "jitVarArrayHandle", "(Ljava/lang/Class;)Ljava/lang/invoke/VarHandle;");
    array = env->NewIntArray(5);
    if (int_array_class && factory && array) {
      handle = env->CallStaticObjectMethod(java_owner, factory, int_array_class);
    }
    if (int_array_class) env->DeleteLocalRef(int_array_class);
  } else {
    auto factory = env->GetStaticMethodID(
        java_owner, "jitVarStaticHandle", "()Ljava/lang/invoke/VarHandle;");
    static_field = env->GetStaticFieldID(java_owner, "jitVarStaticValue", "I");
    if (factory && static_field) handle = env->CallStaticObjectMethod(java_owner, factory);
  }
  if (!handle || (array_shape && !bounds) || env->ExceptionCheck()) return false;
  constexpr jint kIndex = 2;
  auto set_state = [&](jint value) {
    if (array_shape) env->SetIntArrayRegion(array, kIndex, 1, &value);
    else env->SetStaticIntField(java_owner, static_field, value);
  };
  auto get_state = [&]() {
    jint value = 0;
    if (array_shape) env->GetIntArrayRegion(array, kIndex, 1, &value);
    else value = env->GetStaticIntField(java_owner, static_field);
    return value;
  };
  auto call_get = [&](int kind) {
    return array_shape ? env->CallStaticIntMethod(java_owner, ids[kind], handle, array, kIndex)
                       : env->CallStaticIntMethod(java_owner, ids[kind], handle);
  };
  auto call_set = [&](int kind, jint value) {
    if (array_shape) env->CallStaticVoidMethod(java_owner, ids[kind], handle, array, kIndex, value);
    else env->CallStaticVoidMethod(java_owner, ids[kind], handle, value);
  };
  auto call_cas = [&](int kind, jint expected, jint value) {
    return array_shape
        ? env->CallStaticBooleanMethod(java_owner, ids[kind], handle, array, kIndex, expected, value)
        : env->CallStaticBooleanMethod(java_owner, ids[kind], handle, expected, value);
  };
  auto call_exchange = [&](int kind, jint expected, jint value) {
    return array_shape
        ? env->CallStaticIntMethod(java_owner, ids[kind], handle, array, kIndex, expected, value)
        : env->CallStaticIntMethod(java_owner, ids[kind], handle, expected, value);
  };
  auto call_update = [&](int kind, jint value) {
    return array_shape
        ? env->CallStaticIntMethod(java_owner, ids[kind], handle, array, kIndex, value)
        : env->CallStaticIntMethod(java_owner, ids[kind], handle, value);
  };
  int operation_groups = 0;
  int exception_groups = 0;
  for (int phase = 0; phase < 3; ++phase) {
    if (phase) for (auto* method : methods) {
      if (!jit->CompileMethod(method, self,
          phase == 1 ? art::CompilationKind::kBaseline : art::CompilationKind::kOptimized, false) ||
          !jit->GetCodeCache()->ContainsPc(method->GetEntryPointFromQuickCompiledCode())) return false;
    }
    for (jint seed : {jint(0), jint(-1), INT32_MIN, INT32_MAX, jint(0x12345678)}) {
      jint update = seed ^ jint(0x5a5aa5a5);
      for (int kind = 0; kind < 3; ++kind) {
        set_state(seed);
        if (call_get(kind) != seed || get_state() != seed || env->ExceptionCheck()) return false;
        ++operation_groups;
      }
      for (int kind = 3; kind < 6; ++kind) {
        set_state(seed); call_set(kind, update);
        if (get_state() != update || env->ExceptionCheck()) return false;
        ++operation_groups;
      }
      for (int kind = 6; kind < 10; ++kind) {
        set_state(seed);
        if (call_cas(kind, seed ^ 1, update) != JNI_FALSE || get_state() != seed) return false;
        ++operation_groups;
        bool success = false;
        for (int retry = 0; retry < 100 && !success; ++retry) {
          success = call_cas(kind, seed, update) == JNI_TRUE;
          if (env->ExceptionCheck()) return false;
        }
        if (!success || get_state() != update) return false;
        ++operation_groups;
      }
      for (int kind = 10; kind < 12; ++kind) {
        set_state(seed);
        if (call_exchange(kind, seed ^ 1, update) != seed || get_state() != seed) return false;
        ++operation_groups;
        if (call_exchange(kind, seed, update) != seed || get_state() != update) return false;
        ++operation_groups;
      }
      for (int kind = 12; kind < 14; ++kind) {
        set_state(seed);
        if (call_update(kind, update) != seed || get_state() != update) return false;
        ++operation_groups;
      }
      constexpr uint32_t kDelta = UINT32_C(0x13579bdf);
      for (int kind = 14; kind < 16; ++kind) {
        set_state(seed);
        if (call_update(kind, static_cast<jint>(kDelta)) != seed ||
            static_cast<uint32_t>(get_state()) != static_cast<uint32_t>(seed) + kDelta) return false;
        ++operation_groups;
      }
      constexpr uint32_t kMask = UINT32_C(0x0f0ff0f0);
      for (int kind = 16; kind < 24; ++kind) {
        set_state(seed);
        uint32_t expected = kind < 19 ? static_cast<uint32_t>(seed) | kMask
            : kind < 22 ? static_cast<uint32_t>(seed) & kMask
                        : static_cast<uint32_t>(seed) ^ kMask;
        if (call_update(kind, static_cast<jint>(kMask)) != seed ||
            static_cast<uint32_t>(get_state()) != expected) return false;
        ++operation_groups;
      }
    }
    for (int kind = 0; kind < 24; ++kind) {
      int cases = array_shape ? 4 : 1;
      for (int bad = 0; bad < cases; ++bad) {
        jvalue args[5]{};
        args[0].l = bad == 0 ? nullptr : handle;
        if (array_shape) {
          args[1].l = bad == 1 ? nullptr : array;
          args[2].i = bad == 2 ? -1 : bad == 3 ? 5 : kIndex;
        }
        int sig_kind = signature_kind(kind);
        if (sig_kind == 1) env->CallStaticVoidMethodA(java_owner, ids[kind], args);
        else if (sig_kind == 2) env->CallStaticBooleanMethodA(java_owner, ids[kind], args);
        else env->CallStaticIntMethodA(java_owner, ids[kind], args);
        jthrowable thrown = env->ExceptionOccurred();
        if (thrown) env->ExceptionClear();
        bool correct = thrown && env->IsInstanceOf(thrown, bad < 2 ? npe : bounds);
        if (thrown) env->DeleteLocalRef(thrown);
        if (!correct) return false;
        ++exception_groups;
      }
    }
  }
  std::cerr << "ART JIT VarHandle " << (array_shape ? "int[]" : "static int")
            << " ordering modes: operation-groups=" << operation_groups
            << " exception-groups=" << exception_groups << " PASS\n";
  env->DeleteLocalRef(handle);
  if (array) env->DeleteLocalRef(array);
  if (bounds) env->DeleteLocalRef(bounds);
  return !env->ExceptionCheck();
}
inline bool CheckJitVarHandleOrderingShapes(JNIEnv* env, art::Thread* self, art::jit::Jit* jit,
    art::Handle<art::mirror::Class> owner, jclass java_owner, jclass npe) {
  return CheckJitVarHandleOrderingShape(env, self, jit, owner, java_owner, npe, false) &&
      CheckJitVarHandleOrderingShape(env, self, jit, owner, java_owner, npe, true);
}
}  // namespace aim_jni_acceptance_phase
