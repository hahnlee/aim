#pragma once
namespace aim_jni_acceptance_phase {
inline bool CheckJitVirtualComposed(JNIEnv* env, art::Thread* self, art::jit::Jit* jit,
                                    art::Handle<art::mirror::Class> owner, jclass java_owner) {
  art::jit::ScopedJitSuspend workers;
  jclass classes[2]{};
  jobject objects[3]{};
  const char* factories[] = {"jitVirtualBaseClass", "jitVirtualChildClass"};
  for (int i = 0; i < 2; ++i) {
    auto id = env->GetStaticMethodID(java_owner, factories[i], "()Ljava/lang/Class;");
    if (!id || env->ExceptionCheck()) return false;
    classes[i] = static_cast<jclass>(env->CallStaticObjectMethod(java_owner, id));
    if (!classes[i] || env->ExceptionCheck()) return false;
    objects[i + 1] = env->AllocObject(classes[i]);
    if (!objects[i + 1] || env->ExceptionCheck()) return false;
  }
  auto number = env->GetFieldID(classes[0], "number", "I");
  auto object = env->GetFieldID(classes[0], "object", "Ljava/lang/Object;");
  auto alternate = env->GetFieldID(classes[1], "alternative", "I");
  auto alternate_object = env->GetFieldID(classes[1], "alternativeObject", "Ljava/lang/Object;");
  if (!number || !object || !alternate || !alternate_object || env->ExceptionCheck()) return false;
  env->SetIntField(objects[1], number, 11);
  env->SetIntField(objects[2], number, 13);
  env->SetIntField(objects[2], alternate, 29);
  env->SetObjectField(objects[1], object, java_owner);
  env->SetObjectField(objects[2], alternate_object, objects[2]);
  jclass npe = env->FindClass("java/lang/NullPointerException");
  if (!npe || env->ExceptionCheck()) return false;
  const char* names[] = {"jitVirtualComposed", "jitVirtualReferences", "jitVirtualSecond"};
  const char* sigs[] = {
      "(Ldev/aim/probe/JitVirtualBase;Ldev/aim/probe/JitVirtualBase;)I",
      "(Ldev/aim/probe/JitVirtualBase;Ldev/aim/probe/JitVirtualBase;)Ljava/lang/Object;",
      "(Ldev/aim/probe/JitVirtualBase;Ldev/aim/probe/JitVirtualBase;)I"};
  art::ArtMethod* methods[3]{};
  jmethodID ids[3]{};
  for (int i = 0; i < 3; ++i) {
    methods[i] = owner->FindClassMethod(names[i], sigs[i], art::kRuntimePointerSize);
    ids[i] = env->GetStaticMethodID(java_owner, names[i], sigs[i]);
    if (!methods[i] || !ids[i] || env->ExceptionCheck()) return false;
  }
  unsigned cases = 0;
  for (int phase = 0; phase < 3; ++phase) {
    if (phase) for (auto* method : methods) {
      if (!jit->CompileMethod(method, self, phase == 1 ? art::CompilationKind::kBaseline : art::CompilationKind::kOptimized, false) ||
          !jit->GetCodeCache()->ContainsPc(method->GetEntryPointFromQuickCompiledCode())) return false;
    }
    for (int a = 0; a < 3; ++a) for (int b = 0; b < 3; ++b) for (int call = 0; call < 3; ++call) {
      jobject ref = nullptr;
      jint result = 0;
      if (call == 1) ref = env->CallStaticObjectMethod(java_owner, ids[call], objects[a], objects[b]);
      else result = env->CallStaticIntMethod(java_owner, ids[call], objects[a], objects[b]);
      bool expect_null = b == 0 || (call != 2 && a == 0);
      jthrowable exception = env->ExceptionOccurred();
      if (exception) env->ExceptionClear();
      bool correct = expect_null ? exception && env->IsInstanceOf(exception, npe) : !exception;
      if (correct && !expect_null) {
        if (call == 1) correct = env->IsSameObject(ref, a == 1 ? jobject(java_owner) : objects[2]);
        else correct = result == (call == 2 ? (b == 1 ? 11 : 29) : (a == 1 ? 11 : 29) * 31 + (b == 1 ? 11 : 29));
      }
      if (ref) env->DeleteLocalRef(ref);
      if (exception) env->DeleteLocalRef(exception);
      if (!correct) {
        std::cerr << "ART JIT composed virtual failed phase=" << phase << " call=" << call << " a=" << a << " b=" << b << "\n";
        return false;
      }
      ++cases;
    }
  }
  for (auto value : objects) if (value) env->DeleteLocalRef(value);
  for (auto klass : classes) env->DeleteLocalRef(klass);
  env->DeleteLocalRef(npe);
  std::cerr << "ART JIT composed virtual: override/reference/null/second receiver PASS cases=" << cases << "\n";
  return !env->ExceptionCheck();
}
}  // namespace aim_jni_acceptance_phase
