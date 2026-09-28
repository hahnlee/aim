#pragma once
namespace aim_jni_acceptance_phase {
inline bool CheckJitSuper(JNIEnv* env, art::Thread* self, art::jit::Jit* jit, jclass java_owner) {
  art::jit::ScopedJitSuspend workers;
  auto factory = env->GetStaticMethodID(java_owner, "jitVirtualChildClass", "()Ljava/lang/Class;");
  if (!factory || env->ExceptionCheck()) return false;
  jclass child = static_cast<jclass>(env->CallStaticObjectMethod(java_owner, factory));
  if (!child || env->ExceptionCheck()) return false;
  jobject receiver = env->AllocObject(child);
  auto number = env->GetFieldID(child, "number", "I");
  auto alternative = env->GetFieldID(child, "alternative", "I");
  auto object = env->GetFieldID(child, "object", "Ljava/lang/Object;");
  auto alt_object = env->GetFieldID(child, "alternativeObject", "Ljava/lang/Object;");
  jclass denied = env->FindClass("java/lang/IllegalAccessError");
  if (!receiver || !number || !alternative || !object || !alt_object || !denied || env->ExceptionCheck()) return false;
  env->SetIntField(receiver, number, 31); env->SetIntField(receiver, alternative, 79);
  env->SetObjectField(receiver, object, java_owner); env->SetObjectField(receiver, alt_object, receiver);
  { art::ScopedThreadStateChange native(self, art::ThreadState::kNative);
    art::Runtime::Current()->GetClassLinker()->MakeInitializedClassesVisiblyInitialized(self, true); }
  const char* names[] = {"superValue", "superReference"};
  const char* sigs[] = {"()I", "()Ljava/lang/Object;"};
  for (int kind = 0; kind < 2; ++kind) {
    auto id = env->GetMethodID(child, names[kind], sigs[kind]);
    auto* method = self->DecodeJObject(child)->AsClass()->FindClassMethod(names[kind], sigs[kind], art::kRuntimePointerSize);
    if (!id || !method || env->ExceptionCheck()) return false;
    art::CodeItemDataAccessor code(*method->GetDexFile(), method->GetCodeItem());
    if (art::Instruction::At(code.Insns())->Opcode() != art::Instruction::INVOKE_SUPER) return false;
    auto invoke = [&]() {
      if (kind == 0) return env->CallIntMethod(receiver, id) == 31;
      jobject result = env->CallObjectMethod(receiver, id);
      bool correct = !env->ExceptionCheck() && env->IsSameObject(result, java_owner);
      if (result) env->DeleteLocalRef(result);
      return correct;
    };
    if (!invoke() || env->ExceptionCheck()) return false;
    auto* target = art::jit::DarwinJitLookupResolvedMethod(method, art::Instruction::At(code.Insns())->VRegB_35c());
    if (!target) return false;
    struct Restore { art::ArtMethod* method; uint32_t flags; ~Restore() { method->SetAccessFlags(flags); } } restore{target, target->GetAccessFlags()};
    Restore restore_caller{method, method->GetAccessFlags()};
    for (int unresolved = 0; unresolved < 2; ++unresolved) {
      for (auto tier : {art::CompilationKind::kBaseline, art::CompilationKind::kOptimized}) {
        if (jit->GetCodeCache()->ContainsPc(method->GetEntryPointFromQuickCompiledCode())) {
          bool removed;
          { art::ScopedThreadSuspension suspended(self, art::ThreadState::kSuspended);
            art::gc::ScopedGCCriticalSection critical(self, art::gc::kGcCauseInstrumentation, art::gc::kCollectorTypeInstrumentation);
            art::ScopedSuspendAll all("invoke-super recompile acceptance");
            removed = jit->GetCodeCache()->RemoveMethod(method, true); }
          if (!removed) return false;
        }
        if (unresolved) {
          target->SetAccessFlags((restore.flags & ~(art::kAccPublic | art::kAccProtected)) | art::kAccPrivate);
          // Fault injection invalidates the verifier's earlier access-check elision.
          method->SetAccessFlags(method->GetAccessFlags() & ~art::kAccSkipAccessChecks);
        }
        self->GetInterpreterCache()->Clear(self);
        if (!jit->CompileMethod(method, self, tier, false) ||
            !jit->GetCodeCache()->ContainsPc(method->GetEntryPointFromQuickCompiledCode())) {
          std::cerr << "ART JIT invoke-super compile failed kind=" << kind << " unresolved=" << unresolved << "\n";
          return false;
        }
        if (unresolved) {
          invoke();
          jthrowable thrown = env->ExceptionOccurred();
          if (thrown) env->ExceptionClear();
          bool correct = thrown && env->IsInstanceOf(thrown, denied);
          if (thrown) env->DeleteLocalRef(thrown);
          target->SetAccessFlags(restore.flags);
          if (!correct) {
            std::cerr << "ART JIT invoke-super access check failed kind=" << kind << "\n";
            return false;
          }
        }
        if (!invoke() || env->ExceptionCheck()) {
          std::cerr << "ART JIT invoke-super parent result failed kind=" << kind << " unresolved=" << unresolved << "\n";
          return false;
        }
      }
    }
  }
  const char* range_sig = "(IIIIIIIJDLjava/lang/Object;)J";
  auto range_id = env->GetMethodID(child, "superRange", range_sig);
  auto* range = self->DecodeJObject(child)->AsClass()->FindClassMethod("superRange", range_sig, art::kRuntimePointerSize);
  if (!range_id || !range || env->ExceptionCheck()) return false;
  art::CodeItemDataAccessor range_code(*range->GetDexFile(), range->GetCodeItem());
  if (art::Instruction::At(range_code.Insns())->Opcode() != art::Instruction::INVOKE_SUPER_RANGE) return false;
  auto* range_target = art::jit::DarwinJitLookupResolvedMethod(range, art::Instruction::At(range_code.Insns())->VRegB_3rc());
  if (!range_target) return false;
  struct RestoreRange { art::ArtMethod* method; uint32_t flags; ~RestoreRange() { method->SetAccessFlags(flags); } };
  RestoreRange restore_range{range, range->GetAccessFlags()};
  RestoreRange restore_range_target{range_target, range_target->GetAccessFlags()};
  unsigned range_cases = 0;
  for (int phase = 0; phase < 5; ++phase) {
    if (phase >= 3) {
      bool removed;
      { art::ScopedThreadSuspension suspended(self, art::ThreadState::kSuspended);
        art::gc::ScopedGCCriticalSection critical(self, art::gc::kGcCauseInstrumentation, art::gc::kCollectorTypeInstrumentation);
        art::ScopedSuspendAll all("unresolved super-range acceptance");
        removed = jit->GetCodeCache()->RemoveMethod(range, true); }
      if (!removed) return false;
      range_target->SetAccessFlags((restore_range_target.flags & ~(art::kAccPublic | art::kAccProtected)) | art::kAccPrivate);
      range->SetAccessFlags(range->GetAccessFlags() & ~art::kAccSkipAccessChecks);
      self->GetInterpreterCache()->Clear(self);
    }
    if (phase && (!jit->CompileMethod(range, self, phase % 2 == 1 ? art::CompilationKind::kBaseline : art::CompilationKind::kOptimized, false) ||
        !jit->GetCodeCache()->ContainsPc(range->GetEntryPointFromQuickCompiledCode()))) return false;
    if (phase >= 3) {
      jvalue denied_args[10]{};
      env->CallLongMethodA(receiver, range_id, denied_args);
      jthrowable thrown = env->ExceptionOccurred();
      if (thrown) env->ExceptionClear();
      bool correct = thrown && env->IsInstanceOf(thrown, denied);
      if (thrown) env->DeleteLocalRef(thrown);
      range_target->SetAccessFlags(restore_range_target.flags);
      if (!correct) return false;
    }
    for (jlong wide : {jlong(INT64_C(0x123456789abcdef)), jlong(-INT64_C(0x123456789abcdef))})
      for (double real : {-1234.5, 0.0})
        for (jobject reference : {jobject(nullptr), jobject(java_owner)})
          for (jint last : {jint(77), jint(78)}) {
            jvalue args[10]{};
            for (int i = 0; i < 6; ++i) args[i].i = (i + 1) * 11;
            args[6].i = last; args[7].j = wide; args[8].d = real; args[9].l = reference;
            uint64_t before = art::Runtime::Current()->GetHeap()->GetGcCount();
            jlong actual = env->CallLongMethodA(receiver, range_id, args);
            jlong expected = last == 77 && real == -1234.5 && reference ? wide : -1;
            if (env->ExceptionCheck() || actual != expected ||
                art::Runtime::Current()->GetHeap()->GetGcCount() <= before) {
              std::cerr << "ART JIT super-range failed phase=" << phase << " actual=" << actual << " expected=" << expected << "\n";
              return false;
            }
            ++range_cases;
          }
  }
  env->DeleteLocalRef(receiver); env->DeleteLocalRef(child); env->DeleteLocalRef(denied);
  std::cerr << "ART JIT invoke-super: parent-not-override primitive/reference interpreter=2 JIT=8 access=4 PASS\n";
  std::cerr << "ART JIT invoke-super/range: stack/wide/FP/reference/parent selection/callee-GC PASS cases=" << range_cases << "\n";
  return true;
}
}  // namespace aim_jni_acceptance_phase
