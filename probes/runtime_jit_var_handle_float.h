#pragma once
namespace aim_jni_acceptance_phase {
inline uint32_t JitVarHandleFloatBits(jfloat value) {
  uint32_t bits;
  static_assert(sizeof(bits) == sizeof(value));
  std::memcpy(&bits, &value, sizeof(bits));
  return bits;
}
inline jfloat JitVarHandleFloatFromBits(uint32_t bits) {
  jfloat value;
  std::memcpy(&value, &bits, sizeof(value));
  return value;
}
inline bool CheckJitVarHandleFloat(JNIEnv* env, art::Thread* self, art::jit::Jit* jit,
    art::Handle<art::mirror::Class> owner, jclass java_owner, jobject receiver, jclass npe) {
  auto field_factory = env->GetStaticMethodID(
      java_owner, "jitVarFloatHandle", "(Z)Ljava/lang/invoke/VarHandle;");
  auto array_factory = env->GetStaticMethodID(
      java_owner, "jitVarArrayHandle", "(Ljava/lang/Class;)Ljava/lang/invoke/VarHandle;");
  jclass array_class = env->FindClass("[F");
  jobject handles[3] = {
      field_factory ? env->CallStaticObjectMethod(java_owner, field_factory, JNI_FALSE) : nullptr,
      field_factory ? env->CallStaticObjectMethod(java_owner, field_factory, JNI_TRUE) : nullptr,
      array_factory ? env->CallStaticObjectMethod(java_owner, array_factory, array_class) : nullptr};
  const char* names[] = {
      "jitVarFloatGet", "jitVarFloatSet", "jitVarFloatCas", "jitVarFloatAdd",
      "jitVarStaticFloatGet", "jitVarStaticFloatSet", "jitVarStaticFloatCas",
      "jitVarStaticFloatAdd", "jitVarFloatArrayGet", "jitVarFloatArraySet",
      "jitVarFloatArrayCas", "jitVarFloatArrayAdd"};
  const char* sigs[] = {
      "(Ljava/lang/invoke/VarHandle;Ldev/aim/probe/Hello;)F",
      "(Ljava/lang/invoke/VarHandle;Ldev/aim/probe/Hello;F)V",
      "(Ljava/lang/invoke/VarHandle;Ldev/aim/probe/Hello;FF)Z",
      "(Ljava/lang/invoke/VarHandle;Ldev/aim/probe/Hello;F)F",
      "(Ljava/lang/invoke/VarHandle;)F", "(Ljava/lang/invoke/VarHandle;F)V",
      "(Ljava/lang/invoke/VarHandle;FF)Z", "(Ljava/lang/invoke/VarHandle;F)F",
      "(Ljava/lang/invoke/VarHandle;[FI)F", "(Ljava/lang/invoke/VarHandle;[FIF)V",
      "(Ljava/lang/invoke/VarHandle;[FIFF)Z", "(Ljava/lang/invoke/VarHandle;[FIF)F"};
  jmethodID ids[12]{};
  art::ArtMethod* methods[12]{};
  for (int kind = 0; kind < 12; ++kind) {
    ids[kind] = env->GetStaticMethodID(java_owner, names[kind], sigs[kind]);
    methods[kind] = owner->FindClassMethod(names[kind], sigs[kind], art::kRuntimePointerSize);
    if (!handles[kind / 4] || !ids[kind] || !methods[kind] || env->ExceptionCheck()) return false;
    art::CodeItemDataAccessor code(*methods[kind]->GetDexFile(), methods[kind]->GetCodeItem());
    bool polymorphic = false;
    for (const auto& pair : code) {
      polymorphic |= pair.Inst().Opcode() == art::Instruction::INVOKE_POLYMORPHIC ||
          pair.Inst().Opcode() == art::Instruction::INVOKE_POLYMORPHIC_RANGE;
    }
    if (!polymorphic) return false;
  }
  auto instance_field = env->GetFieldID(java_owner, "jitVarFloat", "F");
  auto static_field = env->GetStaticFieldID(java_owner, "jitVarStaticFloat", "F");
  jfloatArray array = env->NewFloatArray(5);
  jclass bounds = env->FindClass("java/lang/ArrayIndexOutOfBoundsException");
  if (!instance_field || !static_field || !array || !bounds || env->ExceptionCheck()) return false;
  const uint32_t value_bits[] = {
      UINT32_C(0x00000000), UINT32_C(0x80000000), UINT32_C(0x3fc00000),
      UINT32_C(0xc2f78000), UINT32_C(0x7f7fffff), UINT32_C(0x7f800000),
      UINT32_C(0xff800000), UINT32_C(0x7fc00042)};
  auto force_gc = [&]() {
    art::ScopedThreadStateChange native(self, art::ThreadState::kNative);
    art::Runtime::Current()->GetHeap()->CollectGarbage(false);
  };
  auto call_for_exception = [&](int kind, jvalue* args) {
    int operation = kind % 4;
    if (operation == 0 || operation == 3) env->CallStaticFloatMethodA(java_owner, ids[kind], args);
    else if (operation == 1) env->CallStaticVoidMethodA(java_owner, ids[kind], args);
    else env->CallStaticBooleanMethodA(java_owner, ids[kind], args);
  };
  int field_operations = 0;
  int array_operations = 0;
  int contract_failures = 0;
  for (int phase = 0; phase < 3; ++phase) {
    if (phase) for (auto* method : methods) {
      if (!jit->CompileMethod(method, self,
          phase == 1 ? art::CompilationKind::kBaseline : art::CompilationKind::kOptimized, false) ||
          !jit->GetCodeCache()->ContainsPc(method->GetEntryPointFromQuickCompiledCode())) {
        std::cerr << "ART JIT float VarHandle compile failed phase=" << phase << "\n";
        return false;
      }
    }
    for (int stat = 0; stat < 2; ++stat) for (size_t value_index = 0;
         value_index < sizeof(value_bits) / sizeof(value_bits[0]); ++value_index) {
      int base = stat * 4;
      jobject handle = handles[stat];
      jfloat seed = JitVarHandleFloatFromBits(value_bits[value_index]);
      jfloat update = JitVarHandleFloatFromBits(value_bits[value_index] ^ UINT32_C(0x80000000));
      auto read_field = [&]() {
        return stat ? env->GetStaticFloatField(java_owner, static_field)
                    : env->GetFloatField(receiver, instance_field);
      };
      auto write_field = [&](jfloat value) {
        if (stat) env->SetStaticFloatField(java_owner, static_field, value);
        else env->SetFloatField(receiver, instance_field, value);
      };
      auto arguments = [&](jfloat first, jfloat second) {
        std::array<jvalue, 4> args{};
        args[0].l = handle;
        args[1].l = receiver;
        args[stat ? 1 : 2].f = first;
        args[stat ? 2 : 3].f = second;
        return args;
      };
      auto args = arguments(seed, 0.0f);
      env->CallStaticVoidMethodA(java_owner, ids[base + 1], args.data());
      if (env->ExceptionCheck() || JitVarHandleFloatBits(read_field()) != value_bits[value_index]) return false;
      force_gc();
      args = arguments(0.0f, 0.0f);
      if (JitVarHandleFloatBits(env->CallStaticFloatMethodA(java_owner, ids[base], args.data())) !=
              value_bits[value_index] || env->ExceptionCheck()) return false;
      ++field_operations;
      args = arguments(JitVarHandleFloatFromBits(value_bits[value_index] ^ 1u), update);
      if (env->CallStaticBooleanMethodA(java_owner, ids[base + 2], args.data()) != JNI_FALSE ||
          JitVarHandleFloatBits(read_field()) != value_bits[value_index] || env->ExceptionCheck()) return false;
      ++field_operations;
      args = arguments(seed, update);
      if (env->CallStaticBooleanMethodA(java_owner, ids[base + 2], args.data()) != JNI_TRUE ||
          JitVarHandleFloatBits(read_field()) != JitVarHandleFloatBits(update) || env->ExceptionCheck()) return false;
      ++field_operations;
      if (value_index < 5) {
        constexpr jfloat kDelta = 1.25f;
        args = arguments(kDelta, 0.0f);
        jfloat expected = update + kDelta;
        if (JitVarHandleFloatBits(env->CallStaticFloatMethodA(java_owner, ids[base + 3], args.data())) !=
                JitVarHandleFloatBits(update) ||
            JitVarHandleFloatBits(read_field()) != JitVarHandleFloatBits(expected) ||
            env->ExceptionCheck()) return false;
        ++field_operations;
      }
      write_field(seed);
      args = arguments(0.0f, 0.0f);
      if (JitVarHandleFloatBits(env->CallStaticFloatMethodA(java_owner, ids[base], args.data())) !=
              value_bits[value_index] || env->ExceptionCheck()) return false;
      ++field_operations;
    }
    for (jint index = 0; index < 5; ++index) {
      uint32_t seed_bits = value_bits[index];
      jfloat seed = JitVarHandleFloatFromBits(seed_bits);
      jfloat update = JitVarHandleFloatFromBits(seed_bits ^ UINT32_C(0x80000000));
      jvalue args[6]{};
      args[0].l = handles[2];
      args[1].l = array;
      args[2].i = index;
      args[3].f = seed;
      env->CallStaticVoidMethodA(java_owner, ids[9], args);
      jfloat direct = 0.0f;
      env->GetFloatArrayRegion(array, index, 1, &direct);
      force_gc();
      if (env->ExceptionCheck() || JitVarHandleFloatBits(direct) != seed_bits ||
          JitVarHandleFloatBits(env->CallStaticFloatMethodA(java_owner, ids[8], args)) != seed_bits) return false;
      ++array_operations;
      args[3].f = JitVarHandleFloatFromBits(seed_bits ^ 1u);
      args[4].f = update;
      if (env->CallStaticBooleanMethodA(java_owner, ids[10], args) != JNI_FALSE) return false;
      env->GetFloatArrayRegion(array, index, 1, &direct);
      if (env->ExceptionCheck() || JitVarHandleFloatBits(direct) != seed_bits) return false;
      ++array_operations;
      args[3].f = seed;
      if (env->CallStaticBooleanMethodA(java_owner, ids[10], args) != JNI_TRUE) return false;
      env->GetFloatArrayRegion(array, index, 1, &direct);
      if (env->ExceptionCheck() || JitVarHandleFloatBits(direct) != JitVarHandleFloatBits(update)) return false;
      ++array_operations;
      args[3].f = 1.25f;
      jfloat expected = update + 1.25f;
      if (JitVarHandleFloatBits(env->CallStaticFloatMethodA(java_owner, ids[11], args)) !=
              JitVarHandleFloatBits(update)) return false;
      env->GetFloatArrayRegion(array, index, 1, &direct);
      if (env->ExceptionCheck() || JitVarHandleFloatBits(direct) != JitVarHandleFloatBits(expected)) return false;
      ++array_operations;
      env->SetFloatArrayRegion(array, index, 1, &seed);
      if (JitVarHandleFloatBits(env->CallStaticFloatMethodA(java_owner, ids[8], args)) != seed_bits ||
          env->ExceptionCheck()) return false;
      ++array_operations;
    }
    for (int kind = 0; kind < 12; ++kind) {
      int target = kind / 4;
      int null_cases = target == 1 ? 1 : 2;
      for (int failure = 0; failure < null_cases; ++failure) {
        jvalue args[6]{};
        args[0].l = failure == 0 ? nullptr : handles[target];
        args[1].l = failure == 1 ? nullptr : (target == 0 ? receiver : jobject(array));
        call_for_exception(kind, args);
        jthrowable thrown = env->ExceptionOccurred();
        if (thrown) env->ExceptionClear();
        bool correct = thrown && env->IsInstanceOf(thrown, npe);
        if (thrown) env->DeleteLocalRef(thrown);
        if (!correct) return false;
        ++contract_failures;
      }
      if (target == 2) for (jint bad_index : {jint(-1), jint(5)}) {
        jvalue args[6]{};
        args[0].l = handles[target];
        args[1].l = array;
        args[2].i = bad_index;
        call_for_exception(kind, args);
        jthrowable thrown = env->ExceptionOccurred();
        if (thrown) env->ExceptionClear();
        bool correct = thrown && env->IsInstanceOf(thrown, bounds);
        if (thrown) env->DeleteLocalRef(thrown);
        if (!correct) return false;
        ++contract_failures;
      }
    }
  }
  env->SetFloatField(receiver, instance_field, 0.0f);
  env->SetStaticFloatField(java_owner, static_field, 0.0f);
  for (jobject handle : handles) env->DeleteLocalRef(handle);
  env->DeleteLocalRef(array);
  env->DeleteLocalRef(array_class);
  env->DeleteLocalRef(bounds);
  std::cerr << "ART JIT float VarHandle: field/static-operations=" << field_operations
            << " array-operations=" << array_operations
            << " null/bounds-failures=" << contract_failures << " PASS\n";
  return !env->ExceptionCheck();
}
}  // namespace aim_jni_acceptance_phase
