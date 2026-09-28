#pragma once
#include <limits>
#include <vector>
namespace aim_jni_acceptance_phase {
template <typename T> inline int ComparisonMask(T a, T b) {
  return (a == b ? 1 : 0) | (a != b ? 2 : 0) | (a < b ? 4 : 0) |
         (a <= b ? 8 : 0) | (a > b ? 16 : 0) | (a >= b ? 32 : 0);
}
inline bool CheckJitCompare(JNIEnv* env, art::Thread* self, art::jit::Jit* jit,
                            art::Handle<art::mirror::Class> owner, jclass java_owner) {
  const char* names[] = {"jitCompareI", "jitCompareJ", "jitCompareF", "jitCompareD"};
  const char* signatures[] = {"(II)I", "(JJ)I", "(FF)I", "(DD)I"};
  unsigned cases = 0;
  for (unsigned kind = 0; kind < 4; ++kind) {
    jmethodID id = env->GetStaticMethodID(java_owner, names[kind], signatures[kind]);
    auto* method = owner->FindClassMethod(names[kind], signatures[kind], art::kRuntimePointerSize);
    if (!id || !method || env->ExceptionCheck() ||
        jit->GetCodeCache()->ContainsPc(method->GetEntryPointFromQuickCompiledCode())) return false;
    std::vector<jvalue> values;
    if (kind < 2) {
      const jlong integers[] = {0, 1, -1, 17, -29, std::numeric_limits<jint>::min(),
        std::numeric_limits<jint>::max(), std::numeric_limits<jlong>::min(), std::numeric_limits<jlong>::max()};
      for (jlong x : integers) {
        jvalue v = {}; if (kind == 0) v.i = static_cast<jint>(x); else v.j = x;
        values.push_back(v);
      }
    } else {
      const double numbers[] = {0.0, -0.0, 1.0, -1.0, 17.25, -29.5,
        std::numeric_limits<double>::infinity(), -std::numeric_limits<double>::infinity(),
        std::numeric_limits<double>::quiet_NaN(), std::numeric_limits<double>::denorm_min(),
        std::numeric_limits<double>::max(), std::numeric_limits<float>::denorm_min()};
      for (double x : numbers) {
        jvalue v = {}; if (kind == 2) v.f = static_cast<jfloat>(x); else v.d = x;
        values.push_back(v);
      }
    }
    auto invoke = [&](jvalue a, jvalue b) {
      jvalue args[] = {a,b}; return env->CallStaticIntMethodA(java_owner, id, args);
    };
    std::vector<jint> expected;
    for (jvalue a : values) for (jvalue b : values) expected.push_back(invoke(a,b));
    if (env->ExceptionCheck() || jit->GetCodeCache()->ContainsPc(method->GetEntryPointFromQuickCompiledCode()) ||
        !jit->CompileMethod(method, self, art::CompilationKind::kOptimized, false) ||
        !jit->GetCodeCache()->ContainsPc(method->GetEntryPointFromQuickCompiledCode())) return false;
    size_t index = 0;
    for (jvalue a : values) for (jvalue b : values) {
      int oracle = kind == 0 ? ComparisonMask(a.i,b.i) : kind == 1 ? ComparisonMask(a.j,b.j) :
                   kind == 2 ? ComparisonMask(a.f,b.f) : ComparisonMask(a.d,b.d);
      jint actual = invoke(a,b);
      if (env->ExceptionCheck() || actual != expected[index++] || actual != oracle) {
        std::cerr << "ART JIT comparison mismatch " << names[kind] << " case=" << index-1 << "\n";
        return false;
      }
      ++cases;
    }
  }
  std::cerr << "ART JIT comparisons: six relations IJFD including unordered NaN PASS cases=" << cases << "\n";
  return true;
}
}  // namespace aim_jni_acceptance_phase
