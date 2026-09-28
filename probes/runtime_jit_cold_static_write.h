#pragma once
#include <cstring>
namespace aim_jni_acceptance_phase {
inline bool CheckJitColdStaticWrite(JNIEnv* env, art::Thread* self, art::jit::Jit* jit,
                                    art::Handle<art::mirror::Class> owner, jclass java_owner) {
  const char* names[][3] = {{"jitColdLongSet", "jitColdLongGet", "jitColdLongCount"},
                           {"jitColdDoubleSet", "jitColdDoubleGet", "jitColdDoubleCount"},
                           {"jitColdReferenceSet", "jitColdReferenceGet", "jitColdReferenceCount"}};
  const char* sigs[][2] = {{"(J)V", "()J"}, {"(D)V", "()D"},
                          {"(Ljava/lang/Object;)V", "()Ljava/lang/Object;"}};
  jobject value = env->AllocObject(java_owner);
  if (!value || env->ExceptionCheck()) return false;
  for (int kind = 0; kind < 3; ++kind) {
    jmethodID ids[2]{};
    jfieldID count = env->GetStaticFieldID(java_owner, names[kind][2], "I");
    if (!count || env->ExceptionCheck()) return false;
    art::ArtField* field = nullptr;
    for (int op = 0; op < 2; ++op) {
      ids[op] = env->GetStaticMethodID(java_owner, names[kind][op], sigs[kind][op]);
      auto* method = owner->FindClassMethod(names[kind][op], sigs[kind][op], art::kRuntimePointerSize);
      if (!ids[op] || !method || env->ExceptionCheck()) return false;
      art::CodeItemDataAccessor code(*method->GetDexFile(), method->GetCodeItem());
      const auto* access = art::Instruction::At(code.Insns());
      auto expected = op == 0 ? (kind == 2 ? art::Instruction::SPUT_OBJECT : art::Instruction::SPUT_WIDE)
                              : (kind == 2 ? art::Instruction::SGET_OBJECT : art::Instruction::SGET_WIDE);
      if (access->Opcode() != expected) return false;
      field = art::Runtime::Current()->GetClassLinker()->ResolveField(access->VRegB_21c(), method, true);
      if (!field || env->ExceptionCheck() || field->GetDeclaringClass()->IsInitialized()) return false;
      if (!jit->CompileMethod(method, self, art::CompilationKind::kOptimized, false) ||
          !jit->GetCodeCache()->ContainsPc(method->GetEntryPointFromQuickCompiledCode()) ||
          field->GetDeclaringClass()->IsInitialized() || env->GetStaticIntField(java_owner, count) != 0) return false;
    }
    for (int sample = 0; sample < 3; ++sample) {
      if (kind == 0) {
        jlong expected = sample == 0 ? static_cast<jlong>(0x12345678abcdef01LL) : sample == 1 ? -17 : 0;
        env->CallStaticVoidMethod(java_owner, ids[0], expected);
        if (env->ExceptionCheck() || env->CallStaticLongMethod(java_owner, ids[1]) != expected) return false;
      } else if (kind == 1) {
        double expected = sample == 0 ? 12345.125 : sample == 1 ? -0.0 : -19.75;
        env->CallStaticVoidMethod(java_owner, ids[0], expected);
        if (env->ExceptionCheck()) return false;
        double actual = env->CallStaticDoubleMethod(java_owner, ids[1]);
        if (std::memcmp(&actual, &expected, sizeof(actual)) != 0) return false;
      } else {
        jobject expected = sample == 1 ? nullptr : value;
        env->CallStaticVoidMethod(java_owner, ids[0], expected);
        if (env->ExceptionCheck()) return false;
        jobject actual = env->CallStaticObjectMethod(java_owner, ids[1]);
        bool same = !env->ExceptionCheck() && env->IsSameObject(actual, expected);
        if (actual) env->DeleteLocalRef(actual);
        if (!same) return false;
      }
      if (env->ExceptionCheck() || env->GetStaticIntField(java_owner, count) != 1) return false;
    }
    if (!field->GetDeclaringClass()->IsInitialized()) return false;
  }
  env->DeleteLocalRef(value);
  std::cerr << "ART JIT cold static writes: long/double/reference cold setter, live args, once-only PASS\n";
  return true;
}
}  // namespace aim_jni_acceptance_phase
