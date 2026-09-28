#pragma once
namespace aim_jni_acceptance_phase {
inline bool CheckJitPolymorphicAccessors(JNIEnv* env, art::Thread* self, art::jit::Jit* jit,
    art::Handle<art::mirror::Class> owner, jclass java_owner, jobject receiver, jclass npe) {
  auto factory = env->GetStaticMethodID(java_owner, "jitPolymorphicAccessorHandle", "(I)Ljava/lang/invoke/MethodHandle;");
  if (!factory || env->ExceptionCheck()) return false;
  const char* names[] = {"jitPolymorphicGet", "jitPolymorphicPut", "jitPolymorphicStaticGet", "jitPolymorphicStaticPut"};
  const char* sigs[] = {
      "(Ljava/lang/invoke/MethodHandle;Ldev/aim/probe/Hello;)Ljava/lang/Object;",
      "(Ljava/lang/invoke/MethodHandle;Ldev/aim/probe/Hello;Ljava/lang/Object;)V",
      "(Ljava/lang/invoke/MethodHandle;)Ljava/lang/Object;",
      "(Ljava/lang/invoke/MethodHandle;Ljava/lang/Object;)V"};
  jobject handles[4]{};
  jmethodID ids[4]{};
  art::ArtMethod* methods[4]{};
  for (int kind = 0; kind < 4; ++kind) {
    handles[kind] = env->CallStaticObjectMethod(java_owner, factory, jint(kind));
    ids[kind] = env->GetStaticMethodID(java_owner, names[kind], sigs[kind]);
    methods[kind] = owner->FindClassMethod(names[kind], sigs[kind], art::kRuntimePointerSize);
    if (!handles[kind] || !ids[kind] || !methods[kind] || env->ExceptionCheck()) return false;
    if (art::ObjPtr<art::mirror::MethodHandle>::DownCast(self->DecodeJObject(handles[kind]))->GetHandleKind() !=
        static_cast<art::mirror::MethodHandle::Kind>(art::mirror::MethodHandle::kInstanceGet + kind)) return false;
    art::CodeItemDataAccessor code(*methods[kind]->GetDexFile(), methods[kind]->GetCodeItem());
    if (art::Instruction::At(code.Insns())->Opcode() != art::Instruction::INVOKE_POLYMORPHIC) return false;
  }
  jfieldID fields[] = {
      env->GetFieldID(java_owner, "jitHandleReference", "Ljava/lang/Object;"),
      env->GetStaticFieldID(java_owner, "jitHandleStaticReference", "Ljava/lang/Object;")};
  if (!fields[0] || !fields[1] || env->ExceptionCheck()) return false;
  for (int phase = 0; phase < 3; ++phase) {
    if (phase) for (auto* method : methods) {
      if (!jit->CompileMethod(method, self,
          phase == 1 ? art::CompilationKind::kBaseline : art::CompilationKind::kOptimized, false) ||
          !jit->GetCodeCache()->ContainsPc(method->GetEntryPointFromQuickCompiledCode())) {
        std::cerr << "ART JIT MethodHandle accessor compile failed phase=" << phase << "\n";
        return false;
      }
    }
    for (int stat = 0; stat < 2; ++stat) {
      int get = stat * 2, put = get + 1;
      for (jobject value : {jobject(nullptr), receiver, jobject(java_owner)}) {
        if (stat) env->CallStaticVoidMethod(java_owner, ids[put], handles[put], value);
        else env->CallStaticVoidMethod(java_owner, ids[put], handles[put], receiver, value);
        if (env->ExceptionCheck()) { env->ExceptionDescribe(); return false; }
        uint64_t before = art::Runtime::Current()->GetHeap()->GetGcCount();
        { art::ScopedThreadStateChange native(self, art::ThreadState::kNative);
          art::Runtime::Current()->GetHeap()->CollectGarbage(false); }
        jobject independent = stat ? env->GetStaticObjectField(java_owner, fields[stat])
                                  : env->GetObjectField(receiver, fields[stat]);
        jobject result = stat ? env->CallStaticObjectMethod(java_owner, ids[get], handles[get])
                              : env->CallStaticObjectMethod(java_owner, ids[get], handles[get], receiver);
        bool correct = !env->ExceptionCheck() && env->IsSameObject(result, value) &&
            env->IsSameObject(independent, value) && art::Runtime::Current()->GetHeap()->GetGcCount() > before;
        if (result) env->DeleteLocalRef(result);
        if (independent) env->DeleteLocalRef(independent);
        if (!correct) {
          std::cerr << "ART JIT MethodHandle accessor store/read/GC failed phase=" << phase << " static=" << stat << "\n";
          env->ExceptionDescribe(); return false;
        }
        jobject alternate = value ? nullptr : receiver;
        if (stat) env->SetStaticObjectField(java_owner, fields[stat], alternate);
        else env->SetObjectField(receiver, fields[stat], alternate);
        result = stat ? env->CallStaticObjectMethod(java_owner, ids[get], handles[get])
                      : env->CallStaticObjectMethod(java_owner, ids[get], handles[get], receiver);
        correct = !env->ExceptionCheck() && env->IsSameObject(result, alternate);
        if (result) env->DeleteLocalRef(result);
        if (!correct) return false;
      }
    }
    // Both instance accessor targets must reject a null receiver.
    for (int kind = 0; kind < 2; ++kind) {
      if (kind == 0) env->CallStaticObjectMethod(java_owner, ids[kind], handles[kind], nullptr);
      else env->CallStaticVoidMethod(java_owner, ids[kind], handles[kind], nullptr, receiver);
      jthrowable thrown = env->ExceptionOccurred();
      if (thrown) env->ExceptionClear();
      bool correct = thrown && env->IsInstanceOf(thrown, npe);
      if (thrown) env->DeleteLocalRef(thrown);
      if (!correct) return false;
    }
  }
  env->SetObjectField(receiver, fields[0], nullptr);
  env->SetStaticObjectField(java_owner, fields[1], nullptr);
  for (jobject handle : handles) env->DeleteLocalRef(handle);
  std::cerr << "ART JIT invoke-polymorphic: field-accessor store/GC/read=18 independent-store/read=18 null-receiver=6 PASS\n";
  return !env->ExceptionCheck();
}
}  // namespace aim_jni_acceptance_phase
