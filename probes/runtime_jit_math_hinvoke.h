#pragma once

#include <array>
#include <limits>

namespace aim_jni_acceptance_phase {

inline bool CheckJitMathHInvoke(JNIEnv* env,
                                art::Thread* self,
                                art::jit::Jit* jit,
                                art::Handle<art::mirror::Class> owner,
                                jclass java_owner) {
  constexpr const char* signature = "(DDFFFJJ)J";
  auto* method = owner->FindClassMethod("jitMathHInvoke", signature, art::kRuntimePointerSize);
  jmethodID id = env->GetStaticMethodID(java_owner, "jitMathHInvoke", signature);
  if (method == nullptr || id == nullptr || env->ExceptionCheck()) return false;

  struct Sample {
    jdouble x;
    jdouble y;
    jfloat a;
    jfloat b;
    jfloat c;
    jlong left;
    jlong right;
  };
  const std::array<Sample, 8> samples{{
      {0.25, 1.75, 0.25f, -2.0f, 0.5f, 3, 7},
      {-0.75, 0.5, -0.75f, 4.0f, -1.0f, INT64_MIN, -1},
      {0.0, -0.0, 0.0f, -0.0f, 1.0f, INT64_MAX, INT64_MAX},
      {-0.0, 0.0, -0.0f, 0.0f, -1.0f, INT64_MIN, INT64_MAX},
      {2.0, -3.0, 2.0f, 3.0f, -4.0f, INT64_C(0x123456789abcdef),
       -INT64_C(0x112233445566778)},
      {std::numeric_limits<jdouble>::infinity(), 1.0,
       std::numeric_limits<jfloat>::infinity(), 1.0f, 2.0f, -73, 29},
      {-std::numeric_limits<jdouble>::infinity(), -1.0,
       -std::numeric_limits<jfloat>::infinity(), -1.0f, -2.0f, 0, -1},
      {std::numeric_limits<jdouble>::quiet_NaN(),
       std::numeric_limits<jdouble>::quiet_NaN(),
       std::numeric_limits<jfloat>::quiet_NaN(),
       std::numeric_limits<jfloat>::quiet_NaN(), 0.0f, -1, 0},
  }};

  auto invoke = [&](const Sample& sample) {
    jvalue args[7]{};
    args[0].d = sample.x;
    args[1].d = sample.y;
    args[2].f = sample.a;
    args[3].f = sample.b;
    args[4].f = sample.c;
    args[5].j = sample.left;
    args[6].j = sample.right;
    return env->CallStaticLongMethodA(java_owner, id, args);
  };

  std::array<jlong, 8> expected{};
  for (size_t index = 0; index < samples.size(); ++index) expected[index] = invoke(samples[index]);
  if (env->ExceptionCheck()) return false;

  for (int phase = 1; phase < 3; ++phase) {
    art::CompilationKind kind = phase == 1
        ? art::CompilationKind::kBaseline
        : art::CompilationKind::kOptimized;
    if (!jit->CompileMethod(method, self, kind, false) ||
        !jit->GetCodeCache()->ContainsPc(method->GetEntryPointFromQuickCompiledCode())) {
      std::cerr << "ART JIT Math HInvoke compile failed phase=" << phase << "\n";
      return false;
    }
    for (size_t index = 0; index < samples.size(); ++index) {
      jlong actual = invoke(samples[index]);
      if (actual != expected[index] || env->ExceptionCheck()) {
        std::cerr << "ART JIT Math HInvoke mismatch phase=" << phase
                  << " sample=" << index << " expected=" << expected[index]
                  << " actual=" << actual << "\n";
        return false;
      }
    }
  }

  art::StackHandleScope<2> hs(self);
  art::Handle<art::mirror::ClassLoader> app_loader = hs.NewHandle(owner->GetClassLoader());
  constexpr const char* descriptor = "Ldev/aim/probe/JitMathDirect;";
  art::ObjPtr<art::mirror::Class> direct_mirror =
      art::Runtime::Current()->GetClassLinker()->FindClass(
          self, descriptor, std::char_traits<char>::length(descriptor), app_loader);
  if (direct_mirror == nullptr || self->IsExceptionPending()) return false;
  art::Handle<art::mirror::Class> direct_handle = hs.NewHandle(direct_mirror);
  if (!art::Runtime::Current()->GetClassLinker()->EnsureInitialized(
          self, direct_handle, true, true)) return false;
  art::Runtime::Current()->GetClassLinker()->MakeInitializedClassesVisiblyInitialized(
      self, /*wait=*/ true);
  jclass direct_class = self->GetJniEnv()->AddLocalReference<jclass>(direct_mirror);
  auto* direct_method = direct_mirror->FindClassMethod(
      "multiplyHigh", "(JJ)J", art::kRuntimePointerSize);
  jmethodID direct_id = env->GetStaticMethodID(direct_class, "multiplyHigh", "(JJ)J");
  if (direct_method == nullptr || direct_id == nullptr || env->ExceptionCheck()) return false;
  constexpr std::array<std::array<jlong, 2>, 6> products{{
      {{0, 0}}, {{3, 7}}, {{-1, -1}}, {{INT64_MIN, -1}},
      {{INT64_MAX, INT64_MAX}}, {{INT64_C(0x123456789abcdef), -73}},
  }};
  std::array<jlong, products.size()> expected_products{};
  for (size_t index = 0; index < products.size(); ++index) {
    expected_products[index] =
        env->CallStaticLongMethod(direct_class, direct_id, products[index][0], products[index][1]);
  }
  if (env->ExceptionCheck()) return false;
  for (int phase = 1; phase < 3; ++phase) {
    art::CompilationKind kind = phase == 1
        ? art::CompilationKind::kBaseline
        : art::CompilationKind::kOptimized;
    if (!jit->CompileMethod(direct_method, self, kind, false) ||
        !jit->GetCodeCache()->ContainsPc(direct_method->GetEntryPointFromQuickCompiledCode())) {
      return false;
    }
    for (size_t index = 0; index < products.size(); ++index) {
      jlong actual =
          env->CallStaticLongMethod(direct_class, direct_id, products[index][0], products[index][1]);
      if (actual != expected_products[index] || env->ExceptionCheck()) return false;
    }
  }
  env->DeleteLocalRef(direct_class);
  std::cerr << "ART JIT Math HInvoke: remaining 29 FMA/transcendental/round/"
               "copySign plus direct multiplyHigh interpreter+baseline+optimized raw bits PASS\n";
  return true;
}

}  // namespace aim_jni_acceptance_phase
