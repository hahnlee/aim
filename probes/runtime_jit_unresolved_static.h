#pragma once
namespace aim_jni_acceptance_phase {
inline bool CheckJitUnresolvedStatic(JNIEnv* env, art::Thread* self, art::jit::Jit* jit,
                                     art::Handle<art::mirror::Class> owner, jclass java_owner) {
  art::jit::ScopedJitSuspend workers;
  const char* names[][2] = {{"jitColdLongSet", "jitColdLongGet"},
                          {"jitColdDoubleSet", "jitColdDoubleGet"},
                          {"jitColdReferenceSet", "jitColdReferenceGet"}};
  const char* sigs[][2] = {{"(J)V", "()J"}, {"(D)V", "()D"},
                         {"(Ljava/lang/Object;)V", "()Ljava/lang/Object;"}};
  jclass access_error = env->FindClass("java/lang/IllegalAccessError");
  auto gc = env->GetStaticMethodID(java_owner, "jitVoidGc", "()V");
  jobject value = env->AllocObject(java_owner);
  if (!access_error || !gc || !value || env->ExceptionCheck()) return false;
  for (int kind = 0; kind < 3; ++kind) {
    art::ArtMethod* methods[2]{};
    jmethodID ids[2]{};
    art::ArtField* field = nullptr;
    for (int op = 0; op < 2; ++op) {
      methods[op] = owner->FindClassMethod(names[kind][op], sigs[kind][op], art::kRuntimePointerSize);
      ids[op] = env->GetStaticMethodID(java_owner, names[kind][op], sigs[kind][op]);
      if (!methods[op] || !ids[op] || env->ExceptionCheck()) return false;
      art::CodeItemDataAccessor code(*methods[op]->GetDexFile(), methods[op]->GetCodeItem());
      field = art::Runtime::Current()->GetClassLinker()->ResolveField(
          art::Instruction::At(code.Insns())->VRegB_21c(), methods[op], true);
      if (!field || env->ExceptionCheck() || !field->GetDeclaringClass()->IsInitialized()) {
        std::cerr << "ART JIT unresolved static setup failed kind=" << kind << " op=" << op << "\n";
        return false;
      }
    }
    struct RestoreAccess {
      art::ArtField* field;
      uint32_t flags;
      ~RestoreAccess() { field->SetAccessFlags(flags); }
    } restore{field, field->GetAccessFlags()};
    for (auto tier : {art::CompilationKind::kBaseline, art::CompilationKind::kOptimized}) {
      // Test-only access change forces AOSP's unresolved field HIR. Restore on every exit.
      field->SetAccessFlags((restore.flags & ~(art::kAccPublic | art::kAccProtected)) | art::kAccPrivate);
      for (auto* method : methods) {
        if (jit->GetCodeCache()->ContainsPc(method->GetEntryPointFromQuickCompiledCode())) {
          bool removed;
          {
            art::ScopedThreadSuspension suspended(self, art::ThreadState::kSuspended);
            art::gc::ScopedGCCriticalSection gc(self, art::gc::kGcCauseInstrumentation,
                                               art::gc::kCollectorTypeInstrumentation);
            art::ScopedSuspendAll all("unresolved field acceptance recompile");
            removed = jit->GetCodeCache()->RemoveMethod(method, true);
          }
          if (!removed) return false;
        }
        if (!jit->CompileMethod(method, self, tier, false) ||
            !jit->GetCodeCache()->ContainsPc(method->GetEntryPointFromQuickCompiledCode())) {
          std::cerr << "ART JIT unresolved static success compile failed " << method->PrettyMethod() << "\n";
          return false;
        }
      }
      for (int op = 0; op < 2; ++op) {
        if (op == 0) {
          if (kind == 0) env->CallStaticVoidMethod(java_owner, ids[op], jlong(73));
          else if (kind == 1) env->CallStaticVoidMethod(java_owner, ids[op], jdouble(73.25));
          else env->CallStaticVoidMethod(java_owner, ids[op], value);
        } else {
          if (kind == 0) env->CallStaticLongMethod(java_owner, ids[op]);
          else if (kind == 1) env->CallStaticDoubleMethod(java_owner, ids[op]);
          else env->CallStaticObjectMethod(java_owner, ids[op]);
        }
        jthrowable thrown = env->ExceptionOccurred();
        if (thrown) env->ExceptionClear();
        bool correct = thrown && env->IsInstanceOf(thrown, access_error);
        if (thrown) env->DeleteLocalRef(thrown);
        if (!correct) {
          std::cerr << "ART JIT unresolved static access check failed kind=" << kind << " op=" << op << "\n";
          return false;
        }
      }
      field->SetAccessFlags(restore.flags);
      // Reuse the same compiled code after access is restored.
      for (int sample = 0; sample < 3; ++sample) {
        jlong wide = sample == 0 ? INT64_C(0x123456789abcdef) : sample == 1 ? -17 : 0;
        double real = sample == 0 ? 12345.125 : sample == 1 ? -0.0 : -19.75;
        jobject reference = sample == 0 ? value : sample == 1 ? nullptr : java_owner;
        if (kind == 0) env->CallStaticVoidMethod(java_owner, ids[0], wide);
        else if (kind == 1) env->CallStaticVoidMethod(java_owner, ids[0], real);
        else env->CallStaticVoidMethod(java_owner, ids[0], reference);
        if (env->ExceptionCheck()) return false;
        uint64_t before = art::Runtime::Current()->GetHeap()->GetGcCount();
        env->CallStaticVoidMethod(java_owner, gc);
        if (env->ExceptionCheck() || art::Runtime::Current()->GetHeap()->GetGcCount() <= before) return false;
        bool correct;
        if (kind == 0) correct = env->CallStaticLongMethod(java_owner, ids[1]) == wide;
        else if (kind == 1) {
          double actual = env->CallStaticDoubleMethod(java_owner, ids[1]);
          correct = std::memcmp(&actual, &real, sizeof(real)) == 0;
        } else {
          jobject actual = env->CallStaticObjectMethod(java_owner, ids[1]);
          correct = env->IsSameObject(actual, reference);
          if (actual) env->DeleteLocalRef(actual);
        }
        if (!correct || env->ExceptionCheck()) {
          std::cerr << "ART JIT unresolved static value failed kind=" << kind << " sample=" << sample << "\n";
          return false;
        }
      }
    }
  }
  env->DeleteLocalRef(value);
  env->DeleteLocalRef(access_error);
  std::cerr << "ART JIT unresolved static fields: access failures=12, same-code wide/FP/reference recovery+GC PASS cases=18\n";
  return true;
}
}  // namespace aim_jni_acceptance_phase
