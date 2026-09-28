#pragma once
namespace aim_jni_acceptance_phase {
inline bool CheckJitVarHandleNarrow16(JNIEnv* env, art::Thread* self, art::jit::Jit* jit,
    art::Handle<art::mirror::Class> owner, jclass java_owner, jobject receiver, jclass npe) {
  auto short_factory = env->GetStaticMethodID(
      java_owner, "jitVarShortHandle", "(Z)Ljava/lang/invoke/VarHandle;");
  auto char_factory = env->GetStaticMethodID(
      java_owner, "jitVarCharHandle", "(Z)Ljava/lang/invoke/VarHandle;");
  auto array_factory = env->GetStaticMethodID(
      java_owner, "jitVarArrayHandle", "(Ljava/lang/Class;)Ljava/lang/invoke/VarHandle;");
  jclass short_array_class = env->FindClass("[S");
  jclass char_array_class = env->FindClass("[C");
  jobject handles[6] = {
      short_factory ? env->CallStaticObjectMethod(java_owner, short_factory, JNI_FALSE) : nullptr,
      short_factory ? env->CallStaticObjectMethod(java_owner, short_factory, JNI_TRUE) : nullptr,
      array_factory ? env->CallStaticObjectMethod(java_owner, array_factory, short_array_class) : nullptr,
      char_factory ? env->CallStaticObjectMethod(java_owner, char_factory, JNI_FALSE) : nullptr,
      char_factory ? env->CallStaticObjectMethod(java_owner, char_factory, JNI_TRUE) : nullptr,
      array_factory ? env->CallStaticObjectMethod(java_owner, array_factory, char_array_class) : nullptr};
  const char* names[] = {
      "jitVarShortGet", "jitVarShortSet", "jitVarShortCas", "jitVarShortAdd",
      "jitVarStaticShortGet", "jitVarStaticShortSet", "jitVarStaticShortCas", "jitVarStaticShortAdd",
      "jitVarShortArrayGet", "jitVarShortArraySet", "jitVarShortArrayCas", "jitVarShortArrayAdd",
      "jitVarCharGet", "jitVarCharSet", "jitVarCharCas", "jitVarCharAdd",
      "jitVarStaticCharGet", "jitVarStaticCharSet", "jitVarStaticCharCas", "jitVarStaticCharAdd",
      "jitVarCharArrayGet", "jitVarCharArraySet", "jitVarCharArrayCas", "jitVarCharArrayAdd"};
  const char* sigs[] = {
      "(Ljava/lang/invoke/VarHandle;Ldev/aim/probe/Hello;)S",
      "(Ljava/lang/invoke/VarHandle;Ldev/aim/probe/Hello;S)V",
      "(Ljava/lang/invoke/VarHandle;Ldev/aim/probe/Hello;SS)Z",
      "(Ljava/lang/invoke/VarHandle;Ldev/aim/probe/Hello;S)S",
      "(Ljava/lang/invoke/VarHandle;)S", "(Ljava/lang/invoke/VarHandle;S)V",
      "(Ljava/lang/invoke/VarHandle;SS)Z", "(Ljava/lang/invoke/VarHandle;S)S",
      "(Ljava/lang/invoke/VarHandle;[SI)S", "(Ljava/lang/invoke/VarHandle;[SIS)V",
      "(Ljava/lang/invoke/VarHandle;[SISS)Z", "(Ljava/lang/invoke/VarHandle;[SIS)S",
      "(Ljava/lang/invoke/VarHandle;Ldev/aim/probe/Hello;)C",
      "(Ljava/lang/invoke/VarHandle;Ldev/aim/probe/Hello;C)V",
      "(Ljava/lang/invoke/VarHandle;Ldev/aim/probe/Hello;CC)Z",
      "(Ljava/lang/invoke/VarHandle;Ldev/aim/probe/Hello;C)C",
      "(Ljava/lang/invoke/VarHandle;)C", "(Ljava/lang/invoke/VarHandle;C)V",
      "(Ljava/lang/invoke/VarHandle;CC)Z", "(Ljava/lang/invoke/VarHandle;C)C",
      "(Ljava/lang/invoke/VarHandle;[CI)C", "(Ljava/lang/invoke/VarHandle;[CIC)V",
      "(Ljava/lang/invoke/VarHandle;[CICC)Z", "(Ljava/lang/invoke/VarHandle;[CIC)C"};
  jmethodID ids[24]{};
  art::ArtMethod* methods[24]{};
  for (int kind = 0; kind < 24; ++kind) {
    ids[kind] = env->GetStaticMethodID(java_owner, names[kind], sigs[kind]);
    methods[kind] = owner->FindClassMethod(names[kind], sigs[kind], art::kRuntimePointerSize);
    if (!handles[(kind / 12) * 3 + (kind % 12) / 4] || !ids[kind] || !methods[kind] ||
        env->ExceptionCheck()) return false;
    art::CodeItemDataAccessor code(*methods[kind]->GetDexFile(), methods[kind]->GetCodeItem());
    bool polymorphic = false;
    for (const auto& pair : code) {
      polymorphic |= pair.Inst().Opcode() == art::Instruction::INVOKE_POLYMORPHIC ||
          pair.Inst().Opcode() == art::Instruction::INVOKE_POLYMORPHIC_RANGE;
    }
    if (!polymorphic) return false;
  }
  auto short_field = env->GetFieldID(java_owner, "jitVarShort", "S");
  auto static_short_field = env->GetStaticFieldID(java_owner, "jitVarStaticShort", "S");
  auto char_field = env->GetFieldID(java_owner, "jitVarChar", "C");
  auto static_char_field = env->GetStaticFieldID(java_owner, "jitVarStaticChar", "C");
  jshortArray shorts = env->NewShortArray(6);
  jcharArray chars = env->NewCharArray(6);
  jclass bounds = env->FindClass("java/lang/ArrayIndexOutOfBoundsException");
  if (!short_field || !static_short_field || !char_field || !static_char_field ||
      !shorts || !chars || !bounds || env->ExceptionCheck()) return false;
  auto force_gc = [&]() {
    art::ScopedThreadStateChange native(self, art::ThreadState::kNative);
    art::Runtime::Current()->GetHeap()->CollectGarbage(false);
  };
  int short_operations = 0;
  int char_operations = 0;
  int contract_failures = 0;
  for (int phase = 0; phase < 3; ++phase) {
    if (phase) for (auto* method : methods) {
      if (!jit->CompileMethod(method, self,
          phase == 1 ? art::CompilationKind::kBaseline : art::CompilationKind::kOptimized, false) ||
          !jit->GetCodeCache()->ContainsPc(method->GetEntryPointFromQuickCompiledCode())) {
        std::cerr << "ART JIT narrow16 VarHandle compile failed phase=" << phase << "\n";
        return false;
      }
    }
    const jshort short_values[] = {
        jshort(0), jshort(1), jshort(-1), jshort(-32768), jshort(32767), jshort(0x5aa5)};
    for (int target = 0; target < 3; ++target) for (jint index = 0; index < 6; ++index) {
      int base = target * 4;
      jshort seed = short_values[index];
      jshort update = jshort(static_cast<uint16_t>(seed) ^ UINT16_C(0xa55a));
      auto arguments = [&](jshort first, jshort second) {
        std::array<jvalue, 5> args{};
        args[0].l = handles[target];
        if (target == 0) { args[1].l = receiver; args[2].s = first; args[3].s = second; }
        else if (target == 1) { args[1].s = first; args[2].s = second; }
        else { args[1].l = shorts; args[2].i = index; args[3].s = first; args[4].s = second; }
        return args;
      };
      auto read_target = [&]() {
        jshort value = target == 0 ? env->GetShortField(receiver, short_field)
            : target == 1 ? env->GetStaticShortField(java_owner, static_short_field) : jshort(0);
        if (target == 2) env->GetShortArrayRegion(shorts, index, 1, &value);
        return value;
      };
      auto write_target = [&](jshort value) {
        if (target == 0) env->SetShortField(receiver, short_field, value);
        else if (target == 1) env->SetStaticShortField(java_owner, static_short_field, value);
        else env->SetShortArrayRegion(shorts, index, 1, &value);
      };
      auto args = arguments(seed, 0);
      env->CallStaticVoidMethodA(java_owner, ids[base + 1], args.data());
      if (env->ExceptionCheck() || read_target() != seed) return false;
      force_gc();
      args = arguments(0, 0);
      if (env->CallStaticShortMethodA(java_owner, ids[base], args.data()) != seed ||
          env->ExceptionCheck()) return false;
      ++short_operations;
      args = arguments(jshort(seed ^ 1), update);
      if (env->CallStaticBooleanMethodA(java_owner, ids[base + 2], args.data()) != JNI_FALSE ||
          read_target() != seed || env->ExceptionCheck()) return false;
      ++short_operations;
      args = arguments(seed, update);
      if (env->CallStaticBooleanMethodA(java_owner, ids[base + 2], args.data()) != JNI_TRUE ||
          read_target() != update || env->ExceptionCheck()) return false;
      ++short_operations;
      constexpr jshort kDelta = 0x1234;
      args = arguments(kDelta, 0);
      jshort expected = jshort(static_cast<uint16_t>(update) + static_cast<uint16_t>(kDelta));
      if (env->CallStaticShortMethodA(java_owner, ids[base + 3], args.data()) != update ||
          read_target() != expected || env->ExceptionCheck()) return false;
      ++short_operations;
      write_target(seed);
      args = arguments(0, 0);
      if (env->CallStaticShortMethodA(java_owner, ids[base], args.data()) != seed ||
          env->ExceptionCheck()) return false;
      ++short_operations;
    }
    const jchar char_values[] = {
        jchar(0), jchar(1), jchar(0x7fff), jchar(0x8000), jchar(0xffff), jchar(0x5aa5)};
    for (int target = 0; target < 3; ++target) for (jint index = 0; index < 6; ++index) {
      int base = 12 + target * 4;
      jchar seed = char_values[index];
      jchar update = jchar(seed ^ UINT16_C(0xa55a));
      auto arguments = [&](jchar first, jchar second) {
        std::array<jvalue, 5> args{};
        args[0].l = handles[3 + target];
        if (target == 0) { args[1].l = receiver; args[2].c = first; args[3].c = second; }
        else if (target == 1) { args[1].c = first; args[2].c = second; }
        else { args[1].l = chars; args[2].i = index; args[3].c = first; args[4].c = second; }
        return args;
      };
      auto read_target = [&]() {
        jchar value = target == 0 ? env->GetCharField(receiver, char_field)
            : target == 1 ? env->GetStaticCharField(java_owner, static_char_field) : jchar(0);
        if (target == 2) env->GetCharArrayRegion(chars, index, 1, &value);
        return value;
      };
      auto write_target = [&](jchar value) {
        if (target == 0) env->SetCharField(receiver, char_field, value);
        else if (target == 1) env->SetStaticCharField(java_owner, static_char_field, value);
        else env->SetCharArrayRegion(chars, index, 1, &value);
      };
      auto args = arguments(seed, 0);
      env->CallStaticVoidMethodA(java_owner, ids[base + 1], args.data());
      if (env->ExceptionCheck() || read_target() != seed) return false;
      force_gc();
      args = arguments(0, 0);
      if (env->CallStaticCharMethodA(java_owner, ids[base], args.data()) != seed ||
          env->ExceptionCheck()) return false;
      ++char_operations;
      args = arguments(jchar(seed ^ 1u), update);
      if (env->CallStaticBooleanMethodA(java_owner, ids[base + 2], args.data()) != JNI_FALSE ||
          read_target() != seed || env->ExceptionCheck()) return false;
      ++char_operations;
      args = arguments(seed, update);
      if (env->CallStaticBooleanMethodA(java_owner, ids[base + 2], args.data()) != JNI_TRUE ||
          read_target() != update || env->ExceptionCheck()) return false;
      ++char_operations;
      constexpr jchar kDelta = 0x4321;
      args = arguments(kDelta, 0);
      jchar expected = jchar(static_cast<uint16_t>(update) + static_cast<uint16_t>(kDelta));
      if (env->CallStaticCharMethodA(java_owner, ids[base + 3], args.data()) != update ||
          read_target() != expected || env->ExceptionCheck()) return false;
      ++char_operations;
      write_target(seed);
      args = arguments(0, 0);
      if (env->CallStaticCharMethodA(java_owner, ids[base], args.data()) != seed ||
          env->ExceptionCheck()) return false;
      ++char_operations;
    }
    for (int kind = 0; kind < 24; ++kind) {
      int group = kind / 12;
      int target = (kind % 12) / 4;
      int operation = kind % 4;
      jobject handle = handles[group * 3 + target];
      auto invoke = [&](jvalue* args) {
        if (operation == 1) env->CallStaticVoidMethodA(java_owner, ids[kind], args);
        else if (operation == 2) env->CallStaticBooleanMethodA(java_owner, ids[kind], args);
        else if (group == 0) env->CallStaticShortMethodA(java_owner, ids[kind], args);
        else env->CallStaticCharMethodA(java_owner, ids[kind], args);
      };
      for (int failure = 0; failure < (target == 1 ? 1 : 2); ++failure) {
        jvalue args[6]{};
        args[0].l = failure == 0 ? nullptr : handle;
        args[1].l = failure == 1 ? nullptr : (target == 0 ? receiver
            : group == 0 ? jobject(shorts) : jobject(chars));
        invoke(args);
        jthrowable thrown = env->ExceptionOccurred();
        if (thrown) env->ExceptionClear();
        bool correct = thrown && env->IsInstanceOf(thrown, npe);
        if (thrown) env->DeleteLocalRef(thrown);
        if (!correct) return false;
        ++contract_failures;
      }
      if (target == 2) for (jint bad_index : {jint(-1), jint(6)}) {
        jvalue args[6]{};
        args[0].l = handle;
        args[1].l = group == 0 ? jobject(shorts) : jobject(chars);
        args[2].i = bad_index;
        invoke(args);
        jthrowable thrown = env->ExceptionOccurred();
        if (thrown) env->ExceptionClear();
        bool correct = thrown && env->IsInstanceOf(thrown, bounds);
        if (thrown) env->DeleteLocalRef(thrown);
        if (!correct) return false;
        ++contract_failures;
      }
    }
  }
  env->SetShortField(receiver, short_field, 0);
  env->SetStaticShortField(java_owner, static_short_field, 0);
  env->SetCharField(receiver, char_field, 0);
  env->SetStaticCharField(java_owner, static_char_field, 0);
  for (jobject handle : handles) env->DeleteLocalRef(handle);
  env->DeleteLocalRef(shorts);
  env->DeleteLocalRef(chars);
  env->DeleteLocalRef(short_array_class);
  env->DeleteLocalRef(char_array_class);
  env->DeleteLocalRef(bounds);
  std::cerr << "ART JIT short/char VarHandle: short-operations=" << short_operations
            << " char-operations=" << char_operations
            << " null/bounds-failures=" << contract_failures << " PASS\n";
  return !env->ExceptionCheck();
}
}  // namespace aim_jni_acceptance_phase
