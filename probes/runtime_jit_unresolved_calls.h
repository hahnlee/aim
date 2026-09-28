#pragma once
namespace aim_jni_acceptance_phase {
inline bool CheckJitUnresolvedCalls(JNIEnv* env, art::Thread* self, art::jit::Jit* jit,
                                    art::Handle<art::mirror::Class> owner, jclass java_owner) {
  art::jit::ScopedJitSuspend workers;
  jclass absent = env->FindClass("java/lang/NoClassDefFoundError");
  jclass denied = env->FindClass("java/lang/IllegalAccessError");
  if (!absent || !denied || env->ExceptionCheck()) return false;
  auto check_exception = [&](jclass expected) {
    jthrowable thrown = env->ExceptionOccurred();
    if (thrown) env->ExceptionClear();
    bool correct = thrown && env->IsInstanceOf(thrown, expected);
    if (thrown) env->DeleteLocalRef(thrown);
    return correct;
  };
  const char* names[] = {"jitMissingStaticCall", "jitMissingReferenceCall"};
  const char* sigs[] = {"()I", "(Ljava/lang/Object;)Ljava/lang/Object;"};
  for (int kind = 0; kind < 2; ++kind) {
    auto id = env->GetStaticMethodID(java_owner, names[kind], sigs[kind]);
    auto* method = owner->FindClassMethod(names[kind], sigs[kind], art::kRuntimePointerSize);
    if (!id || !method || env->ExceptionCheck()) return false;
    for (int phase = 0; phase < 3; ++phase) {
      if (phase && (!jit->CompileMethod(method, self, phase == 1 ? art::CompilationKind::kBaseline : art::CompilationKind::kOptimized, false) ||
          !jit->GetCodeCache()->ContainsPc(method->GetEntryPointFromQuickCompiledCode()))) return false;
      if (kind == 0) env->CallStaticIntMethod(java_owner, id);
      else env->CallStaticObjectMethod(java_owner, id, java_owner);
      if (!check_exception(absent)) return false;
    }
  }
  auto* caller = owner->FindClassMethod("jitFirstInitialization", "()I", art::kRuntimePointerSize);
  auto caller_id = env->GetStaticMethodID(java_owner, "jitFirstInitialization", "()I");
  if (!caller || !caller_id || env->ExceptionCheck()) return false;
  art::CodeItemDataAccessor code(*caller->GetDexFile(), caller->GetCodeItem());
  auto* target = art::jit::DarwinJitLookupResolvedMethod(caller, art::Instruction::At(code.Insns())->VRegB_35c());
  if (!target || !target->IsStatic()) return false;
  struct Restore { art::ArtMethod* target; uint32_t flags; ~Restore() { target->SetAccessFlags(flags); } } restore{target, target->GetAccessFlags()};
  for (auto tier : {art::CompilationKind::kBaseline, art::CompilationKind::kOptimized}) {
    if (jit->GetCodeCache()->ContainsPc(caller->GetEntryPointFromQuickCompiledCode())) {
      bool removed;
      {
        art::ScopedThreadSuspension suspended(self, art::ThreadState::kSuspended);
        art::gc::ScopedGCCriticalSection critical(self, art::gc::kGcCauseInstrumentation, art::gc::kCollectorTypeInstrumentation);
        art::ScopedSuspendAll all("unresolved static invoke acceptance");
        removed = jit->GetCodeCache()->RemoveMethod(caller, true);
      }
      if (!removed) return false;
    }
    target->SetAccessFlags((restore.flags & ~(art::kAccPublic | art::kAccProtected)) | art::kAccPrivate);
    self->GetInterpreterCache()->Clear(self);  // Test-only access mutation invalidates cached resolution.
    if (!jit->CompileMethod(caller, self, tier, false) ||
        !jit->GetCodeCache()->ContainsPc(caller->GetEntryPointFromQuickCompiledCode())) return false;
    env->CallStaticIntMethod(java_owner, caller_id);
    if (!check_exception(denied)) return false;
    target->SetAccessFlags(restore.flags);
    if (env->CallStaticIntMethod(java_owner, caller_id) != 42 || env->ExceptionCheck()) return false;
  }
  auto factory = env->GetStaticMethodID(java_owner, "jitVirtualBaseClass", "()Ljava/lang/Class;");
  if (!factory || env->ExceptionCheck()) return false;
  jclass base = static_cast<jclass>(env->CallStaticObjectMethod(java_owner, factory));
  if (!base || env->ExceptionCheck()) return false;
  jobject receiver = env->AllocObject(base);
  auto number = env->GetFieldID(base, "number", "I");
  auto object = env->GetFieldID(base, "object", "Ljava/lang/Object;");
  jclass npe = env->FindClass("java/lang/NullPointerException");
  if (!receiver || !number || !object || !npe || env->ExceptionCheck()) return false;
  env->SetIntField(receiver, number, 73);
  env->SetObjectField(receiver, object, java_owner);
  const char* virtual_names[] = {"jitVirtualValue", "jitVirtualReference", "jitInterfaceValue", "jitInterfaceReference"};
  const char* virtual_sigs[] = {"(Ldev/aim/probe/JitVirtualBase;)I", "(Ldev/aim/probe/JitVirtualBase;)Ljava/lang/Object;",
      "(Ldev/aim/probe/JitCallable;)I", "(Ldev/aim/probe/JitCallable;)Ljava/lang/Object;"};
  for (int kind = 0; kind < 4; ++kind) {
    auto* method = owner->FindClassMethod(virtual_names[kind], virtual_sigs[kind], art::kRuntimePointerSize);
    auto id = env->GetStaticMethodID(java_owner, virtual_names[kind], virtual_sigs[kind]);
    if (!method || !id || env->ExceptionCheck()) return false;
    art::CodeItemDataAccessor body(*method->GetDexFile(), method->GetCodeItem());
    auto* callee = art::jit::DarwinJitLookupResolvedMethod(method, art::Instruction::At(body.Insns())->VRegB_35c());
    if (!callee || callee->IsStatic()) return false;
    Restore restore_virtual{callee, callee->GetAccessFlags()};
    auto invoke = [&](jobject input) {
      if (kind % 2 == 0) return env->CallStaticIntMethod(java_owner, id, input) == 73;
      jobject result = env->CallStaticObjectMethod(java_owner, id, input);
      bool correct = !env->ExceptionCheck() && env->IsSameObject(result, java_owner);
      if (result) env->DeleteLocalRef(result);
      return correct;
    };
    for (auto tier : {art::CompilationKind::kBaseline, art::CompilationKind::kOptimized}) {
      if (jit->GetCodeCache()->ContainsPc(method->GetEntryPointFromQuickCompiledCode())) {
        bool removed;
        {
          art::ScopedThreadSuspension suspended(self, art::ThreadState::kSuspended);
          art::gc::ScopedGCCriticalSection critical(self, art::gc::kGcCauseInstrumentation, art::gc::kCollectorTypeInstrumentation);
          art::ScopedSuspendAll all("unresolved virtual invoke acceptance");
          removed = jit->GetCodeCache()->RemoveMethod(method, true);
        }
        if (!removed) return false;
      }
      callee->SetAccessFlags((restore_virtual.flags & ~(art::kAccPublic | art::kAccProtected)) | art::kAccPrivate);
      self->GetInterpreterCache()->Clear(self);
      if (!jit->CompileMethod(method, self, tier, false) ||
          !jit->GetCodeCache()->ContainsPc(method->GetEntryPointFromQuickCompiledCode())) return false;
      invoke(receiver);
      if (!check_exception(denied)) {
        std::cerr << "ART JIT unresolved invocation access check failed kind=" << kind << "\n";
        return false;
      }
      callee->SetAccessFlags(restore_virtual.flags);
      invoke(nullptr);
      if (!check_exception(npe)) {
        std::cerr << "ART JIT unresolved invocation null check failed kind=" << kind << "\n";
        return false;
      }
      if (!invoke(receiver) || env->ExceptionCheck()) {
        std::cerr << "ART JIT unresolved invocation recovery failed kind=" << kind << "\n";
        return false;
      }
    }
  }
  env->DeleteLocalRef(receiver); env->DeleteLocalRef(base); env->DeleteLocalRef(npe);
  env->DeleteLocalRef(absent); env->DeleteLocalRef(denied);
  std::cerr << "ART JIT unresolved static invokes: missing owner=6 access=2 same-code recovery=2 PASS\n";
  std::cerr << "ART JIT unresolved virtual/interface invokes: primitive/reference access=8 null=8 same-code recovery=8 PASS\n";
  return true;
}
}  // namespace aim_jni_acceptance_phase
