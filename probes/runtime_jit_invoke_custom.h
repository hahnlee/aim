#pragma once

namespace aim_jni_acceptance_phase {

inline bool MethodContainsInvokeCustom(art::ArtMethod* method) {
  if (method == nullptr || method->GetCodeItem() == nullptr) return false;
  art::CodeItemDataAccessor code(*method->GetDexFile(), method->GetCodeItem());
  const uint16_t* const end = code.Insns() + code.InsnsSizeInCodeUnits();
  for (const art::Instruction* instruction = art::Instruction::At(code.Insns());
       reinterpret_cast<const uint16_t*>(instruction) < end;
       instruction = instruction->Next()) {
    if (instruction->Opcode() == art::Instruction::INVOKE_CUSTOM ||
        instruction->Opcode() == art::Instruction::INVOKE_CUSTOM_RANGE) {
      return true;
    }
  }
  return false;
}

inline bool MethodContainsInvokeCustomRange(art::ArtMethod* method) {
  if (method == nullptr || method->GetCodeItem() == nullptr) return false;
  art::CodeItemDataAccessor code(*method->GetDexFile(), method->GetCodeItem());
  const uint16_t* const end = code.Insns() + code.InsnsSizeInCodeUnits();
  for (const art::Instruction* instruction = art::Instruction::At(code.Insns());
       reinterpret_cast<const uint16_t*>(instruction) < end;
       instruction = instruction->Next()) {
    if (instruction->Opcode() == art::Instruction::INVOKE_CUSTOM_RANGE) return true;
  }
  return false;
}

inline bool CompileInvokeCustomMethod(art::jit::Jit* jit,
                                      art::Thread* self,
                                      art::ArtMethod* method,
                                      int phase) {
  if (phase == 0) return true;
  const art::CompilationKind kind = phase == 1
      ? art::CompilationKind::kBaseline
      : art::CompilationKind::kOptimized;
  if (!jit->CompileMethod(method, self, kind, false) ||
      !jit->GetCodeCache()->ContainsPc(method->GetEntryPointFromQuickCompiledCode())) {
    std::cerr << "ART JIT invoke-custom compile failed phase=" << phase
              << " method=" << method->PrettyMethod() << "\n";
    return false;
  }
  return true;
}

inline bool CheckJitInvokeCustom(JNIEnv* env,
                                 art::Thread* self,
                                 art::jit::Jit* jit,
                                 art::Handle<art::mirror::Class> app_owner) {
  art::jit::ScopedJitSuspend workers;
  art::StackHandleScope<2> hs(self);
  art::Handle<art::mirror::ClassLoader> app_loader =
      hs.NewHandle(app_owner->GetClassLoader());
  constexpr const char* descriptor = "Ldev/aim/probe/JitInvokeCustom;";
  art::ObjPtr<art::mirror::Class> mirror_owner =
      art::Runtime::Current()->GetClassLinker()->FindClass(
          self,
          descriptor,
          std::char_traits<char>::length(descriptor),
          app_loader);
  if (mirror_owner == nullptr || self->IsExceptionPending()) {
    std::cerr << "ART JIT invoke-custom app ClassLoader lookup failed\n";
    return false;
  }
  art::Handle<art::mirror::Class> owner_handle = hs.NewHandle(mirror_owner);
  art::ClassLinker* class_linker = art::Runtime::Current()->GetClassLinker();
  if (!class_linker->EnsureInitialized(self, owner_handle, true, true)) return false;
  {
    art::ScopedThreadStateChange suspended(self, art::ThreadState::kNative);
    class_linker->MakeInitializedClassesVisiblyInitialized(self, true);
  }
  jclass owner = self->GetJniEnv()->AddLocalReference<jclass>(mirror_owner);
  jclass illegal_argument = env->FindClass("java/lang/IllegalArgumentException");
  if (owner == nullptr || illegal_argument == nullptr || env->ExceptionCheck()) return false;

  struct IntMethod {
    const char* name;
    const char* signature;
  };
  const IntMethod int_methods[] = {
      {"stateless", "(I)I"},
      {"capturing", "(II)I"},
      {"throwing", "(I)I"},
  };
  art::ArtMethod* methods[3]{};
  jmethodID ids[3]{};
  for (size_t index = 0; index < std::size(int_methods); ++index) {
    ids[index] = env->GetStaticMethodID(
        owner, int_methods[index].name, int_methods[index].signature);
    methods[index] = mirror_owner->FindClassMethod(
        int_methods[index].name, int_methods[index].signature, art::kRuntimePointerSize);
    if (ids[index] == nullptr || methods[index] == nullptr ||
        !MethodContainsInvokeCustom(methods[index]) || env->ExceptionCheck()) {
      std::cerr << "ART JIT invoke-custom DEX contract missing method="
                << int_methods[index].name << "\n";
      return false;
    }
  }

  constexpr const char* reference_signature = "(Ljava/lang/Object;)Ljava/lang/Object;";
  jmethodID reference_id = env->GetStaticMethodID(owner, "reference", reference_signature);
  art::ArtMethod* reference_method = mirror_owner->FindClassMethod(
      "reference", reference_signature, art::kRuntimePointerSize);
  jobject reference = env->AllocObject(owner);
  if (reference_id == nullptr || reference_method == nullptr || reference == nullptr ||
      !MethodContainsInvokeCustom(reference_method) || env->ExceptionCheck()) return false;
  constexpr const char* range_signature = "(IIIIIIJDLjava/lang/Object;)J";
  jmethodID range_id = env->GetStaticMethodID(owner, "range", range_signature);
  art::ArtMethod* range_method = mirror_owner->FindClassMethod(
      "range", range_signature, art::kRuntimePointerSize);
  if (range_id == nullptr || range_method == nullptr ||
      !MethodContainsInvokeCustomRange(range_method) || env->ExceptionCheck()) return false;

  for (int phase = 0; phase < 3; ++phase) {
    for (art::ArtMethod* method : methods) {
      if (!CompileInvokeCustomMethod(jit, self, method, phase)) return false;
    }
    if (!CompileInvokeCustomMethod(jit, self, reference_method, phase)) return false;
    if (!CompileInvokeCustomMethod(jit, self, range_method, phase)) return false;

    for (jint value : {jint(0), jint(1), jint(-1), jint(INT32_MIN), jint(INT32_MAX)}) {
      jint stateless = env->CallStaticIntMethod(owner, ids[0], value);
      if (env->ExceptionCheck() ||
          static_cast<uint32_t>(stateless) != static_cast<uint32_t>(value) + 17u) {
        std::cerr << "ART JIT invoke-custom stateless failed phase=" << phase << "\n";
        env->ExceptionDescribe();
        return false;
      }
      for (jint base : {jint(0), jint(29), jint(-73), jint(INT32_MAX)}) {
        jint capturing = env->CallStaticIntMethod(owner, ids[1], base, value);
        if (env->ExceptionCheck() || static_cast<uint32_t>(capturing) !=
            static_cast<uint32_t>(base) + static_cast<uint32_t>(value)) {
          std::cerr << "ART JIT invoke-custom capture failed phase=" << phase << "\n";
          env->ExceptionDescribe();
          return false;
        }
      }
    }

    uint64_t gc_before = art::Runtime::Current()->GetHeap()->GetGcCount();
    {
      art::ScopedThreadStateChange suspended(self, art::ThreadState::kNative);
      art::Runtime::Current()->GetHeap()->CollectGarbage(false);
    }
    jobject result = env->CallStaticObjectMethod(owner, reference_id, reference);
    bool reference_ok = !env->ExceptionCheck() && env->IsSameObject(result, reference) &&
        art::Runtime::Current()->GetHeap()->GetGcCount() > gc_before;
    if (result != nullptr) env->DeleteLocalRef(result);
    if (!reference_ok) {
      std::cerr << "ART JIT invoke-custom reference/GC failed phase=" << phase << "\n";
      env->ExceptionDescribe();
      return false;
    }

    for (jobject range_reference : {jobject(nullptr), reference}) {
      jvalue arguments[9]{};
      for (int index = 0; index < 6; ++index) arguments[index].i = index * 19 - 31;
      arguments[6].j = INT64_C(0x123456789abcdef);
      arguments[7].d = -1234.75;
      arguments[8].l = range_reference;
      jlong actual = env->CallStaticLongMethodA(owner, range_id, arguments);
      jlong expected = arguments[6].j + static_cast<jlong>(arguments[7].d) +
          (range_reference == nullptr ? 0 : 97);
      for (int index = 0; index < 6; ++index) expected += arguments[index].i;
      if (env->ExceptionCheck() || actual != expected) {
        std::cerr << "ART JIT invoke-custom/range failed phase=" << phase << "\n";
        env->ExceptionDescribe();
        return false;
      }
    }

    for (jint value : {jint(0), jint(7), jint(INT32_MAX)}) {
      jint actual = env->CallStaticIntMethod(owner, ids[2], value);
      if (env->ExceptionCheck() ||
          static_cast<uint32_t>(actual) != static_cast<uint32_t>(value) * 3u) {
        std::cerr << "ART JIT invoke-custom throwing normal path failed phase=" << phase << "\n";
        env->ExceptionDescribe();
        return false;
      }
    }
    env->CallStaticIntMethod(owner, ids[2], jint(-1));
    jthrowable thrown = env->ExceptionOccurred();
    if (thrown != nullptr) env->ExceptionClear();
    bool exception_ok = thrown != nullptr && env->IsInstanceOf(thrown, illegal_argument);
    if (thrown != nullptr) env->DeleteLocalRef(thrown);
    if (!exception_ok || env->CallStaticIntMethod(owner, ids[2], jint(5)) != 15 ||
        env->ExceptionCheck()) {
      std::cerr << "ART JIT invoke-custom exception/recovery failed phase=" << phase << "\n";
      env->ExceptionDescribe();
      return false;
    }
  }

  env->DeleteLocalRef(reference);
  env->DeleteLocalRef(illegal_argument);
  env->DeleteLocalRef(owner);
  std::cerr << "ART JIT invoke-custom/range: stateless/capturing/reference/exception "
               "interpreter+baseline+optimized with CC GC PASS\n";
  return true;
}

}  // namespace aim_jni_acceptance_phase
