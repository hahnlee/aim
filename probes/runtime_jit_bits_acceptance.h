#pragma once
#include <limits>
#include <vector>
namespace aim_jni_acceptance_phase {
inline bool CheckJitBits(JNIEnv* env, art::Thread* self, art::jit::Jit* jit,
                         art::Handle<art::mirror::Class> owner, jclass java_owner) {
  const char* names[] = {"jitAndLong", "jitOrLong", "jitXorLong", "jitShlLong", "jitShrLong", "jitUshrLong"};
  const jlong values[] = {0, 1, -1, std::numeric_limits<jlong>::min(), std::numeric_limits<jlong>::max(),
                         0x123456789abcdefLL, 0x5555555555555555LL};
  const jint shifts[] = {-65,-64,-1,0,1,31,32,63,64,65,127,std::numeric_limits<jint>::min(),std::numeric_limits<jint>::max()};
  unsigned cases = 0;
  for (unsigned op = 0; op < 6; ++op) {
    const char* sig = op < 3 ? "(JJ)J" : "(JI)J";
    jmethodID id = env->GetStaticMethodID(java_owner, names[op], sig);
    auto* method = owner->FindClassMethod(names[op], sig, art::kRuntimePointerSize);
    if (!id || !method || env->ExceptionCheck() || jit->GetCodeCache()->ContainsPc(method->GetEntryPointFromQuickCompiledCode())) return false;
    std::vector<jlong> rhs;
    if (op < 3) for (jlong value : values) rhs.push_back(value);
    else for (jint value : shifts) rhs.push_back(value);
    auto invoke = [&](jlong a, jlong b) {
      jvalue args[2] = {};
      args[0].j = a;
      if (op < 3) args[1].j = b; else args[1].i = static_cast<jint>(b);
      jlong value = env->CallStaticLongMethodA(java_owner, id, args);
      return static_cast<uint64_t>(value);
    };
    std::vector<uint64_t> expected;
    for (jlong a : values) for (jlong b : rhs) expected.push_back(invoke(a,b));
    if (env->ExceptionCheck() || jit->GetCodeCache()->ContainsPc(method->GetEntryPointFromQuickCompiledCode()) ||
        !jit->CompileMethod(method, self, art::CompilationKind::kOptimized, false) ||
        !jit->GetCodeCache()->ContainsPc(method->GetEntryPointFromQuickCompiledCode())) return false;
    size_t index = 0;
    for (jlong a : values) for (jlong b : rhs) {
      uint64_t x = static_cast<uint64_t>(a), y = static_cast<uint64_t>(b), oracle = 0;
      unsigned shift = static_cast<uint32_t>(b) & 63u;
      switch (op) {
        case 0: oracle = x & y; break;
        case 1: oracle = x | y; break;
        case 2: oracle = x ^ y; break;
        case 3: oracle = x << shift; break;
        case 4:
          oracle = x >> shift;
          if (a < 0 && shift != 0) oracle |= (~uint64_t{0}) << (64 - shift);
          break;
        case 5: oracle = x >> shift; break;
      }
      uint64_t actual = invoke(a,b);
      if (env->ExceptionCheck() || actual != expected[index++] || actual != oracle) {
        std::cerr << "ART JIT bits mismatch " << names[op] << " case=" << index - 1 << "\n";
        return false;
      }
      ++cases;
    }
  }
  std::cerr << "ART JIT long bits: independent masked-shift and interpreter oracles PASS cases=" << cases << "\n";
  return true;
}
}  // namespace aim_jni_acceptance_phase
