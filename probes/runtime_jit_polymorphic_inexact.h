#pragma once
namespace aim_jni_acceptance_phase {
inline bool CheckJitPolymorphicInexact(JNIEnv* env, art::Thread* self, art::jit::Jit* jit,
    art::Handle<art::mirror::Class> owner, jclass java_owner, jobject primitive_handle,
    jobject reference_handle, jobject adapted_handle, jclass npe, jclass cast_error) {
  const char* signature = "(Ljava/lang/invoke/MethodHandle;I)J";
  auto id = env->GetStaticMethodID(java_owner, "jitPolymorphicInexact", signature);
  auto* method = owner->FindClassMethod("jitPolymorphicInexact", signature, art::kRuntimePointerSize);
  if (!id || !method || env->ExceptionCheck()) return false;
  art::CodeItemDataAccessor code(*method->GetDexFile(), method->GetCodeItem());
  if (art::Instruction::At(code.Insns())->Opcode() != art::Instruction::INVOKE_POLYMORPHIC) return false;
  for (int phase = 0; phase < 3; ++phase) {
    if (phase && (!jit->CompileMethod(method, self,
        phase == 1 ? art::CompilationKind::kBaseline : art::CompilationKind::kOptimized, false) ||
        !jit->GetCodeCache()->ContainsPc(method->GetEntryPointFromQuickCompiledCode()))) {
      std::cerr << "ART JIT inexact MethodHandle compile failed phase=" << phase << "\n";
      return false;
    }
    for (jint input : {jint(0), jint(1), jint(-1), jint(INT32_MIN), jint(INT32_MAX), jint(0x12345678)}) {
      for (int boxing = 0; boxing < 2; ++boxing) {
        uint64_t before = art::Runtime::Current()->GetHeap()->GetGcCount();
        jlong actual = env->CallStaticLongMethod(java_owner, id,
            boxing ? reference_handle : primitive_handle, input);
        uint32_t bits = static_cast<uint32_t>(input) * 31u + 7u;
        jint wrapped;
        memcpy(&wrapped, &bits, sizeof(wrapped));
        jlong expected = boxing ? static_cast<jlong>(input) : static_cast<jlong>(wrapped);
        if (env->ExceptionCheck() || actual != expected ||
            (boxing && art::Runtime::Current()->GetHeap()->GetGcCount() <= before)) {
          std::cerr << "ART JIT inexact MethodHandle conversion failed phase=" << phase
                    << " boxing=" << boxing << " input=" << input
                    << " actual=" << actual << " expected=" << expected << "\n";
          env->ExceptionDescribe();
          return false;
        }
      }
    }
    for (jobject bad : {jobject(nullptr), adapted_handle}) {
      env->CallStaticLongMethod(java_owner, id, bad, jint(5));
      jthrowable thrown = env->ExceptionOccurred();
      if (thrown) env->ExceptionClear();
      bool correct = thrown && env->IsInstanceOf(thrown, bad ? cast_error : npe);
      if (thrown) env->DeleteLocalRef(thrown);
      if (!correct) {
        std::cerr << "ART JIT inexact MethodHandle exception failed phase=" << phase << "\n";
        return false;
      }
    }
  }
  std::cerr << "ART JIT invoke-polymorphic: inexact widening=18 boxing/unboxing/GC=18 exceptions=6 PASS\n";
  return true;
}
}  // namespace aim_jni_acceptance_phase
