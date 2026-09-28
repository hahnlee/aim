#pragma once
#include <cmath>
#include <limits>
#include <vector>

namespace aim_jni_acceptance_phase {
inline bool CheckJitNarrow(JNIEnv* env, art::Thread* self, art::jit::Jit* jit,
                          art::Handle<art::mirror::Class> owner, jclass java_owner) {
  const jint samples[] = {0, 1, -1, 127, 128, 255, 256, -128, -129, -256,
      32767, 32768, 65535, 65536, -32768, -32769, -65536, 0x12345678,
      std::numeric_limits<jint>::min(), std::numeric_limits<jint>::max()};
  for (char type : std::string_view("BCS")) {
    std::string name = std::string("jitNarrow") + type;
    std::string signature = std::string("(I)") + type;
    jmethodID id = env->GetStaticMethodID(java_owner, name.c_str(), signature.c_str());
    auto* method = owner->FindClassMethod(name.c_str(), signature.c_str(), art::kRuntimePointerSize);
    if (!id || !method || env->ExceptionCheck() || jit->GetCodeCache()->ContainsPc(method->GetEntryPointFromQuickCompiledCode())) return false;
    auto invoke = [&](jint input) -> jint {
      if (type == 'B') return env->CallStaticByteMethod(java_owner, id, input);
      if (type == 'C') return env->CallStaticCharMethod(java_owner, id, input);
      return env->CallStaticShortMethod(java_owner, id, input);
    };
    std::vector<jint> expected;
    for (jint input : samples) expected.push_back(invoke(input));
    if (env->ExceptionCheck() || jit->GetCodeCache()->ContainsPc(method->GetEntryPointFromQuickCompiledCode()) ||
        !jit->CompileMethod(method, self, art::CompilationKind::kOptimized, false) ||
        !jit->GetCodeCache()->ContainsPc(method->GetEntryPointFromQuickCompiledCode())) return false;
    size_t index = 0;
    for (jint input : samples) {
      uint32_t low = static_cast<uint32_t>(input) & (type == 'B' ? 255u : 65535u);
      jint independent = static_cast<jint>(low);
      if (type == 'B' && low >= 128) independent -= 256;
      if (type == 'S' && low >= 32768) independent -= 65536;
      jint actual = invoke(input);
      if (env->ExceptionCheck() || actual != expected[index++] || actual != independent) return false;
    }
  }
  std::cerr << "ART JIT narrowing: byte/char/short signedness and truncation PASS cases=60\n";
  return true;
}
inline bool CheckJitConversions(JNIEnv* env, art::Thread* self, art::jit::Jit* jit,
                                art::Handle<art::mirror::Class> owner, jclass java_owner) {
  const char* types = "IJFD";
  unsigned cases = 0;
  for (char source : std::string_view(types)) for (char target : std::string_view(types)) {
    if (source == target) continue;
    const std::string name = std::string("jitConvert") + source + target;
    const std::string signature = std::string("(") + source + ")" + target;
    jmethodID id = env->GetStaticMethodID(java_owner, name.c_str(), signature.c_str());
    auto* method = owner->FindClassMethod(name.c_str(), signature.c_str(), art::kRuntimePointerSize);
    if (!id || !method || env->ExceptionCheck() || jit->GetCodeCache()->ContainsPc(method->GetEntryPointFromQuickCompiledCode())) return false;
    std::vector<jvalue> samples;
    if (source == 'I' || source == 'J') {
      for (jlong value : {jlong{0}, jlong{1}, jlong{-1}, jlong{123456789}, jlong{-123456789},
                         jlong{2147483647}, jlong{-2147483648LL}, std::numeric_limits<jlong>::min(),
                         std::numeric_limits<jlong>::max(), jlong{0x123456789abcdefLL}}) {
        jvalue input{};
        if (source == 'I') input.i = static_cast<jint>(value); else input.j = value;
        samples.push_back(input);
      }
    } else {
      for (double value : {0.0, -0.0, 0.5, -0.5, 1.0, -1.0, 2147483647.0, 2147483648.0,
                            -2147483649.0, 9223372036854775808.0, -9223372036854775808.0,
                            std::nextafter(9223372036854775808.0, 0.0),
                            std::numeric_limits<double>::infinity(), -std::numeric_limits<double>::infinity(),
                            std::numeric_limits<double>::quiet_NaN()}) {
        jvalue input{};
        if (source == 'F') input.f = static_cast<float>(value); else input.d = value;
        samples.push_back(input);
      }
      jvalue tiny{}, huge{};
      if (source == 'F') { tiny.f = std::numeric_limits<float>::denorm_min(); huge.f = std::numeric_limits<float>::max(); }
      else { tiny.d = std::numeric_limits<double>::denorm_min(); huge.d = std::numeric_limits<double>::max(); }
      samples.push_back(tiny); samples.push_back(huge);
    }
    struct Result { uint64_t bits = 0; bool nan = false; };
    auto invoke = [&](jvalue input) {
      Result result;
      if (target == 'I') { jint v = env->CallStaticIntMethodA(java_owner, id, &input); std::memcpy(&result.bits, &v, sizeof(v)); }
      if (target == 'J') { jlong v = env->CallStaticLongMethodA(java_owner, id, &input); std::memcpy(&result.bits, &v, sizeof(v)); }
      if (target == 'F') { float v = env->CallStaticFloatMethodA(java_owner, id, &input); result.nan = std::isnan(v); std::memcpy(&result.bits, &v, sizeof(v)); }
      if (target == 'D') { double v = env->CallStaticDoubleMethodA(java_owner, id, &input); result.nan = std::isnan(v); std::memcpy(&result.bits, &v, sizeof(v)); }
      return result;
    };
    std::vector<Result> expected;
    for (jvalue input : samples) expected.push_back(invoke(input));
    if (env->ExceptionCheck() || jit->GetCodeCache()->ContainsPc(method->GetEntryPointFromQuickCompiledCode()) ||
        !jit->CompileMethod(method, self, art::CompilationKind::kOptimized, false) ||
        !jit->GetCodeCache()->ContainsPc(method->GetEntryPointFromQuickCompiledCode())) {
      std::cerr << "ART JIT conversion compile failed " << name << "\n";
      return false;
    }
    for (size_t i = 0; i < samples.size(); ++i) {
      Result actual = invoke(samples[i]);
      if (env->ExceptionCheck() || actual.nan != expected[i].nan || (!actual.nan && actual.bits != expected[i].bits)) {
        std::cerr << "ART JIT conversion mismatch " << name << " sample=" << i << "\n";
        return false;
      }
      ++cases;
    }
  }
  std::cerr << "ART JIT conversions: all 12 IJFD directions differential PASS cases=" << cases << "\n";
  return CheckJitNarrow(env, self, jit, owner, java_owner);
}
inline bool CheckJitNumeric(JNIEnv* env, art::Thread* self, art::jit::Jit* jit,
                             art::Handle<art::mirror::Class> owner, jclass java_owner) {
  const char* names[] = {"jitNumericLong", "jitNumericFloat", "jitNumericDouble",
      "jitDivideLong", "jitDivideFloat", "jitDivideDouble",
      "jitRemainderLong", "jitRemainderFloat", "jitRemainderDouble"};
  const char* signatures[] = {"(JJ)J", "(FF)F", "(DD)D"};
  struct Result { uint64_t bits = 0; bool nan = false; bool exception = false; };
  for (unsigned test = 0; test < 9; ++test) {
    const unsigned kind = test % 3;
    const char* name = names[test];
    jmethodID id = env->GetStaticMethodID(java_owner, name, signatures[kind]);
    auto* method = owner->FindClassMethod(name, signatures[kind], art::kRuntimePointerSize);
    if (!id || !method || env->ExceptionCheck() || jit->GetCodeCache()->ContainsPc(method->GetEntryPointFromQuickCompiledCode())) return false;
    std::vector<jvalue> values;
    if (kind == 0) {
      for (jlong value : {jlong{0}, jlong{1}, jlong{-1}, jlong{17}, jlong{-29},
                         std::numeric_limits<jlong>::min(), std::numeric_limits<jlong>::max(),
                         jlong{0x123456789abcdefLL}}) { jvalue v{}; v.j = value; values.push_back(v); }
    } else {
      for (double value : {0.0, -0.0, 1.0, -1.0, 17.25, -29.5,
                            std::numeric_limits<double>::infinity(),
                            -std::numeric_limits<double>::infinity(),
                            std::numeric_limits<double>::quiet_NaN()}) {
        jvalue v{};
        if (kind == 1) v.f = static_cast<float>(value); else v.d = value;
        values.push_back(v);
      }
      jvalue small{}, large{};
      if (kind == 1) { small.f = std::numeric_limits<float>::denorm_min(); large.f = std::numeric_limits<float>::max(); }
      else { small.d = std::numeric_limits<double>::denorm_min(); large.d = std::numeric_limits<double>::max(); }
      values.push_back(small); values.push_back(large);
    }
    bool valid = true;
    auto invoke = [&](jvalue a, jvalue b) {
      jvalue args[] = {a, b};
      Result result;
      if (kind == 0) { jlong v = env->CallStaticLongMethodA(java_owner, id, args); std::memcpy(&result.bits, &v, sizeof(v)); }
      else if (kind == 1) { float v = env->CallStaticFloatMethodA(java_owner, id, args); result.nan = std::isnan(v); std::memcpy(&result.bits, &v, sizeof(v)); }
      else { double v = env->CallStaticDoubleMethodA(java_owner, id, args); result.nan = std::isnan(v); std::memcpy(&result.bits, &v, sizeof(v)); }
      if (env->ExceptionCheck()) {
        result.exception = true;
        valid &= kind == 0 && ClearExpectedArrayException(env, "java/lang/ArithmeticException");
      }
      return result;
    };
    std::vector<Result> expected;
    for (jvalue a : values) for (jvalue b : values) expected.push_back(invoke(a, b));
    if (!valid || jit->GetCodeCache()->ContainsPc(method->GetEntryPointFromQuickCompiledCode()) ||
        !jit->CompileMethod(method, self, art::CompilationKind::kOptimized, false) ||
        !jit->GetCodeCache()->ContainsPc(method->GetEntryPointFromQuickCompiledCode())) {
      std::cerr << "ART JIT numeric compile failed " << name << "\n";
      return false;
    }
    size_t index = 0;
    for (jvalue a : values) for (jvalue b : values) {
      Result actual = invoke(a, b), reference = expected[index++];
      if (!valid || actual.exception != reference.exception ||
          (!actual.exception && (actual.nan != reference.nan || (!actual.nan && actual.bits != reference.bits)))) {
        std::cerr << "ART JIT numeric mismatch " << name << " case=" << index - 1 << "\n";
        return false;
      }
    }
    std::cerr << "ART JIT numeric: " << name << " interpreter differential PASS cases=" << expected.size() << "\n";
  }
  return CheckJitConversions(env, self, jit, owner, java_owner);
}
}  // namespace aim_jni_acceptance_phase
