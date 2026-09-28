#pragma once
namespace aim_jni_acceptance_phase {
inline bool CheckJitArrayLiterals(JNIEnv* env, art::Thread* self, art::jit::Jit* jit,
                                  art::Handle<art::mirror::Class> owner, jclass java_owner) {
  art::jit::ScopedJitSuspend workers;
  const char* names[] = {"jitLiteralBytes", "jitLiteralShorts", "jitLiteralInts", "jitLiteralLongs", "jitLiteralDoubles", "jitLiteralReferences", "jitLiteralReferenceRange", "jitLiteralFloats"};
  const char* sigs[] = {"()[B", "()[S", "()[I", "()[J", "()[D",
      "(Ljava/lang/Object;Ljava/lang/Object;)[Ljava/lang/Object;",
      "(Ljava/lang/Object;Ljava/lang/Object;Ljava/lang/Object;Ljava/lang/Object;Ljava/lang/Object;Ljava/lang/Object;Ljava/lang/Object;)[Ljava/lang/Object;", "()[F"};
  auto gc = env->GetStaticMethodID(java_owner, "jitVoidGc", "()V");
  jobject value = env->AllocObject(java_owner);
  if (!gc || !value || env->ExceptionCheck()) return false;
  for (int kind = 0; kind < 8; ++kind) {
    auto* method = owner->FindClassMethod(names[kind], sigs[kind], art::kRuntimePointerSize);
    auto id = env->GetStaticMethodID(java_owner, names[kind], sigs[kind]);
    if (!method || !id || env->ExceptionCheck()) return false;
    art::CodeItemDataAccessor code(*method->GetDexFile(), method->GetCodeItem());
    bool found = false;
    auto opcode = kind == 2 ? art::Instruction::FILLED_NEW_ARRAY : kind < 5 || kind == 7 ? art::Instruction::FILL_ARRAY_DATA : kind == 5
        ? art::Instruction::FILLED_NEW_ARRAY : art::Instruction::FILLED_NEW_ARRAY_RANGE;
    for (auto pair : code) found |= pair.Inst().Opcode() == opcode;
    if (!found) { std::cerr << "ART JIT array literal missing expected opcode kind=" << kind << "\n"; return false; }
    for (int phase = 0; phase < 3; ++phase) {
      if (phase && (!jit->CompileMethod(method, self, phase == 1 ? art::CompilationKind::kBaseline : art::CompilationKind::kOptimized, false) ||
          !jit->GetCodeCache()->ContainsPc(method->GetEntryPointFromQuickCompiledCode()))) {
        std::cerr << "ART JIT array literal compile failed kind=" << kind << " phase=" << phase << "\n";
        return false;
      }
      jvalue args[7]{};
      jobject references[] = {value, java_owner, nullptr, value, java_owner, nullptr, value};
      for (int i = 0; i < 7; ++i) args[i].l = references[i];
      jarray result = static_cast<jarray>(env->CallStaticObjectMethodA(java_owner, id, args));
      if (!result || env->ExceptionCheck()) return false;
      uint64_t before = art::Runtime::Current()->GetHeap()->GetGcCount();
      env->CallStaticVoidMethod(java_owner, gc);
      if (env->ExceptionCheck() || art::Runtime::Current()->GetHeap()->GetGcCount() <= before) return false;
      bool correct = env->GetArrayLength(result) == (kind < 5 || kind == 7 ? 5 : kind == 5 ? 3 : 7);
      if (!correct || env->ExceptionCheck()) return false;
      if (kind == 7) {
        jfloat actual[5], expected[] = {-1234.5f, -0.0f, 0.0f, 1.0f, std::numeric_limits<float>::max()};
        env->GetFloatArrayRegion(static_cast<jfloatArray>(result), 0, 5, actual);
        correct = std::memcmp(actual, expected, sizeof(actual)) == 0;
      } else if (kind == 0) {
        jbyte actual[5], expected[] = {-128, -1, 0, 1, 127};
        env->GetByteArrayRegion(static_cast<jbyteArray>(result), 0, 5, actual);
        correct = std::memcmp(actual, expected, sizeof(actual)) == 0;
      } else if (kind == 1) {
        jshort actual[5], expected[] = {-32768, -1, 0, 1, 32767};
        env->GetShortArrayRegion(static_cast<jshortArray>(result), 0, 5, actual);
        correct = std::memcmp(actual, expected, sizeof(actual)) == 0;
      } else if (kind == 2) {
        jint actual[5], expected[] = {INT32_MIN, -1, 0, 1, INT32_MAX};
        env->GetIntArrayRegion(static_cast<jintArray>(result), 0, 5, actual);
        correct = std::memcmp(actual, expected, sizeof(actual)) == 0;
      } else if (kind == 3) {
        jlong actual[5], expected[] = {INT64_MIN, -1, 0, 1, INT64_MAX};
        env->GetLongArrayRegion(static_cast<jlongArray>(result), 0, 5, actual);
        correct = std::memcmp(actual, expected, sizeof(actual)) == 0;
      } else if (kind == 4) {
        jdouble actual[5], expected[] = {-1234.5, -0.0, 0.0, 1.0, std::numeric_limits<double>::max()};
        env->GetDoubleArrayRegion(static_cast<jdoubleArray>(result), 0, 5, actual);
        correct = std::memcmp(actual, expected, sizeof(actual)) == 0;
      } else {
        for (int i = 0; i < (kind == 5 ? 3 : 7); ++i) {
          jobject actual = env->GetObjectArrayElement(static_cast<jobjectArray>(result), i);
          jobject expected = kind == 5 ? (i == 0 ? value : i == 1 ? nullptr : java_owner) : references[i];
          correct &= env->IsSameObject(actual, expected);
          if (actual) env->DeleteLocalRef(actual);
        }
      }
      env->DeleteLocalRef(result);
      if (!correct || env->ExceptionCheck()) { std::cerr << "ART JIT array literal contents failed kind=" << kind << " phase=" << phase << "\n"; return false; }
    }
  }
  env->DeleteLocalRef(value);
  std::cerr << "ART JIT array literals: payload widths1/2/4/8, FP bits, filled int/reference/range + GC PASS cases=24\n";
  return true;
}
}  // namespace aim_jni_acceptance_phase
