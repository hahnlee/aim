#pragma once
namespace aim_jni_acceptance_phase {
inline bool CheckJitUnresolvedInstance(JNIEnv* env, art::Thread* self, art::jit::Jit* jit,
                                       art::Handle<art::mirror::Class> owner, jclass java_owner) {
  art::jit::ScopedJitSuspend workers;
  auto factory = env->GetStaticMethodID(java_owner, "jitVirtualBaseClass", "()Ljava/lang/Class;");
  if (!factory || env->ExceptionCheck()) return false;
  jclass klass = static_cast<jclass>(env->CallStaticObjectMethod(java_owner, factory));
  if (!klass || env->ExceptionCheck()) return false;
  jobject receiver = env->AllocObject(klass);
  jclass access_error = env->FindClass("java/lang/IllegalAccessError");
  jclass npe = env->FindClass("java/lang/NullPointerException");
  auto gc = env->GetStaticMethodID(java_owner, "jitVoidGc", "()V");
  if (!receiver || !access_error || !npe || !gc || env->ExceptionCheck()) return false;
  const char* names[][2] = {{"jitExternalIntGet", "jitExternalIntSet"}, {"jitExternalRefGet", "jitExternalRefSet"}};
  const char* sigs[][2] = {{"(Ldev/aim/probe/JitVirtualBase;)I", "(Ldev/aim/probe/JitVirtualBase;I)V"},
      {"(Ldev/aim/probe/JitVirtualBase;)Ljava/lang/Object;", "(Ldev/aim/probe/JitVirtualBase;Ljava/lang/Object;)V"}};
  auto check_exception = [&](jclass expected) {
    jthrowable thrown = env->ExceptionOccurred();
    if (thrown) env->ExceptionClear();
    bool correct = thrown && env->IsInstanceOf(thrown, expected);
    if (thrown) env->DeleteLocalRef(thrown);
    return correct;
  };
  for (int kind = 0; kind < 2; ++kind) {
    art::ArtMethod* methods[2]{};
    jmethodID ids[2]{};
    art::ArtField* field = nullptr;
    for (int op = 0; op < 2; ++op) {
      methods[op] = owner->FindClassMethod(names[kind][op], sigs[kind][op], art::kRuntimePointerSize);
      ids[op] = env->GetStaticMethodID(java_owner, names[kind][op], sigs[kind][op]);
      if (!methods[op] || !ids[op] || env->ExceptionCheck()) return false;
      art::CodeItemDataAccessor code(*methods[op]->GetDexFile(), methods[op]->GetCodeItem());
      field = art::Runtime::Current()->GetClassLinker()->ResolveField(
          art::Instruction::At(code.Insns())->VRegC_22c(), methods[op], false);
      if (!field || env->ExceptionCheck()) return false;
    }
    struct Restore { art::ArtField* field; uint32_t flags; ~Restore() { field->SetAccessFlags(flags); } } restore{field, field->GetAccessFlags()};
    auto invoke = [&](int op, jobject object) {
      if (op == 0) {
        if (kind == 0) env->CallStaticIntMethod(java_owner, ids[0], object);
        else { jobject result = env->CallStaticObjectMethod(java_owner, ids[0], object); if (result) env->DeleteLocalRef(result); }
      } else {
        if (kind == 0) env->CallStaticVoidMethod(java_owner, ids[1], object, jint(73));
        else env->CallStaticVoidMethod(java_owner, ids[1], object, java_owner);
      }
    };
    for (auto tier : {art::CompilationKind::kBaseline, art::CompilationKind::kOptimized}) {
      field->SetAccessFlags((restore.flags & ~(art::kAccPublic | art::kAccProtected)) | art::kAccPrivate);
      for (auto* method : methods) {
        if (jit->GetCodeCache()->ContainsPc(method->GetEntryPointFromQuickCompiledCode())) {
          bool removed;
          {
            art::ScopedThreadSuspension suspended(self, art::ThreadState::kSuspended);
            art::gc::ScopedGCCriticalSection critical(self, art::gc::kGcCauseInstrumentation, art::gc::kCollectorTypeInstrumentation);
            art::ScopedSuspendAll all("unresolved instance field recompile");
            removed = jit->GetCodeCache()->RemoveMethod(method, true);
          }
          if (!removed) return false;
        }
        if (!jit->CompileMethod(method, self, tier, false) ||
            !jit->GetCodeCache()->ContainsPc(method->GetEntryPointFromQuickCompiledCode())) return false;
      }
      for (int op = 0; op < 2; ++op) { invoke(op, receiver); if (!check_exception(access_error)) return false; }
      field->SetAccessFlags(restore.flags);
      for (int op = 0; op < 2; ++op) { invoke(op, nullptr); if (!check_exception(npe)) return false; }
      for (int sample = 0; sample < 3; ++sample) {
        jint integer = sample == 0 ? 0x12345678 : sample == 1 ? -17 : 0;
        jobject reference = sample == 0 ? receiver : sample == 1 ? nullptr : java_owner;
        if (kind == 0) env->CallStaticVoidMethod(java_owner, ids[1], receiver, integer);
        else env->CallStaticVoidMethod(java_owner, ids[1], receiver, reference);
        if (env->ExceptionCheck()) return false;
        uint64_t before = art::Runtime::Current()->GetHeap()->GetGcCount();
        env->CallStaticVoidMethod(java_owner, gc);
        if (env->ExceptionCheck() || art::Runtime::Current()->GetHeap()->GetGcCount() <= before) return false;
        bool correct;
        if (kind == 0) correct = env->CallStaticIntMethod(java_owner, ids[0], receiver) == integer;
        else {
          jobject actual = env->CallStaticObjectMethod(java_owner, ids[0], receiver);
          correct = env->IsSameObject(actual, reference);
          if (actual) env->DeleteLocalRef(actual);
        }
        if (!correct || env->ExceptionCheck()) return false;
      }
    }
  }
  env->DeleteLocalRef(receiver); env->DeleteLocalRef(klass);
  env->DeleteLocalRef(access_error); env->DeleteLocalRef(npe);
  std::cerr << "ART JIT unresolved instance fields: access=8 null=8 int/reference recovery+GC PASS cases=12\n";
  return true;
}
}  // namespace aim_jni_acceptance_phase
