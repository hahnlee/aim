#pragma once
namespace aim_jni_acceptance_phase {
inline bool CheckJitColdTypes(JNIEnv* env,art::Thread* self,art::jit::Jit* jit,
                              art::Handle<art::mirror::Class> owner,jclass java_owner) {
  const char* names[]={"jitIsChecksum","jitCastChecksum"};
  auto missing_id = env->GetStaticMethodID(java_owner, "jitMissingClass", "()Ljava/lang/Class;");
  auto* missing = owner->FindClassMethod("jitMissingClass", "()Ljava/lang/Class;", art::kRuntimePointerSize);
  jclass absent = env->FindClass("java/lang/NoClassDefFoundError");
  if (!missing_id || !missing || !absent || env->ExceptionCheck()) return false;
  for (int phase = 0; phase < 3; ++phase) {
    if (phase && (!jit->CompileMethod(missing, self, phase == 1 ? art::CompilationKind::kBaseline : art::CompilationKind::kOptimized, false) ||
        !jit->GetCodeCache()->ContainsPc(missing->GetEntryPointFromQuickCompiledCode()))) {
      std::cerr << "ART JIT unresolved class compile failed phase=" << phase << "\n";
      return false;
    }
    env->CallStaticObjectMethod(java_owner, missing_id);
    jthrowable thrown = env->ExceptionOccurred();
    if (thrown) env->ExceptionClear();
    bool correct = thrown && env->IsInstanceOf(thrown, absent);
    if (thrown) env->DeleteLocalRef(thrown);
    if (!correct) return false;
  }
  const char* missing_names[] = {"jitMissingInstanceOf", "jitMissingCast"};
  const char* missing_sigs[] = {"(Ljava/lang/Object;)Z", "(Ljava/lang/Object;)Ljava/lang/Object;"};
  for (int kind = 0; kind < 2; ++kind) {
    auto id = env->GetStaticMethodID(java_owner, missing_names[kind], missing_sigs[kind]);
    auto* method = owner->FindClassMethod(missing_names[kind], missing_sigs[kind], art::kRuntimePointerSize);
    if (!id || !method || env->ExceptionCheck()) return false;
    bool expected_exception[2]{};
    for (int phase = 0; phase < 3; ++phase) {
      if (phase && (!jit->CompileMethod(method, self, phase == 1 ? art::CompilationKind::kBaseline : art::CompilationKind::kOptimized, false) ||
          !jit->GetCodeCache()->ContainsPc(method->GetEntryPointFromQuickCompiledCode()))) {
        std::cerr << "ART JIT unresolved type check compile failed kind=" << kind << " phase=" << phase << "\n";
        return false;
      }
      for (int input = 0; input < 2; ++input) {
        jobject value = input == 0 ? nullptr : java_owner;
        jobject result = nullptr;
        jboolean boolean_result = JNI_FALSE;
        if (kind == 0) boolean_result = env->CallStaticBooleanMethod(java_owner, id, value);
        else result = env->CallStaticObjectMethod(java_owner, id, value);
        jthrowable thrown = env->ExceptionOccurred();
        if (thrown) env->ExceptionClear();
        bool has_exception = thrown != nullptr;
        if (phase == 0) expected_exception[input] = has_exception;
        bool correct = has_exception == expected_exception[input] &&
            (has_exception ? env->IsInstanceOf(thrown, absent) :
             (kind == 0 ? boolean_result == JNI_FALSE : result == nullptr));
        if (result) env->DeleteLocalRef(result);
        if (thrown) env->DeleteLocalRef(thrown);
        if (!correct) {
          std::cerr << "ART JIT unresolved type check mismatch kind=" << kind << " phase=" << phase << " input=" << input << "\n";
          return false;
        }
      }
    }
    std::cerr << "ART JIT unresolved type check kind=" << kind << " null_throws=" << expected_exception[0]
              << " object_throws=" << expected_exception[1] << " PASS cases=6\n";
  }
  const char* field_names[] = {"jitMissingStaticInt", "jitMissingStaticReference", "jitMissingStaticIntSet", "jitMissingStaticReferenceSet"};
  const char* field_sigs[] = {"()I", "()Ljava/lang/Object;", "(I)V", "(Ljava/lang/Object;)V"};
  for (int kind = 0; kind < 4; ++kind) {
    auto id = env->GetStaticMethodID(java_owner, field_names[kind], field_sigs[kind]);
    auto* method = owner->FindClassMethod(field_names[kind], field_sigs[kind], art::kRuntimePointerSize);
    if (!id || !method || env->ExceptionCheck()) return false;
    for (int phase = 0; phase < 3; ++phase) {
      if (phase && (!jit->CompileMethod(method, self, phase == 1 ? art::CompilationKind::kBaseline : art::CompilationKind::kOptimized, false) ||
          !jit->GetCodeCache()->ContainsPc(method->GetEntryPointFromQuickCompiledCode()))) {
        std::cerr << "ART JIT unresolved static field compile failed kind=" << kind << " phase=" << phase << "\n";
        return false;
      }
      switch (kind) {
        case 0: env->CallStaticIntMethod(java_owner, id); break;
        case 1: env->CallStaticObjectMethod(java_owner, id); break;
        case 2: env->CallStaticVoidMethod(java_owner, id, jint(0x12345678)); break;
        case 3: env->CallStaticVoidMethod(java_owner, id, java_owner); break;
      }
      jthrowable thrown = env->ExceptionOccurred();
      if (thrown) env->ExceptionClear();
      bool correct = thrown && env->IsInstanceOf(thrown, absent);
      if (thrown) env->DeleteLocalRef(thrown);
      if (!correct) return false;
    }
  }
  std::cerr << "ART JIT unresolved static fields: primitive/reference get/set resolution failure PASS cases=12\n";
  env->DeleteLocalRef(absent);
  std::cerr << "ART JIT unresolved class: runtime resolution NoClassDefFoundError PASS phases=3\n";
  const char* sigs[]={"(Ljava/lang/Object;)Z","(Ljava/lang/Object;)Ljava/lang/Object;"};
  jmethodID ids[2]={};
  for(unsigned i=0;i<2;++i) {
    ids[i]=env->GetStaticMethodID(java_owner,names[i],sigs[i]);
    auto* method=owner->FindClassMethod(names[i],sigs[i],art::kRuntimePointerSize);
    if(!ids[i] || !method || env->ExceptionCheck()) return false;
    art::CodeItemDataAccessor code(*method->GetDexFile(),method->GetCodeItem());
    const auto* inst=art::Instruction::At(code.Insns());
    if(inst->Opcode()!=(i==0 ? art::Instruction::INSTANCE_OF:art::Instruction::CHECK_CAST)) return false;
    uint32_t index=i==0 ? inst->VRegC_22c():inst->VRegB_21c();
    bool resolved=method->GetDexCache()->GetResolvedType(art::dex::TypeIndex(index))!=nullptr;
    if(!jit->CompileMethod(method,self,art::CompilationKind::kOptimized,false) ||
       !jit->GetCodeCache()->ContainsPc(method->GetEntryPointFromQuickCompiledCode())) return false;
    std::cerr<<"ART JIT cold type compile "<<names[i]<<" cache_before="<<resolved
             <<" cache_after="<<(method->GetDexCache()->GetResolvedType(art::dex::TypeIndex(index))!=nullptr)<<"\n";
  }
  jclass concrete=env->FindClass("java/util/zip/Adler32");
  if(!concrete || env->ExceptionCheck()) return false;
  jobject value=env->AllocObject(concrete);
  jclass cast_error=env->FindClass("java/lang/ClassCastException");
  if(!value || !cast_error || env->ExceptionCheck()) return false;
  if(!env->CallStaticBooleanMethod(java_owner,ids[0],value) || env->ExceptionCheck()) return false;
  if(env->CallStaticBooleanMethod(java_owner,ids[0],java_owner) || env->ExceptionCheck()) return false;
  if(env->CallStaticBooleanMethod(java_owner,ids[0],nullptr) || env->ExceptionCheck()) return false;
  jobject actual=env->CallStaticObjectMethod(java_owner,ids[1],value);
  if(env->ExceptionCheck() || !env->IsSameObject(actual,value)) return false;
  env->DeleteLocalRef(actual);
  if(env->CallStaticObjectMethod(java_owner,ids[1],nullptr)!=nullptr || env->ExceptionCheck()) return false;
  env->CallStaticObjectMethod(java_owner,ids[1],java_owner);
  if(!env->ExceptionCheck()) return false;
  jthrowable error=env->ExceptionOccurred(); env->ExceptionClear();
  bool correct=env->IsInstanceOf(error,cast_error);
  env->DeleteLocalRef(error);env->DeleteLocalRef(value);env->DeleteLocalRef(concrete);env->DeleteLocalRef(cast_error);
  if(correct) std::cerr<<"ART JIT cold type: compile-before-invoke interface and cast outcomes PASS (cache state logged separately; not proof of unresolved runtime loading)\n";
  return correct;
}
}  // namespace aim_jni_acceptance_phase
