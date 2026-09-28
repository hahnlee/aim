#pragma once
namespace aim_jni_acceptance_phase {
inline uint64_t JitVarHandleLongBits(jlong value) {
  uint64_t bits;
  static_assert(sizeof(bits) == sizeof(value));
  std::memcpy(&bits, &value, sizeof(bits));
  return bits;
}
inline jlong JitVarHandleLongFromBits(uint64_t bits) {
  jlong value;
  std::memcpy(&value, &bits, sizeof(value));
  return value;
}
inline bool CheckJitVarHandleLong(JNIEnv* env, art::Thread* self, art::jit::Jit* jit,
    art::Handle<art::mirror::Class> owner, jclass java_owner, jobject receiver, jclass npe) {
  auto factory = env->GetStaticMethodID(
      java_owner, "jitVarLongHandle", "(Z)Ljava/lang/invoke/VarHandle;");
  jobject handles[2] = {
      factory ? env->CallStaticObjectMethod(java_owner, factory, JNI_FALSE) : nullptr,
      factory ? env->CallStaticObjectMethod(java_owner, factory, JNI_TRUE) : nullptr};
  const char* names[] = {"jitVarLongGet", "jitVarLongSet", "jitVarLongCas", "jitVarLongAdd",
                         "jitVarStaticLongGet", "jitVarStaticLongSet", "jitVarStaticLongCas",
                         "jitVarStaticLongAdd"};
  const char* sigs[] = {
      "(Ljava/lang/invoke/VarHandle;Ldev/aim/probe/Hello;)J",
      "(Ljava/lang/invoke/VarHandle;Ldev/aim/probe/Hello;J)V",
      "(Ljava/lang/invoke/VarHandle;Ldev/aim/probe/Hello;JJ)Z",
      "(Ljava/lang/invoke/VarHandle;Ldev/aim/probe/Hello;J)J",
      "(Ljava/lang/invoke/VarHandle;)J",
      "(Ljava/lang/invoke/VarHandle;J)V",
      "(Ljava/lang/invoke/VarHandle;JJ)Z",
      "(Ljava/lang/invoke/VarHandle;J)J"};
  jmethodID ids[8]{};
  art::ArtMethod* methods[8]{};
  for (int kind = 0; kind < 8; ++kind) {
    ids[kind] = env->GetStaticMethodID(java_owner, names[kind], sigs[kind]);
    methods[kind] = owner->FindClassMethod(names[kind], sigs[kind], art::kRuntimePointerSize);
    if (!handles[kind / 4] || !ids[kind] || !methods[kind] || env->ExceptionCheck()) return false;
    art::CodeItemDataAccessor code(*methods[kind]->GetDexFile(), methods[kind]->GetCodeItem());
    bool has_invoke_polymorphic = false;
    for (const auto& pair : code) {
      has_invoke_polymorphic |= pair.Inst().Opcode() == art::Instruction::INVOKE_POLYMORPHIC ||
          pair.Inst().Opcode() == art::Instruction::INVOKE_POLYMORPHIC_RANGE;
    }
    if (!has_invoke_polymorphic) {
      std::cerr << "ART JIT long VarHandle wrapper is not invoke-polymorphic kind=" << kind << "\n";
      return false;
    }
  }
  auto instance_field = env->GetFieldID(java_owner, "jitVarLong", "J");
  auto static_field = env->GetStaticFieldID(java_owner, "jitVarStaticLong", "J");
  if (!instance_field || !static_field || env->ExceptionCheck()) return false;
  const jlong seeds[] = {jlong(0), jlong(1), jlong(-1), INT64_MIN, INT64_MAX,
                         JitVarHandleLongFromBits(UINT64_C(0x0123456789abcdef))};
  for (int phase = 0; phase < 3; ++phase) {
    if (phase) for (auto* method : methods) {
      if (!jit->CompileMethod(method, self,
          phase == 1 ? art::CompilationKind::kBaseline : art::CompilationKind::kOptimized, false) ||
          !jit->GetCodeCache()->ContainsPc(method->GetEntryPointFromQuickCompiledCode())) {
        std::cerr << "ART JIT long VarHandle compile failed phase=" << phase << "\n";
        return false;
      }
    }
    for (int stat = 0; stat < 2; ++stat) for (jlong seed : seeds) {
      int base = stat * 4;
      jobject handle = handles[stat];
      uint64_t seed_bits = JitVarHandleLongBits(seed);
      jlong update = JitVarHandleLongFromBits(seed_bits ^ UINT64_C(0x5a5aa5a53c3cc3c3));
      auto read_field = [&]() {
        return stat ? env->GetStaticLongField(java_owner, static_field)
                    : env->GetLongField(receiver, instance_field);
      };
      auto write_field = [&](jlong value) {
        if (stat) env->SetStaticLongField(java_owner, static_field, value);
        else env->SetLongField(receiver, instance_field, value);
      };
      auto call_get = [&]() {
        jvalue args[2]{};
        args[0].l = handle;
        args[1].l = receiver;
        return env->CallStaticLongMethodA(java_owner, ids[base], args);
      };
      auto call_set = [&](jlong value) {
        jvalue args[3]{};
        args[0].l = handle;
        args[1].l = receiver;
        args[stat ? 1 : 2].j = value;
        env->CallStaticVoidMethodA(java_owner, ids[base + 1], args);
      };
      auto call_cas = [&](jlong expected, jlong value) {
        jvalue args[4]{};
        args[0].l = handle;
        args[1].l = receiver;
        args[stat ? 1 : 2].j = expected;
        args[stat ? 2 : 3].j = value;
        return env->CallStaticBooleanMethodA(java_owner, ids[base + 2], args);
      };
      auto call_add = [&](jlong value) {
        jvalue args[3]{};
        args[0].l = handle;
        args[1].l = receiver;
        args[stat ? 1 : 2].j = value;
        return env->CallStaticLongMethodA(java_owner, ids[base + 3], args);
      };
      auto check = [&](bool result, uint64_t expected, const char* operation) {
        bool correct = result && !env->ExceptionCheck() &&
            JitVarHandleLongBits(read_field()) == expected;
        if (!correct) {
          std::cerr << "ART JIT long VarHandle failed op=" << operation
                    << " static=" << stat << " phase=" << phase << "\n";
          env->ExceptionDescribe();
        }
        return correct;
      };
      call_set(seed);
      if (!check(true, seed_bits, "set/JNI read")) return false;
      { art::ScopedThreadStateChange native(self, art::ThreadState::kNative);
        art::Runtime::Current()->GetHeap()->CollectGarbage(false); }
      if (!check(JitVarHandleLongBits(call_get()) == seed_bits, seed_bits, "get after GC")) return false;
      if (!check(call_cas(JitVarHandleLongFromBits(seed_bits ^ 1), update) == JNI_FALSE,
                 seed_bits, "CAS mismatch")) return false;
      if (!check(call_cas(seed, update) == JNI_TRUE,
                 JitVarHandleLongBits(update), "CAS success")) return false;
      constexpr uint64_t kDelta = UINT64_C(0x23456789abcdef01);
      if (!check(JitVarHandleLongBits(call_add(JitVarHandleLongFromBits(kDelta))) ==
                     JitVarHandleLongBits(update),
                 JitVarHandleLongBits(update) + kDelta, "get-and-add")) return false;
      jlong direct = JitVarHandleLongFromBits(seed_bits ^ UINT64_C(0xf00dcafe12345678));
      write_field(direct);
      if (!check(JitVarHandleLongBits(call_get()) == JitVarHandleLongBits(direct),
                 JitVarHandleLongBits(direct), "JNI store/get")) return false;
    }
    for (int kind = 0; kind < 8; ++kind) {
      int stat = kind / 4;
      int failure_cases = stat ? 1 : 2;
      for (int failure = 0; failure < failure_cases; ++failure) {
        jvalue args[4]{};
        args[0].l = failure == 0 ? nullptr : handles[stat];
        args[1].l = failure == 1 ? nullptr : receiver;
        int operation = kind % 4;
        if (operation == 0 || operation == 3) env->CallStaticLongMethodA(java_owner, ids[kind], args);
        else if (operation == 1) env->CallStaticVoidMethodA(java_owner, ids[kind], args);
        else env->CallStaticBooleanMethodA(java_owner, ids[kind], args);
        jthrowable thrown = env->ExceptionOccurred();
        if (thrown) env->ExceptionClear();
        bool correct = thrown && env->IsInstanceOf(thrown, npe);
        if (thrown) env->DeleteLocalRef(thrown);
        if (!correct) {
          std::cerr << "ART JIT long VarHandle null failure kind=" << kind
                    << " case=" << failure << " phase=" << phase << "\n";
          return false;
        }
      }
    }
  }
  env->SetLongField(receiver, instance_field, 0);
  env->SetStaticLongField(java_owner, static_field, 0);
  env->DeleteLocalRef(handles[0]);
  env->DeleteLocalRef(handles[1]);
  std::cerr << "ART JIT long VarHandle: instance/static operations=180 null-failures=36 PASS\n";
  return !env->ExceptionCheck();
}
}  // namespace aim_jni_acceptance_phase
