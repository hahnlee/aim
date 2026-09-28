#pragma once
namespace aim_jni_acceptance_phase {
inline uint64_t JitVarHandleDoubleBits(jdouble value) {
  uint64_t bits;
  static_assert(sizeof(bits) == sizeof(value));
  std::memcpy(&bits, &value, sizeof(bits));
  return bits;
}
inline jdouble JitVarHandleDoubleFromBits(uint64_t bits) {
  jdouble value;
  std::memcpy(&value, &bits, sizeof(value));
  return value;
}
inline bool CheckJitVarHandleDouble(JNIEnv* env, art::Thread* self, art::jit::Jit* jit,
    art::Handle<art::mirror::Class> owner, jclass java_owner, jobject receiver, jclass npe) {
  auto field_factory = env->GetStaticMethodID(
      java_owner, "jitVarDoubleHandle", "(Z)Ljava/lang/invoke/VarHandle;");
  auto array_factory = env->GetStaticMethodID(
      java_owner, "jitVarArrayHandle", "(Ljava/lang/Class;)Ljava/lang/invoke/VarHandle;");
  jclass double_array_class = env->FindClass("[D");
  jobject handles[3] = {
      field_factory ? env->CallStaticObjectMethod(java_owner, field_factory, JNI_FALSE) : nullptr,
      field_factory ? env->CallStaticObjectMethod(java_owner, field_factory, JNI_TRUE) : nullptr,
      array_factory ? env->CallStaticObjectMethod(java_owner, array_factory, double_array_class) : nullptr};
  const char* names[] = {
      "jitVarDoubleGet", "jitVarDoubleSet", "jitVarDoubleCas", "jitVarDoubleAdd",
      "jitVarStaticDoubleGet", "jitVarStaticDoubleSet", "jitVarStaticDoubleCas",
      "jitVarStaticDoubleAdd", "jitVarDoubleArrayGet", "jitVarDoubleArraySet",
      "jitVarDoubleArrayCas", "jitVarDoubleArrayAdd"};
  const char* sigs[] = {
      "(Ljava/lang/invoke/VarHandle;Ldev/aim/probe/Hello;)D",
      "(Ljava/lang/invoke/VarHandle;Ldev/aim/probe/Hello;D)V",
      "(Ljava/lang/invoke/VarHandle;Ldev/aim/probe/Hello;DD)Z",
      "(Ljava/lang/invoke/VarHandle;Ldev/aim/probe/Hello;D)D",
      "(Ljava/lang/invoke/VarHandle;)D", "(Ljava/lang/invoke/VarHandle;D)V",
      "(Ljava/lang/invoke/VarHandle;DD)Z", "(Ljava/lang/invoke/VarHandle;D)D",
      "(Ljava/lang/invoke/VarHandle;[DI)D", "(Ljava/lang/invoke/VarHandle;[DID)V",
      "(Ljava/lang/invoke/VarHandle;[DIDD)Z", "(Ljava/lang/invoke/VarHandle;[DID)D"};
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
  auto instance_field = env->GetFieldID(java_owner, "jitVarDouble", "D");
  auto static_field = env->GetStaticFieldID(java_owner, "jitVarStaticDouble", "D");
  jdoubleArray array = env->NewDoubleArray(5);
  jclass bounds = env->FindClass("java/lang/ArrayIndexOutOfBoundsException");
  if (!instance_field || !static_field || !array || !bounds || env->ExceptionCheck()) return false;
  const uint64_t value_bits[] = {
      UINT64_C(0x0000000000000000), UINT64_C(0x8000000000000000),
      UINT64_C(0x3ff8000000000000), UINT64_C(0xc05ef00000000000),
      UINT64_C(0x7fefffffffffffff), UINT64_C(0x7ff0000000000000),
      UINT64_C(0xfff0000000000000), UINT64_C(0x7ff8000000000042)};
  auto force_gc = [&]() {
    art::ScopedThreadStateChange native(self, art::ThreadState::kNative);
    art::Runtime::Current()->GetHeap()->CollectGarbage(false);
  };
  int field_operations = 0;
  int array_operations = 0;
  int contract_failures = 0;
  for (int phase = 0; phase < 3; ++phase) {
    if (phase) for (auto* method : methods) {
      if (!jit->CompileMethod(method, self,
          phase == 1 ? art::CompilationKind::kBaseline : art::CompilationKind::kOptimized, false) ||
          !jit->GetCodeCache()->ContainsPc(method->GetEntryPointFromQuickCompiledCode())) {
        std::cerr << "ART JIT double VarHandle compile failed phase=" << phase << "\n";
        return false;
      }
    }
    for (int stat = 0; stat < 2; ++stat) for (size_t value_index = 0;
         value_index < sizeof(value_bits) / sizeof(value_bits[0]); ++value_index) {
      int base = stat * 4;
      jobject handle = handles[stat];
      jdouble seed = JitVarHandleDoubleFromBits(value_bits[value_index]);
      jdouble update = JitVarHandleDoubleFromBits(value_bits[value_index] ^ UINT64_C(0x8000000000000000));
      auto read_field = [&]() {
        return stat ? env->GetStaticDoubleField(java_owner, static_field)
                    : env->GetDoubleField(receiver, instance_field);
      };
      auto write_field = [&](jdouble value) {
        if (stat) env->SetStaticDoubleField(java_owner, static_field, value);
        else env->SetDoubleField(receiver, instance_field, value);
      };
      auto arguments = [&](jdouble first, jdouble second) {
        std::array<jvalue, 4> args{};
        args[0].l = handle;
        args[1].l = receiver;
        args[stat ? 1 : 2].d = first;
        args[stat ? 2 : 3].d = second;
        return args;
      };
      auto set_args = arguments(seed, 0.0);
      env->CallStaticVoidMethodA(java_owner, ids[base + 1], set_args.data());
      if (env->ExceptionCheck() || JitVarHandleDoubleBits(read_field()) != value_bits[value_index]) return false;
      force_gc();
      auto get_args = arguments(0.0, 0.0);
      if (JitVarHandleDoubleBits(env->CallStaticDoubleMethodA(
              java_owner, ids[base], get_args.data())) != value_bits[value_index] || env->ExceptionCheck()) return false;
      ++field_operations;
      auto cas_args = arguments(
          JitVarHandleDoubleFromBits(value_bits[value_index] ^ 1u), update);
      if (env->CallStaticBooleanMethodA(java_owner, ids[base + 2], cas_args.data()) != JNI_FALSE ||
          JitVarHandleDoubleBits(read_field()) != value_bits[value_index] || env->ExceptionCheck()) return false;
      ++field_operations;
      cas_args = arguments(seed, update);
      if (env->CallStaticBooleanMethodA(java_owner, ids[base + 2], cas_args.data()) != JNI_TRUE ||
          JitVarHandleDoubleBits(read_field()) != JitVarHandleDoubleBits(update) || env->ExceptionCheck()) return false;
      ++field_operations;
      if (value_index < 5) {
        constexpr jdouble kDelta = 1.25;
        auto add_args = arguments(kDelta, 0.0);
        jdouble expected = update + kDelta;
        if (JitVarHandleDoubleBits(env->CallStaticDoubleMethodA(
                java_owner, ids[base + 3], add_args.data())) != JitVarHandleDoubleBits(update) ||
            JitVarHandleDoubleBits(read_field()) != JitVarHandleDoubleBits(expected) ||
            env->ExceptionCheck()) return false;
        ++field_operations;
      }
      write_field(seed);
      if (JitVarHandleDoubleBits(env->CallStaticDoubleMethodA(
              java_owner, ids[base], get_args.data())) != value_bits[value_index] || env->ExceptionCheck()) return false;
      ++field_operations;
    }
    for (jint index = 0; index < 5; ++index) {
      uint64_t seed_bits = value_bits[index];
      jdouble seed = JitVarHandleDoubleFromBits(seed_bits);
      jdouble update = JitVarHandleDoubleFromBits(seed_bits ^ UINT64_C(0x8000000000000000));
      jvalue args[6]{};
      args[0].l = handles[2];
      args[1].l = array;
      args[2].i = index;
      args[3].d = seed;
      env->CallStaticVoidMethodA(java_owner, ids[9], args);
      jdouble direct = 0.0;
      env->GetDoubleArrayRegion(array, index, 1, &direct);
      force_gc();
      if (env->ExceptionCheck() || JitVarHandleDoubleBits(direct) != seed_bits ||
          JitVarHandleDoubleBits(env->CallStaticDoubleMethodA(java_owner, ids[8], args)) != seed_bits) return false;
      ++array_operations;
      args[3].d = JitVarHandleDoubleFromBits(seed_bits ^ 1u);
      args[4].d = update;
      if (env->CallStaticBooleanMethodA(java_owner, ids[10], args) != JNI_FALSE) return false;
      env->GetDoubleArrayRegion(array, index, 1, &direct);
      if (env->ExceptionCheck() || JitVarHandleDoubleBits(direct) != seed_bits) return false;
      ++array_operations;
      args[3].d = seed;
      if (env->CallStaticBooleanMethodA(java_owner, ids[10], args) != JNI_TRUE) return false;
      env->GetDoubleArrayRegion(array, index, 1, &direct);
      if (env->ExceptionCheck() || JitVarHandleDoubleBits(direct) != JitVarHandleDoubleBits(update)) return false;
      ++array_operations;
      args[3].d = 1.25;
      jdouble expected = update + 1.25;
      if (JitVarHandleDoubleBits(env->CallStaticDoubleMethodA(java_owner, ids[11], args)) !=
              JitVarHandleDoubleBits(update)) return false;
      env->GetDoubleArrayRegion(array, index, 1, &direct);
      if (env->ExceptionCheck() || JitVarHandleDoubleBits(direct) != JitVarHandleDoubleBits(expected)) return false;
      ++array_operations;
      env->SetDoubleArrayRegion(array, index, 1, &seed);
      if (JitVarHandleDoubleBits(env->CallStaticDoubleMethodA(java_owner, ids[8], args)) != seed_bits ||
          env->ExceptionCheck()) return false;
      ++array_operations;
    }
    for (int kind = 0; kind < 12; ++kind) {
      int target = kind / 4;
      int operation = kind % 4;
      int null_cases = target == 1 ? 1 : 2;
      for (int failure = 0; failure < null_cases; ++failure) {
        jvalue args[6]{};
        args[0].l = failure == 0 ? nullptr : handles[target];
        args[1].l = failure == 1 ? nullptr : (target == 0 ? receiver : jobject(array));
        if (operation == 0 || operation == 3) env->CallStaticDoubleMethodA(java_owner, ids[kind], args);
        else if (operation == 1) env->CallStaticVoidMethodA(java_owner, ids[kind], args);
        else env->CallStaticBooleanMethodA(java_owner, ids[kind], args);
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
        if (operation == 0 || operation == 3) env->CallStaticDoubleMethodA(java_owner, ids[kind], args);
        else if (operation == 1) env->CallStaticVoidMethodA(java_owner, ids[kind], args);
        else env->CallStaticBooleanMethodA(java_owner, ids[kind], args);
        jthrowable thrown = env->ExceptionOccurred();
        if (thrown) env->ExceptionClear();
        bool correct = thrown && env->IsInstanceOf(thrown, bounds);
        if (thrown) env->DeleteLocalRef(thrown);
        if (!correct) return false;
        ++contract_failures;
      }
    }
  }
  env->SetDoubleField(receiver, instance_field, 0.0);
  env->SetStaticDoubleField(java_owner, static_field, 0.0);
  for (jobject handle : handles) env->DeleteLocalRef(handle);
  env->DeleteLocalRef(array);
  env->DeleteLocalRef(double_array_class);
  env->DeleteLocalRef(bounds);
  std::cerr << "ART JIT double VarHandle: field/static-operations=" << field_operations
            << " array-operations=" << array_operations
            << " null/bounds-failures=" << contract_failures << " PASS\n";
  return !env->ExceptionCheck();
}
}  // namespace aim_jni_acceptance_phase
