#pragma once
namespace aim_jni_acceptance_phase {
inline bool CheckJitVarHandleNarrow8(JNIEnv* env, art::Thread* self, art::jit::Jit* jit,
    art::Handle<art::mirror::Class> owner, jclass java_owner, jobject receiver, jclass npe) {
  auto byte_factory = env->GetStaticMethodID(
      java_owner, "jitVarByteHandle", "(Z)Ljava/lang/invoke/VarHandle;");
  auto boolean_factory = env->GetStaticMethodID(
      java_owner, "jitVarBooleanHandle", "(Z)Ljava/lang/invoke/VarHandle;");
  auto array_factory = env->GetStaticMethodID(
      java_owner, "jitVarArrayHandle", "(Ljava/lang/Class;)Ljava/lang/invoke/VarHandle;");
  jclass byte_array_class = env->FindClass("[B");
  jclass boolean_array_class = env->FindClass("[Z");
  jobject handles[6] = {
      byte_factory ? env->CallStaticObjectMethod(java_owner, byte_factory, JNI_FALSE) : nullptr,
      byte_factory ? env->CallStaticObjectMethod(java_owner, byte_factory, JNI_TRUE) : nullptr,
      array_factory ? env->CallStaticObjectMethod(java_owner, array_factory, byte_array_class) : nullptr,
      boolean_factory ? env->CallStaticObjectMethod(java_owner, boolean_factory, JNI_FALSE) : nullptr,
      boolean_factory ? env->CallStaticObjectMethod(java_owner, boolean_factory, JNI_TRUE) : nullptr,
      array_factory ? env->CallStaticObjectMethod(java_owner, array_factory, boolean_array_class) : nullptr};
  const char* names[] = {
      "jitVarByteGet", "jitVarByteSet", "jitVarByteCas", "jitVarByteAdd",
      "jitVarStaticByteGet", "jitVarStaticByteSet", "jitVarStaticByteCas", "jitVarStaticByteAdd",
      "jitVarByteArrayGet", "jitVarByteArraySet", "jitVarByteArrayCas", "jitVarByteArrayAdd",
      "jitVarBooleanGet", "jitVarBooleanSet", "jitVarBooleanCas", "jitVarBooleanXor",
      "jitVarStaticBooleanGet", "jitVarStaticBooleanSet", "jitVarStaticBooleanCas",
      "jitVarStaticBooleanXor", "jitVarBooleanArrayGet", "jitVarBooleanArraySet",
      "jitVarBooleanArrayCas", "jitVarBooleanArrayXor"};
  const char* sigs[] = {
      "(Ljava/lang/invoke/VarHandle;Ldev/aim/probe/Hello;)B",
      "(Ljava/lang/invoke/VarHandle;Ldev/aim/probe/Hello;B)V",
      "(Ljava/lang/invoke/VarHandle;Ldev/aim/probe/Hello;BB)Z",
      "(Ljava/lang/invoke/VarHandle;Ldev/aim/probe/Hello;B)B",
      "(Ljava/lang/invoke/VarHandle;)B", "(Ljava/lang/invoke/VarHandle;B)V",
      "(Ljava/lang/invoke/VarHandle;BB)Z", "(Ljava/lang/invoke/VarHandle;B)B",
      "(Ljava/lang/invoke/VarHandle;[BI)B", "(Ljava/lang/invoke/VarHandle;[BIB)V",
      "(Ljava/lang/invoke/VarHandle;[BIBB)Z", "(Ljava/lang/invoke/VarHandle;[BIB)B",
      "(Ljava/lang/invoke/VarHandle;Ldev/aim/probe/Hello;)Z",
      "(Ljava/lang/invoke/VarHandle;Ldev/aim/probe/Hello;Z)V",
      "(Ljava/lang/invoke/VarHandle;Ldev/aim/probe/Hello;ZZ)Z",
      "(Ljava/lang/invoke/VarHandle;Ldev/aim/probe/Hello;Z)Z",
      "(Ljava/lang/invoke/VarHandle;)Z", "(Ljava/lang/invoke/VarHandle;Z)V",
      "(Ljava/lang/invoke/VarHandle;ZZ)Z", "(Ljava/lang/invoke/VarHandle;Z)Z",
      "(Ljava/lang/invoke/VarHandle;[ZI)Z", "(Ljava/lang/invoke/VarHandle;[ZIZ)V",
      "(Ljava/lang/invoke/VarHandle;[ZIZZ)Z", "(Ljava/lang/invoke/VarHandle;[ZIZ)Z"};
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
  auto byte_field = env->GetFieldID(java_owner, "jitVarByte", "B");
  auto static_byte_field = env->GetStaticFieldID(java_owner, "jitVarStaticByte", "B");
  auto boolean_field = env->GetFieldID(java_owner, "jitVarBoolean", "Z");
  auto static_boolean_field = env->GetStaticFieldID(java_owner, "jitVarStaticBoolean", "Z");
  jbyteArray bytes = env->NewByteArray(6);
  jbooleanArray booleans = env->NewBooleanArray(2);
  jclass bounds = env->FindClass("java/lang/ArrayIndexOutOfBoundsException");
  if (!byte_field || !static_byte_field || !boolean_field || !static_boolean_field ||
      !bytes || !booleans || !bounds || env->ExceptionCheck()) return false;
  auto force_gc = [&]() {
    art::ScopedThreadStateChange native(self, art::ThreadState::kNative);
    art::Runtime::Current()->GetHeap()->CollectGarbage(false);
  };
  int byte_operations = 0;
  int boolean_operations = 0;
  int contract_failures = 0;
  for (int phase = 0; phase < 3; ++phase) {
    if (phase) for (auto* method : methods) {
      if (!jit->CompileMethod(method, self,
          phase == 1 ? art::CompilationKind::kBaseline : art::CompilationKind::kOptimized, false) ||
          !jit->GetCodeCache()->ContainsPc(method->GetEntryPointFromQuickCompiledCode())) {
        std::cerr << "ART JIT narrow8 VarHandle compile failed phase=" << phase << "\n";
        return false;
      }
    }
    const jbyte byte_values[] = {jbyte(0), jbyte(1), jbyte(-1), jbyte(-128), jbyte(127), jbyte(0x5a)};
    for (int target = 0; target < 3; ++target) for (jint index = 0; index < 6; ++index) {
      int base = target * 4;
      jbyte seed = byte_values[index];
      jbyte update = jbyte(static_cast<uint8_t>(seed) ^ UINT8_C(0xa5));
      auto arguments = [&](jbyte first, jbyte second) {
        std::array<jvalue, 5> args{};
        args[0].l = handles[target];
        if (target == 0) {
          args[1].l = receiver; args[2].b = first; args[3].b = second;
        } else if (target == 1) {
          args[1].b = first; args[2].b = second;
        } else {
          args[1].l = bytes; args[2].i = index; args[3].b = first; args[4].b = second;
        }
        return args;
      };
      auto read_target = [&]() {
        jbyte value = target == 0 ? env->GetByteField(receiver, byte_field)
            : target == 1 ? env->GetStaticByteField(java_owner, static_byte_field) : jbyte(0);
        if (target == 2) env->GetByteArrayRegion(bytes, index, 1, &value);
        return value;
      };
      auto write_target = [&](jbyte value) {
        if (target == 0) env->SetByteField(receiver, byte_field, value);
        else if (target == 1) env->SetStaticByteField(java_owner, static_byte_field, value);
        else env->SetByteArrayRegion(bytes, index, 1, &value);
      };
      auto args = arguments(seed, 0);
      env->CallStaticVoidMethodA(java_owner, ids[base + 1], args.data());
      if (env->ExceptionCheck() || read_target() != seed) return false;
      force_gc();
      args = arguments(0, 0);
      if (env->CallStaticByteMethodA(java_owner, ids[base], args.data()) != seed ||
          env->ExceptionCheck()) return false;
      ++byte_operations;
      args = arguments(jbyte(seed ^ 1), update);
      if (env->CallStaticBooleanMethodA(java_owner, ids[base + 2], args.data()) != JNI_FALSE ||
          read_target() != seed || env->ExceptionCheck()) return false;
      ++byte_operations;
      args = arguments(seed, update);
      if (env->CallStaticBooleanMethodA(java_owner, ids[base + 2], args.data()) != JNI_TRUE ||
          read_target() != update || env->ExceptionCheck()) return false;
      ++byte_operations;
      constexpr jbyte kDelta = 37;
      args = arguments(kDelta, 0);
      jbyte expected = jbyte(static_cast<uint8_t>(update) + static_cast<uint8_t>(kDelta));
      if (env->CallStaticByteMethodA(java_owner, ids[base + 3], args.data()) != update ||
          read_target() != expected || env->ExceptionCheck()) return false;
      ++byte_operations;
      write_target(seed);
      args = arguments(0, 0);
      if (env->CallStaticByteMethodA(java_owner, ids[base], args.data()) != seed ||
          env->ExceptionCheck()) return false;
      ++byte_operations;
    }
    for (int target = 0; target < 3; ++target) for (jint raw = 0; raw < 2; ++raw) {
      int base = 12 + target * 4;
      jboolean seed = raw ? JNI_TRUE : JNI_FALSE;
      jboolean update = raw ? JNI_FALSE : JNI_TRUE;
      auto arguments = [&](jboolean first, jboolean second) {
        std::array<jvalue, 5> args{};
        args[0].l = handles[3 + target];
        if (target == 0) {
          args[1].l = receiver; args[2].z = first; args[3].z = second;
        } else if (target == 1) {
          args[1].z = first; args[2].z = second;
        } else {
          args[1].l = booleans; args[2].i = raw; args[3].z = first; args[4].z = second;
        }
        return args;
      };
      auto read_target = [&]() {
        jboolean value = target == 0 ? env->GetBooleanField(receiver, boolean_field)
            : target == 1 ? env->GetStaticBooleanField(java_owner, static_boolean_field) : JNI_FALSE;
        if (target == 2) env->GetBooleanArrayRegion(booleans, raw, 1, &value);
        return value;
      };
      auto write_target = [&](jboolean value) {
        if (target == 0) env->SetBooleanField(receiver, boolean_field, value);
        else if (target == 1) env->SetStaticBooleanField(java_owner, static_boolean_field, value);
        else env->SetBooleanArrayRegion(booleans, raw, 1, &value);
      };
      auto args = arguments(seed, JNI_FALSE);
      env->CallStaticVoidMethodA(java_owner, ids[base + 1], args.data());
      if (env->ExceptionCheck() || read_target() != seed) return false;
      force_gc();
      args = arguments(JNI_FALSE, JNI_FALSE);
      if (env->CallStaticBooleanMethodA(java_owner, ids[base], args.data()) != seed ||
          env->ExceptionCheck()) return false;
      ++boolean_operations;
      args = arguments(update, update);
      if (env->CallStaticBooleanMethodA(java_owner, ids[base + 2], args.data()) != JNI_FALSE ||
          read_target() != seed || env->ExceptionCheck()) return false;
      ++boolean_operations;
      args = arguments(seed, update);
      if (env->CallStaticBooleanMethodA(java_owner, ids[base + 2], args.data()) != JNI_TRUE ||
          read_target() != update || env->ExceptionCheck()) return false;
      ++boolean_operations;
      args = arguments(JNI_TRUE, JNI_FALSE);
      if (env->CallStaticBooleanMethodA(java_owner, ids[base + 3], args.data()) != update ||
          read_target() != seed || env->ExceptionCheck()) return false;
      ++boolean_operations;
      write_target(update);
      args = arguments(JNI_FALSE, JNI_FALSE);
      if (env->CallStaticBooleanMethodA(java_owner, ids[base], args.data()) != update ||
          env->ExceptionCheck()) return false;
      ++boolean_operations;
    }
    for (int kind = 0; kind < 24; ++kind) {
      int group = kind / 12;
      int target = (kind % 12) / 4;
      int operation = kind % 4;
      jobject handle = handles[group * 3 + target];
      for (int failure = 0; failure < (target == 1 ? 1 : 2); ++failure) {
        jvalue args[6]{};
        args[0].l = failure == 0 ? nullptr : handle;
        args[1].l = failure == 1 ? nullptr : (target == 0 ? receiver
            : group == 0 ? jobject(bytes) : jobject(booleans));
        if (operation == 1) env->CallStaticVoidMethodA(java_owner, ids[kind], args);
        else if (operation == 2 || group == 1) env->CallStaticBooleanMethodA(java_owner, ids[kind], args);
        else env->CallStaticByteMethodA(java_owner, ids[kind], args);
        jthrowable thrown = env->ExceptionOccurred();
        if (thrown) env->ExceptionClear();
        bool correct = thrown && env->IsInstanceOf(thrown, npe);
        if (thrown) env->DeleteLocalRef(thrown);
        if (!correct) return false;
        ++contract_failures;
      }
      if (target == 2) for (jint bad_index : {jint(-1), jint(group == 0 ? 6 : 2)}) {
        jvalue args[6]{};
        args[0].l = handle;
        args[1].l = group == 0 ? jobject(bytes) : jobject(booleans);
        args[2].i = bad_index;
        if (operation == 1) env->CallStaticVoidMethodA(java_owner, ids[kind], args);
        else if (operation == 2 || group == 1) env->CallStaticBooleanMethodA(java_owner, ids[kind], args);
        else env->CallStaticByteMethodA(java_owner, ids[kind], args);
        jthrowable thrown = env->ExceptionOccurred();
        if (thrown) env->ExceptionClear();
        bool correct = thrown && env->IsInstanceOf(thrown, bounds);
        if (thrown) env->DeleteLocalRef(thrown);
        if (!correct) return false;
        ++contract_failures;
      }
    }
  }
  env->SetByteField(receiver, byte_field, 0);
  env->SetStaticByteField(java_owner, static_byte_field, 0);
  env->SetBooleanField(receiver, boolean_field, JNI_FALSE);
  env->SetStaticBooleanField(java_owner, static_boolean_field, JNI_FALSE);
  for (jobject handle : handles) env->DeleteLocalRef(handle);
  env->DeleteLocalRef(bytes);
  env->DeleteLocalRef(booleans);
  env->DeleteLocalRef(byte_array_class);
  env->DeleteLocalRef(boolean_array_class);
  env->DeleteLocalRef(bounds);
  std::cerr << "ART JIT byte/boolean VarHandle: byte-operations=" << byte_operations
            << " boolean-operations=" << boolean_operations
            << " null/bounds-failures=" << contract_failures << " PASS\n";
  return !env->ExceptionCheck();
}
}  // namespace aim_jni_acceptance_phase
