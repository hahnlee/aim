#include "method_call.h"

#include <cstdint>
#include <cstring>

namespace darwin_art::jni {
namespace {

constexpr int32_t kInstance = 0;
constexpr int32_t kStatic = 1;
constexpr int32_t kConstructor = 2;

bool IsReturnShorty(int32_t shorty) {
  switch (shorty) {
    case 'Z':
    case 'B':
    case 'C':
    case 'S':
    case 'I':
    case 'J':
    case 'F':
    case 'D':
    case 'L':
    case 'V':
      return true;
    default:
      return false;
  }
}

uint64_t FloatBits(jfloat value) {
  uint32_t bits = 0;
  std::memcpy(&bits, &value, sizeof(bits));
  return bits;
}

uint64_t DoubleBits(jdouble value) {
  uint64_t bits = 0;
  std::memcpy(&bits, &value, sizeof(bits));
  return bits;
}

}  // namespace

uint64_t CallMethodA(JNIEnv* env,
                     jobject receiver,
                     jmethodID method,
                     const jvalue* args,
                     int32_t return_shorty,
                     int32_t invocation_kind) {
  if (env == nullptr || receiver == nullptr || method == nullptr ||
      !IsReturnShorty(return_shorty) ||
      (invocation_kind != kInstance && invocation_kind != kStatic &&
       invocation_kind != kConstructor)) {
    return 0;
  }
  if (invocation_kind == kConstructor) {
    if (return_shorty != 'L') return 0;
    return reinterpret_cast<uint64_t>(
        env->NewObjectA(static_cast<jclass>(receiver), method, args));
  }
  if (invocation_kind == kStatic) {
    switch (return_shorty) {
      case 'L':
        return reinterpret_cast<uint64_t>(
            env->CallStaticObjectMethodA(static_cast<jclass>(receiver), method,
                                         args));
      case 'Z':
        return env->CallStaticBooleanMethodA(static_cast<jclass>(receiver),
                                             method, args);
      case 'B':
        return static_cast<uint64_t>(
            env->CallStaticByteMethodA(static_cast<jclass>(receiver), method,
                                       args));
      case 'C':
        return env->CallStaticCharMethodA(static_cast<jclass>(receiver), method,
                                          args);
      case 'S':
        return static_cast<uint64_t>(
            env->CallStaticShortMethodA(static_cast<jclass>(receiver), method,
                                        args));
      case 'I':
        return static_cast<uint64_t>(
            env->CallStaticIntMethodA(static_cast<jclass>(receiver), method,
                                      args));
      case 'J':
        return static_cast<uint64_t>(
            env->CallStaticLongMethodA(static_cast<jclass>(receiver), method,
                                       args));
      case 'F':
        return FloatBits(env->CallStaticFloatMethodA(
            static_cast<jclass>(receiver), method, args));
      case 'D':
        return DoubleBits(env->CallStaticDoubleMethodA(
            static_cast<jclass>(receiver), method, args));
      case 'V':
        env->CallStaticVoidMethodA(static_cast<jclass>(receiver), method, args);
        return 0;
      default:
        return 0;
    }
  }
  switch (return_shorty) {
    case 'L':
      return reinterpret_cast<uint64_t>(
          env->CallObjectMethodA(receiver, method, args));
    case 'Z':
      return env->CallBooleanMethodA(receiver, method, args);
    case 'B':
      return static_cast<uint64_t>(env->CallByteMethodA(receiver, method, args));
    case 'C':
      return env->CallCharMethodA(receiver, method, args);
    case 'S':
      return static_cast<uint64_t>(
          env->CallShortMethodA(receiver, method, args));
    case 'I':
      return static_cast<uint64_t>(env->CallIntMethodA(receiver, method, args));
    case 'J':
      return static_cast<uint64_t>(env->CallLongMethodA(receiver, method, args));
    case 'F':
      return FloatBits(env->CallFloatMethodA(receiver, method, args));
    case 'D':
      return DoubleBits(env->CallDoubleMethodA(receiver, method, args));
    case 'V':
      env->CallVoidMethodA(receiver, method, args);
      return 0;
    default:
      return 0;
  }
}

}  // namespace darwin_art::jni
