#pragma once
namespace aim_jni_acceptance_phase {
inline bool CheckJitVarHandleReference(JNIEnv* env, art::Thread* self, art::jit::Jit* jit,
    art::Handle<art::mirror::Class> owner, jclass java_owner, jobject receiver, jclass npe) {
  auto factory = env->GetStaticMethodID(java_owner, "jitVarReferenceHandle", "()Ljava/lang/invoke/VarHandle;");
  jobject handle = factory ? env->CallStaticObjectMethod(java_owner, factory) : nullptr;
  const char* names[] = {"jitVarReferenceGet", "jitVarReferenceSet", "jitVarReferenceCas",
                         "jitVarReferenceExchange", "jitVarReferenceSwap"};
  const char* sigs[] = {
      "(Ljava/lang/invoke/VarHandle;Ldev/aim/probe/Hello;)Ljava/lang/Object;",
      "(Ljava/lang/invoke/VarHandle;Ldev/aim/probe/Hello;Ljava/lang/Object;)V",
      "(Ljava/lang/invoke/VarHandle;Ldev/aim/probe/Hello;Ljava/lang/Object;Ljava/lang/Object;)Z",
      "(Ljava/lang/invoke/VarHandle;Ldev/aim/probe/Hello;Ljava/lang/Object;Ljava/lang/Object;)Ljava/lang/Object;",
      "(Ljava/lang/invoke/VarHandle;Ldev/aim/probe/Hello;Ljava/lang/Object;)Ljava/lang/Object;"};
  jmethodID ids[5]{};
  art::ArtMethod* methods[5]{};
  for (int kind = 0; kind < 5; ++kind) {
    ids[kind] = env->GetStaticMethodID(java_owner, names[kind], sigs[kind]);
    methods[kind] = owner->FindClassMethod(names[kind], sigs[kind], art::kRuntimePointerSize);
    if (!handle || !ids[kind] || !methods[kind] || env->ExceptionCheck()) return false;
    art::CodeItemDataAccessor code(*methods[kind]->GetDexFile(), methods[kind]->GetCodeItem());
    if (art::Instruction::At(code.Insns())->Opcode() != art::Instruction::INVOKE_POLYMORPHIC) return false;
  }
  auto field = env->GetFieldID(java_owner, "jitVarReference", "Ljava/lang/Object;");
  if (!field || env->ExceptionCheck()) return false;
  for (int phase = 0; phase < 3; ++phase) {
    if (phase) for (auto* method : methods) {
      if (!jit->CompileMethod(method, self,
          phase == 1 ? art::CompilationKind::kBaseline : art::CompilationKind::kOptimized, false) ||
          !jit->GetCodeCache()->ContainsPc(method->GetEntryPointFromQuickCompiledCode())) return false;
    }
    for (jobject input : {jobject(nullptr), receiver, jobject(java_owner)}) {
      jobject replacement = env->IsSameObject(input, java_owner) ? receiver : java_owner;
      auto check = [&](jobject expected, const char* operation) {
        { art::ScopedThreadStateChange native(self, art::ThreadState::kNative);
          art::Runtime::Current()->GetHeap()->CollectGarbage(false); }
        jobject independent = env->GetObjectField(receiver, field);
        jobject result = env->CallStaticObjectMethod(java_owner, ids[0], handle, receiver);
        bool correct = !env->ExceptionCheck() && env->IsSameObject(independent, expected) &&
            env->IsSameObject(result, expected);
        if (independent) env->DeleteLocalRef(independent);
        if (result) env->DeleteLocalRef(result);
        if (!correct) {
          std::cerr << "ART JIT reference VarHandle failed op=" << operation
                    << " phase=" << phase << "\n";
          env->ExceptionDescribe();
        }
        return correct;
      };
      env->CallStaticVoidMethod(java_owner, ids[1], handle, receiver, input);
      if (!check(input, "set/get")) return false;
      if (env->CallStaticBooleanMethod(java_owner, ids[2], handle, receiver, replacement, input) != JNI_FALSE ||
          !check(input, "CAS mismatch")) return false;
      if (env->CallStaticBooleanMethod(java_owner, ids[2], handle, receiver, input, replacement) != JNI_TRUE ||
          !check(replacement, "CAS success")) return false;
      jobject result = env->CallStaticObjectMethod(java_owner, ids[3], handle, receiver, input, input);
      bool correct = !env->ExceptionCheck() && env->IsSameObject(result, replacement);
      if (result) env->DeleteLocalRef(result);
      if (!correct || !check(replacement, "exchange mismatch")) return false;
      result = env->CallStaticObjectMethod(java_owner, ids[3], handle, receiver, replacement, input);
      correct = !env->ExceptionCheck() && env->IsSameObject(result, replacement);
      if (result) env->DeleteLocalRef(result);
      if (!correct || !check(input, "exchange success")) return false;
      result = env->CallStaticObjectMethod(java_owner, ids[4], handle, receiver, replacement);
      correct = !env->ExceptionCheck() && env->IsSameObject(result, input);
      if (result) env->DeleteLocalRef(result);
      if (!correct || !check(replacement, "get-and-set")) return false;
    }
    for (int kind = 0; kind < 5; ++kind) for (int null_handle = 0; null_handle < 2; ++null_handle) {
      jvalue args[4]{};
      args[0].l = null_handle ? nullptr : handle;
      args[1].l = null_handle ? receiver : nullptr;
      if (kind == 0 || kind >= 3) env->CallStaticObjectMethodA(java_owner, ids[kind], args);
      else if (kind == 1) env->CallStaticVoidMethodA(java_owner, ids[kind], args);
      else env->CallStaticBooleanMethodA(java_owner, ids[kind], args);
      jthrowable thrown = env->ExceptionOccurred();
      if (thrown) env->ExceptionClear();
      bool correct = thrown && env->IsInstanceOf(thrown, npe);
      if (thrown) env->DeleteLocalRef(thrown);
      if (!correct) return false;
    }
  }
  env->SetObjectField(receiver, field, nullptr);
  env->DeleteLocalRef(handle);
  std::cerr << "ART JIT VarHandle reference: set/get/CAS/exchange/swap+GC=54 null-failures=30 PASS\n";
  return !env->ExceptionCheck();
}
}  // namespace aim_jni_acceptance_phase
