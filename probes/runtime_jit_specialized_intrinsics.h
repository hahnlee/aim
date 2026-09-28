#pragma once

#include <array>
#include <cstring>
#include <limits>
#include <vector>

namespace aim_jni_acceptance_phase {

inline bool CheckJitSpecializedIntrinsics(JNIEnv* env,
                                          art::Thread* self,
                                          art::jit::Jit* jit,
                                          art::Handle<art::mirror::Class> owner,
                                          jclass java_owner) {
  constexpr const char* signature = "(IJFDLjava/lang/String;)J";
  auto* method = owner->FindClassMethod(
      "jitSpecializedIntrinsics", signature, art::kRuntimePointerSize);
  jmethodID id = env->GetStaticMethodID(java_owner, "jitSpecializedIntrinsics", signature);
  auto* char_at_method = owner->FindClassMethod(
      "jitSpecializedStringCharAt", "(Ljava/lang/String;I)C", art::kRuntimePointerSize);
  jmethodID char_at_id = env->GetStaticMethodID(
      java_owner, "jitSpecializedStringCharAt", "(Ljava/lang/String;I)C");
  if (method == nullptr || id == nullptr || char_at_method == nullptr || char_at_id == nullptr ||
      env->ExceptionCheck()) {
    return false;
  }

  struct Sample {
    jint integer;
    jlong wide;
    jfloat single;
    jdouble real;
    const char* text;
  };
  const Sample samples[] = {
      {0, 0, 0.0f, -0.0, "Darwin"},
      {1, -1, -0.0f, 1.0, "Android"},
      {-1, INT64_MIN, 1.25f, -2.5, "ART"},
      {INT32_MIN, INT64_MAX, -3.75f, 7.125, "intrinsic"},
      {INT32_MAX, INT64_C(0x123456789abcdef),
       std::numeric_limits<jfloat>::quiet_NaN(),
       std::numeric_limits<jdouble>::quiet_NaN(), "ARM64"},
      {29, -73, std::numeric_limits<jfloat>::infinity(),
       -std::numeric_limits<jdouble>::infinity(), "Metal"},
  };
  std::vector<jlong> expected;
  auto invoke = [&](const Sample& sample) {
    jstring text = env->NewStringUTF(sample.text);
    jvalue args[5]{};
    args[0].i = sample.integer;
    args[1].j = sample.wide;
    args[2].f = sample.single;
    args[3].d = sample.real;
    args[4].l = text;
    jlong result = env->CallStaticLongMethodA(java_owner, id, args);
    env->DeleteLocalRef(text);
    return result;
  };
  for (const Sample& sample : samples) expected.push_back(invoke(sample));
  if (env->ExceptionCheck()) return false;

  for (int phase = 1; phase < 3; ++phase) {
    art::CompilationKind kind = phase == 1
        ? art::CompilationKind::kBaseline
        : art::CompilationKind::kOptimized;
    bool main_compiled = jit->CompileMethod(method, self, kind, false);
    bool char_at_compiled = jit->CompileMethod(char_at_method, self, kind, false);
    if (!main_compiled || !char_at_compiled ||
        !jit->GetCodeCache()->ContainsPc(method->GetEntryPointFromQuickCompiledCode()) ||
        !jit->GetCodeCache()->ContainsPc(char_at_method->GetEntryPointFromQuickCompiledCode())) {
      std::cerr << "ART JIT specialized intrinsic compile failed phase=" << phase
                << " main=" << main_compiled << " char_at=" << char_at_compiled << "\n";
      return false;
    }
    for (size_t index = 0; index < std::size(samples); ++index) {
      if (invoke(samples[index]) != expected[index] || env->ExceptionCheck()) {
        std::cerr << "ART JIT specialized intrinsic mismatch phase=" << phase
                  << " sample=" << index << "\n";
        return false;
      }
    }
  }

  jstring text = env->NewStringUTF("abc");
  if (env->CallStaticCharMethod(java_owner, char_at_id, text, 1) != 'b' ||
      env->ExceptionCheck()) {
    return false;
  }
  env->CallStaticCharMethod(java_owner, char_at_id, text, 3);
  bool bounds = ClearExpectedArrayException(env, "java/lang/StringIndexOutOfBoundsException");
  env->CallStaticCharMethod(java_owner, char_at_id, nullptr, 0);
  bool null = ClearExpectedArrayException(env, "java/lang/NullPointerException");
  env->DeleteLocalRef(text);
  if (!bounds || !null || env->ExceptionCheck()) return false;

  std::cerr << "ART JIT specialized intrinsics: numeric/string/VarHandle-fence "
               "interpreter+baseline+optimized and exact exceptions PASS\n";
  return true;
}

}  // namespace aim_jni_acceptance_phase
