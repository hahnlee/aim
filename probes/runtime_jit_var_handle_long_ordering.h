#pragma once

namespace aim_jni_acceptance_phase {

inline bool CheckJitVarHandleLongOrderingShape(JNIEnv* env, art::Thread* self,
    art::jit::Jit* jit, art::Handle<art::mirror::Class> owner, jclass java_owner,
    jobject receiver, jclass npe, int shape) {
  const char* suffixes[] = {
      "GetOpaque", "GetAcquire", "GetVolatile", "SetOpaque", "SetRelease", "SetVolatile",
      "WeakPlain", "WeakAcquire", "WeakRelease", "WeakVolatile", "ExchangeAcquire",
      "ExchangeRelease", "SwapAcquire", "SwapRelease", "AddAcquire", "AddRelease", "Or",
      "OrAcquire", "OrRelease", "And", "AndAcquire", "AndRelease", "XorAcquire", "XorRelease"};
  constexpr const char* kInstanceSigs[] = {
      "(Ljava/lang/invoke/VarHandle;Ldev/aim/probe/Hello;)J",
      "(Ljava/lang/invoke/VarHandle;Ldev/aim/probe/Hello;J)V",
      "(Ljava/lang/invoke/VarHandle;Ldev/aim/probe/Hello;JJ)Z",
      "(Ljava/lang/invoke/VarHandle;Ldev/aim/probe/Hello;JJ)J",
      "(Ljava/lang/invoke/VarHandle;Ldev/aim/probe/Hello;J)J"};
  constexpr const char* kStaticSigs[] = {
      "(Ljava/lang/invoke/VarHandle;)J", "(Ljava/lang/invoke/VarHandle;J)V",
      "(Ljava/lang/invoke/VarHandle;JJ)Z", "(Ljava/lang/invoke/VarHandle;JJ)J",
      "(Ljava/lang/invoke/VarHandle;J)J"};
  constexpr const char* kArraySigs[] = {
      "(Ljava/lang/invoke/VarHandle;[JI)J", "(Ljava/lang/invoke/VarHandle;[JIJ)V",
      "(Ljava/lang/invoke/VarHandle;[JIJJ)Z", "(Ljava/lang/invoke/VarHandle;[JIJJ)J",
      "(Ljava/lang/invoke/VarHandle;[JIJ)J"};
  auto signature_kind = [](int kind) {
    return kind < 3 ? 0 : kind < 6 ? 1 : kind < 10 ? 2 : kind < 12 ? 3 : 4;
  };
  const char* prefix = shape == 0 ? "jitVarLong" :
      shape == 1 ? "jitVarStaticLong" : "jitVarLongArray";
  jmethodID ids[24]{};
  art::ArtMethod* methods[24]{};
  for (int kind = 0; kind < 24; ++kind) {
    std::string name = prefix;
    name += suffixes[kind];
    const char* sig = shape == 0 ? kInstanceSigs[signature_kind(kind)] :
        shape == 1 ? kStaticSigs[signature_kind(kind)] : kArraySigs[signature_kind(kind)];
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
  jlongArray array = nullptr;
  jfieldID field = nullptr;
  jclass bounds = shape == 2 ? env->FindClass("java/lang/ArrayIndexOutOfBoundsException") : nullptr;
  if (shape == 2) {
    jclass array_class = env->FindClass("[J");
    auto factory = env->GetStaticMethodID(
        java_owner, "jitVarArrayHandle", "(Ljava/lang/Class;)Ljava/lang/invoke/VarHandle;");
    array = env->NewLongArray(5);
    if (array_class && factory && array) {
      handle = env->CallStaticObjectMethod(java_owner, factory, array_class);
    }
    if (array_class) env->DeleteLocalRef(array_class);
  } else {
    auto factory = env->GetStaticMethodID(
        java_owner, "jitVarLongHandle", "(Z)Ljava/lang/invoke/VarHandle;");
    if (factory) handle = env->CallStaticObjectMethod(java_owner, factory, shape == 1 ? JNI_TRUE : JNI_FALSE);
    field = shape == 0 ? env->GetFieldID(java_owner, "jitVarLong", "J") :
                         env->GetStaticFieldID(java_owner, "jitVarStaticLong", "J");
  }
  if (!handle || !field && shape != 2 || shape == 2 && (!array || !bounds) ||
      env->ExceptionCheck()) return false;

  constexpr jint kIndex = 2;
  auto set_state = [&](jlong value) {
    if (shape == 0) env->SetLongField(receiver, field, value);
    else if (shape == 1) env->SetStaticLongField(java_owner, field, value);
    else env->SetLongArrayRegion(array, kIndex, 1, &value);
  };
  auto get_state = [&]() {
    if (shape == 0) return env->GetLongField(receiver, field);
    if (shape == 1) return env->GetStaticLongField(java_owner, field);
    jlong value = 0;
    env->GetLongArrayRegion(array, kIndex, 1, &value);
    return value;
  };
  auto call_get = [&](int kind) {
    if (shape == 0) return env->CallStaticLongMethod(java_owner, ids[kind], handle, receiver);
    if (shape == 1) return env->CallStaticLongMethod(java_owner, ids[kind], handle);
    return env->CallStaticLongMethod(java_owner, ids[kind], handle, array, kIndex);
  };
  auto call_set = [&](int kind, jlong value) {
    if (shape == 0) env->CallStaticVoidMethod(java_owner, ids[kind], handle, receiver, value);
    else if (shape == 1) env->CallStaticVoidMethod(java_owner, ids[kind], handle, value);
    else env->CallStaticVoidMethod(java_owner, ids[kind], handle, array, kIndex, value);
  };
  auto call_cas = [&](int kind, jlong expected, jlong value) {
    if (shape == 0) return env->CallStaticBooleanMethod(
        java_owner, ids[kind], handle, receiver, expected, value);
    if (shape == 1) return env->CallStaticBooleanMethod(
        java_owner, ids[kind], handle, expected, value);
    return env->CallStaticBooleanMethod(java_owner, ids[kind], handle, array, kIndex, expected, value);
  };
  auto call_exchange = [&](int kind, jlong expected, jlong value) {
    if (shape == 0) return env->CallStaticLongMethod(
        java_owner, ids[kind], handle, receiver, expected, value);
    if (shape == 1) return env->CallStaticLongMethod(
        java_owner, ids[kind], handle, expected, value);
    return env->CallStaticLongMethod(java_owner, ids[kind], handle, array, kIndex, expected, value);
  };
  auto call_update = [&](int kind, jlong value) {
    if (shape == 0) return env->CallStaticLongMethod(java_owner, ids[kind], handle, receiver, value);
    if (shape == 1) return env->CallStaticLongMethod(java_owner, ids[kind], handle, value);
    return env->CallStaticLongMethod(java_owner, ids[kind], handle, array, kIndex, value);
  };

  int operation_groups = 0;
  int exception_groups = 0;
  for (int phase = 0; phase < 3; ++phase) {
    if (phase) for (auto* method : methods) {
      if (!jit->CompileMethod(method, self,
          phase == 1 ? art::CompilationKind::kBaseline : art::CompilationKind::kOptimized, false) ||
          !jit->GetCodeCache()->ContainsPc(method->GetEntryPointFromQuickCompiledCode())) {
        std::cerr << "ART JIT VarHandle long ordering compile failed shape=" << shape
                  << " phase=" << phase << "\n";
        return false;
      }
    }
    for (jlong seed : {jlong(0), jlong(-1), INT64_MIN, INT64_MAX,
                       static_cast<jlong>(UINT64_C(0x0123456789abcdef))}) {
      jlong update = static_cast<jlong>(static_cast<uint64_t>(seed) ^
                                       UINT64_C(0x5a5aa5a53c3cc3c3));
      for (int kind = 0; kind < 3; ++kind) {
        set_state(seed);
        if (call_get(kind) != seed || get_state() != seed || env->ExceptionCheck()) return false;
        ++operation_groups;
      }
      for (int kind = 3; kind < 6; ++kind) {
        set_state(seed);
        call_set(kind, update);
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
      constexpr uint64_t kDelta = UINT64_C(0x13579bdf2468ace0);
      for (int kind = 14; kind < 16; ++kind) {
        set_state(seed);
        if (call_update(kind, static_cast<jlong>(kDelta)) != seed ||
            static_cast<uint64_t>(get_state()) != static_cast<uint64_t>(seed) + kDelta) return false;
        ++operation_groups;
      }
      constexpr uint64_t kMask = UINT64_C(0x0f0ff0f05555aaaa);
      for (int kind = 16; kind < 24; ++kind) {
        set_state(seed);
        uint64_t expected = kind < 19 ? static_cast<uint64_t>(seed) | kMask :
            kind < 22 ? static_cast<uint64_t>(seed) & kMask :
                        static_cast<uint64_t>(seed) ^ kMask;
        if (call_update(kind, static_cast<jlong>(kMask)) != seed ||
            static_cast<uint64_t>(get_state()) != expected) return false;
        ++operation_groups;
      }
    }

    for (int kind = 0; kind < 24; ++kind) {
      int cases = shape == 0 ? 2 : shape == 1 ? 1 : 4;
      for (int bad = 0; bad < cases; ++bad) {
        jvalue args[5]{};
        args[0].l = bad == 0 ? nullptr : handle;
        if (shape == 0) args[1].l = bad == 1 ? nullptr : receiver;
        if (shape == 2) {
          args[1].l = bad == 1 ? nullptr : array;
          args[2].i = bad == 2 ? -1 : bad == 3 ? 5 : kIndex;
        }
        int sig_kind = signature_kind(kind);
        if (sig_kind == 1) env->CallStaticVoidMethodA(java_owner, ids[kind], args);
        else if (sig_kind == 2) env->CallStaticBooleanMethodA(java_owner, ids[kind], args);
        else env->CallStaticLongMethodA(java_owner, ids[kind], args);
        jthrowable thrown = env->ExceptionOccurred();
        if (thrown) env->ExceptionClear();
        bool correct = thrown && env->IsInstanceOf(thrown,
            shape == 2 && bad >= 2 ? bounds : npe);
        if (thrown) env->DeleteLocalRef(thrown);
        if (!correct) return false;
        ++exception_groups;
      }
    }
  }
  std::cerr << "ART JIT VarHandle "
            << (shape == 0 ? "long field" : shape == 1 ? "static long" : "long[]")
            << " ordering modes: operation-groups=" << operation_groups
            << " exception-groups=" << exception_groups << " PASS\n";
  env->DeleteLocalRef(handle);
  if (array) env->DeleteLocalRef(array);
  if (bounds) env->DeleteLocalRef(bounds);
  return !env->ExceptionCheck();
}

inline bool CheckJitVarHandleLongOrdering(JNIEnv* env, art::Thread* self, art::jit::Jit* jit,
    art::Handle<art::mirror::Class> owner, jclass java_owner, jobject receiver, jclass npe) {
  return CheckJitVarHandleLongOrderingShape(
             env, self, jit, owner, java_owner, receiver, npe, 0) &&
      CheckJitVarHandleLongOrderingShape(
             env, self, jit, owner, java_owner, receiver, npe, 1) &&
      CheckJitVarHandleLongOrderingShape(
             env, self, jit, owner, java_owner, receiver, npe, 2);
}

}  // namespace aim_jni_acceptance_phase
