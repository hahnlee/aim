#pragma once
namespace aim_jni_acceptance_phase {
inline bool CheckJitPolymorphicFloating(JNIEnv* env, art::Thread* self, art::jit::Jit* jit,
    art::Handle<art::mirror::Class> owner, jclass java_owner, jobject object,
    jobject incompatible, jclass npe, jclass wrong_type) {
  const char* factories[] = {"jitPolymorphicFloatHandle", "jitPolymorphicDoubleHandle"};
  const char* names[] = {"jitPolymorphicFloat", "jitPolymorphicDouble"};
  const char* targets[] = {"jitPolymorphicFloatTarget", "jitPolymorphicDoubleTarget"};
  const char* target_signatures[] = {"(FFFFFFFFFLjava/lang/Object;)F", "(DDDDDDDDDLjava/lang/Object;)D"};
  const char* signatures[] = {
      "(Ljava/lang/invoke/MethodHandle;FFFFFFFFFLjava/lang/Object;)F",
      "(Ljava/lang/invoke/MethodHandle;DDDDDDDDDLjava/lang/Object;)D"};
  const uint64_t patterns[][6] = {
      {0, UINT64_C(0x80000000), 1, UINT64_C(0x7f800000), UINT64_C(0x7fc12345), UINT64_C(0x7f7fffff)},
      {0, UINT64_C(0x8000000000000000), 1, UINT64_C(0x7ff0000000000000),
       UINT64_C(0x7ff8123456789abc), UINT64_C(0x7fefffffffffffff)}};
  for (int kind = 0; kind < 2; ++kind) {
    auto factory = env->GetStaticMethodID(java_owner, factories[kind], "()Ljava/lang/invoke/MethodHandle;");
    if (!factory || env->ExceptionCheck()) return false;
    jobject handle = env->CallStaticObjectMethod(java_owner, factory);
    auto id = env->GetStaticMethodID(java_owner, names[kind], signatures[kind]);
    auto* method = owner->FindClassMethod(names[kind], signatures[kind], art::kRuntimePointerSize);
    auto* target = owner->FindClassMethod(targets[kind], target_signatures[kind], art::kRuntimePointerSize);
    if (!handle || !id || !method || !target || env->ExceptionCheck()) return false;
    art::CodeItemDataAccessor code(*method->GetDexFile(), method->GetCodeItem());
    if (art::Instruction::At(code.Insns())->Opcode() != art::Instruction::INVOKE_POLYMORPHIC_RANGE) return false;
    for (int phase = 0; phase < 4; ++phase) {
      if (phase == 3 && (!jit->CompileMethod(target, self, art::CompilationKind::kOptimized, false) ||
          !jit->GetCodeCache()->ContainsPc(target->GetEntryPointFromQuickCompiledCode()))) return false;
      if (phase > 0 && phase < 3 && (!jit->CompileMethod(method, self,
          phase == 1 ? art::CompilationKind::kBaseline : art::CompilationKind::kOptimized, false) ||
          !jit->GetCodeCache()->ContainsPc(method->GetEntryPointFromQuickCompiledCode()))) return false;
      for (uint64_t bits : patterns[kind]) {
        for (jobject reference : {jobject(nullptr), object}) {
          jvalue args[11]{};
          args[0].l = handle;
          for (int index = 1; index <= 8; ++index) {
            if (kind == 0) args[index].f = static_cast<jfloat>(index);
            else args[index].d = static_cast<jdouble>(index);
          }
          if (kind == 0) {
            uint32_t low = static_cast<uint32_t>(bits);
            memcpy(&args[9].f, &low, sizeof(low));
          } else {
            memcpy(&args[9].d, &bits, sizeof(bits));
          }
          args[10].l = reference;
          uint64_t before = art::Runtime::Current()->GetHeap()->GetGcCount();
          uint64_t actual_bits = 0;
          uint64_t expected_bits = bits;
          if (kind == 0) {
            jfloat actual = env->CallStaticFloatMethodA(java_owner, id, args);
            uint32_t low;
            memcpy(&low, &actual, sizeof(low)); actual_bits = low;
            if (!reference) {
              jfloat expected = -1234.5f;
              memcpy(&low, &expected, sizeof(low)); expected_bits = low;
            }
          } else {
            jdouble actual = env->CallStaticDoubleMethodA(java_owner, id, args);
            memcpy(&actual_bits, &actual, sizeof(actual));
            if (!reference) {
              jdouble expected = -1234.5;
              memcpy(&expected_bits, &expected, sizeof(expected));
            }
          }
          if (env->ExceptionCheck() || actual_bits != expected_bits ||
              art::Runtime::Current()->GetHeap()->GetGcCount() <= before) {
            std::cerr << "ART JIT MethodHandle FP spill/GC failed kind=" << kind
                      << " phase=" << phase << " actual=" << actual_bits
                      << " expected=" << expected_bits << "\n";
            env->ExceptionDescribe();
            return false;
          }
        }
      }
      for (jobject bad : {jobject(nullptr), incompatible}) {
        jvalue args[11]{}; args[0].l = bad;
        if (kind == 0) env->CallStaticFloatMethodA(java_owner, id, args);
        else env->CallStaticDoubleMethodA(java_owner, id, args);
        jthrowable thrown = env->ExceptionOccurred();
        if (thrown) env->ExceptionClear();
        bool correct = thrown && env->IsInstanceOf(thrown, bad ? wrong_type : npe);
        if (thrown) env->DeleteLocalRef(thrown);
        if (!correct) return false;
      }
    }
    env->DeleteLocalRef(handle);
  }
  std::cerr << "ART JIT invoke-polymorphic: FP register spill/return bits/callee-GC=96 null/wrong-type=16 PASS\n";
  return true;
}
}  // namespace aim_jni_acceptance_phase
