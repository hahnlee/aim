#include "jni/method_call.h"

#include <cassert>
#include <cstdint>
#include <cstdio>
#include <cstring>

#include <jni.h>

namespace {

constexpr uintptr_t kReceiverValue = 0x1111222233334444ull;
constexpr uintptr_t kMethodValue = 0x5555666677778888ull;
constexpr uintptr_t kInstanceObjectValue = 0x9999aaaabbbbccccull;
constexpr uintptr_t kStaticObjectValue = 0xddddeeeeffff0001ull;
constexpr uintptr_t kConstructedObjectValue = 0x123456789abcdef0ull;

jobject Receiver() { return reinterpret_cast<jobject>(kReceiverValue); }

jmethodID Method() { return reinterpret_cast<jmethodID>(kMethodValue); }

const jvalue *g_expected_args = nullptr;
int g_calls = 0;
bool g_pending_exception = false;

void CheckCall(JNIEnv *, jobject receiver, jmethodID method,
               const jvalue *args) {
  assert(receiver == Receiver());
  assert(method == Method());
  assert(args == g_expected_args);
  ++g_calls;
  // Model an exception produced by the invoked Java method, not invocation
  // with a pre-existing exception (which violates normal JNI preconditions).
  g_pending_exception = true;
}

void CheckStaticCall(JNIEnv *env, jclass receiver, jmethodID method,
                     const jvalue *args) {
  CheckCall(env, reinterpret_cast<jobject>(receiver), method, args);
}

jobject InstanceObject(JNIEnv *e, jobject r, jmethodID m, const jvalue *a) {
  CheckCall(e, r, m, a);
  return reinterpret_cast<jobject>(kInstanceObjectValue);
}
jboolean InstanceBoolean(JNIEnv *e, jobject r, jmethodID m, const jvalue *a) {
  CheckCall(e, r, m, a);
  return JNI_TRUE;
}
jbyte InstanceByte(JNIEnv *e, jobject r, jmethodID m, const jvalue *a) {
  CheckCall(e, r, m, a);
  return static_cast<jbyte>(-7);
}
jchar InstanceChar(JNIEnv *e, jobject r, jmethodID m, const jvalue *a) {
  CheckCall(e, r, m, a);
  return static_cast<jchar>(0x1234);
}
jshort InstanceShort(JNIEnv *e, jobject r, jmethodID m, const jvalue *a) {
  CheckCall(e, r, m, a);
  return static_cast<jshort>(-1234);
}
jint InstanceInt(JNIEnv *e, jobject r, jmethodID m, const jvalue *a) {
  CheckCall(e, r, m, a);
  return static_cast<jint>(-1234567);
}
jlong InstanceLong(JNIEnv *e, jobject r, jmethodID m, const jvalue *a) {
  CheckCall(e, r, m, a);
  return static_cast<jlong>(-0x11223344556677ll);
}
jfloat InstanceFloat(JNIEnv *e, jobject r, jmethodID m, const jvalue *a) {
  CheckCall(e, r, m, a);
  std::uint32_t bits = 0x7fc12345u;
  jfloat result = 0;
  std::memcpy(&result, &bits, sizeof(result));
  return result;
}
jdouble InstanceDouble(JNIEnv *e, jobject r, jmethodID m, const jvalue *a) {
  CheckCall(e, r, m, a);
  std::uint64_t bits = 0x7ff8000000001234ull;
  jdouble result = 0;
  std::memcpy(&result, &bits, sizeof(result));
  return result;
}
void InstanceVoid(JNIEnv *e, jobject r, jmethodID m, const jvalue *a) {
  CheckCall(e, r, m, a);
}

jobject StaticObject(JNIEnv *e, jclass r, jmethodID m, const jvalue *a) {
  CheckStaticCall(e, r, m, a);
  return reinterpret_cast<jobject>(kStaticObjectValue);
}
jboolean StaticBoolean(JNIEnv *e, jclass r, jmethodID m, const jvalue *a) {
  CheckStaticCall(e, r, m, a);
  return JNI_FALSE;
}
jbyte StaticByte(JNIEnv *e, jclass r, jmethodID m, const jvalue *a) {
  CheckStaticCall(e, r, m, a);
  return static_cast<jbyte>(-8);
}
jchar StaticChar(JNIEnv *e, jclass r, jmethodID m, const jvalue *a) {
  CheckStaticCall(e, r, m, a);
  return static_cast<jchar>(0x5678);
}
jshort StaticShort(JNIEnv *e, jclass r, jmethodID m, const jvalue *a) {
  CheckStaticCall(e, r, m, a);
  return static_cast<jshort>(-2345);
}
jint StaticInt(JNIEnv *e, jclass r, jmethodID m, const jvalue *a) {
  CheckStaticCall(e, r, m, a);
  return static_cast<jint>(-7654321);
}
jlong StaticLong(JNIEnv *e, jclass r, jmethodID m, const jvalue *a) {
  CheckStaticCall(e, r, m, a);
  return static_cast<jlong>(-0x22334455667788ll);
}
jfloat StaticFloat(JNIEnv *e, jclass r, jmethodID m, const jvalue *a) {
  CheckStaticCall(e, r, m, a);
  std::uint32_t bits = 0x3fc00001u;
  jfloat result = 0;
  std::memcpy(&result, &bits, sizeof(result));
  return result;
}
jdouble StaticDouble(JNIEnv *e, jclass r, jmethodID m, const jvalue *a) {
  CheckStaticCall(e, r, m, a);
  std::uint64_t bits = 0x4008000000000001ull;
  jdouble result = 0;
  std::memcpy(&result, &bits, sizeof(result));
  return result;
}
void StaticVoid(JNIEnv *e, jclass r, jmethodID m, const jvalue *a) {
  CheckStaticCall(e, r, m, a);
}

jobject Constructor(JNIEnv *e, jclass r, jmethodID m, const jvalue *a) {
  CheckStaticCall(e, r, m, a);
  return reinterpret_cast<jobject>(kConstructedObjectValue);
}

void ResetCallState(const jvalue *args) {
  g_expected_args = args;
  g_calls = 0;
  g_pending_exception = false;
}

void AssertOneCall(std::uint64_t actual, std::uint64_t expected) {
  assert(actual == expected);
  assert(g_calls == 1);
  assert(g_pending_exception);
}

void AssertNoCall(std::uint64_t actual) {
  assert(actual == 0);
  assert(g_calls == 0);
}

} // namespace

int main() {
  JNINativeInterface functions{};
  functions.CallObjectMethodA = InstanceObject;
  functions.CallBooleanMethodA = InstanceBoolean;
  functions.CallByteMethodA = InstanceByte;
  functions.CallCharMethodA = InstanceChar;
  functions.CallShortMethodA = InstanceShort;
  functions.CallIntMethodA = InstanceInt;
  functions.CallLongMethodA = InstanceLong;
  functions.CallFloatMethodA = InstanceFloat;
  functions.CallDoubleMethodA = InstanceDouble;
  functions.CallVoidMethodA = InstanceVoid;
  functions.CallStaticObjectMethodA = StaticObject;
  functions.CallStaticBooleanMethodA = StaticBoolean;
  functions.CallStaticByteMethodA = StaticByte;
  functions.CallStaticCharMethodA = StaticChar;
  functions.CallStaticShortMethodA = StaticShort;
  functions.CallStaticIntMethodA = StaticInt;
  functions.CallStaticLongMethodA = StaticLong;
  functions.CallStaticFloatMethodA = StaticFloat;
  functions.CallStaticDoubleMethodA = StaticDouble;
  functions.CallStaticVoidMethodA = StaticVoid;
  functions.NewObjectA = Constructor;
  // ExceptionClear intentionally remains null. The adapter must preserve the
  // callee's pending exception and perform no exception-state policy.
  JNIEnv env{&functions};

  jvalue arguments[3]{};
  arguments[0].i = 17;
  arguments[1].j = static_cast<jlong>(0x1122334455667788ll);
  arguments[2].l = reinterpret_cast<jobject>(uintptr_t{0xabc});
  const jvalue *args = arguments;

  using darwin_art::jni::CallMethodA;
  const jobject receiver = Receiver();
  const jmethodID method = Method();

  ResetCallState(args);
  AssertOneCall(CallMethodA(&env, receiver, method, args, 'L', 0),
                kInstanceObjectValue);
  ResetCallState(args);
  AssertOneCall(CallMethodA(&env, receiver, method, args, 'Z', 0), JNI_TRUE);
  ResetCallState(args);
  AssertOneCall(CallMethodA(&env, receiver, method, args, 'B', 0),
                static_cast<std::uint64_t>(static_cast<std::int64_t>(-7)));
  ResetCallState(args);
  AssertOneCall(CallMethodA(&env, receiver, method, args, 'C', 0), 0x1234);
  ResetCallState(args);
  AssertOneCall(CallMethodA(&env, receiver, method, args, 'S', 0),
                static_cast<std::uint64_t>(static_cast<std::int64_t>(-1234)));
  ResetCallState(args);
  AssertOneCall(
      CallMethodA(&env, receiver, method, args, 'I', 0),
      static_cast<std::uint64_t>(static_cast<std::int64_t>(-1234567)));
  ResetCallState(args);
  AssertOneCall(CallMethodA(&env, receiver, method, args, 'J', 0),
                static_cast<std::uint64_t>(
                    static_cast<std::int64_t>(-0x11223344556677ll)));
  ResetCallState(args);
  AssertOneCall(CallMethodA(&env, receiver, method, args, 'F', 0), 0x7fc12345u);
  ResetCallState(args);
  AssertOneCall(CallMethodA(&env, receiver, method, args, 'D', 0),
                0x7ff8000000001234ull);
  ResetCallState(args);
  AssertOneCall(CallMethodA(&env, receiver, method, args, 'V', 0), 0);

  ResetCallState(args);
  AssertOneCall(CallMethodA(&env, receiver, method, args, 'L', 1),
                kStaticObjectValue);
  ResetCallState(args);
  AssertOneCall(CallMethodA(&env, receiver, method, args, 'Z', 1), JNI_FALSE);
  ResetCallState(args);
  AssertOneCall(CallMethodA(&env, receiver, method, args, 'B', 1),
                static_cast<std::uint64_t>(static_cast<std::int64_t>(-8)));
  ResetCallState(args);
  AssertOneCall(CallMethodA(&env, receiver, method, args, 'C', 1), 0x5678);
  ResetCallState(args);
  AssertOneCall(CallMethodA(&env, receiver, method, args, 'S', 1),
                static_cast<std::uint64_t>(static_cast<std::int64_t>(-2345)));
  ResetCallState(args);
  AssertOneCall(
      CallMethodA(&env, receiver, method, args, 'I', 1),
      static_cast<std::uint64_t>(static_cast<std::int64_t>(-7654321)));
  ResetCallState(args);
  AssertOneCall(CallMethodA(&env, receiver, method, args, 'J', 1),
                static_cast<std::uint64_t>(
                    static_cast<std::int64_t>(-0x22334455667788ll)));
  ResetCallState(args);
  AssertOneCall(CallMethodA(&env, receiver, method, args, 'F', 1), 0x3fc00001u);
  ResetCallState(args);
  AssertOneCall(CallMethodA(&env, receiver, method, args, 'D', 1),
                0x4008000000000001ull);
  ResetCallState(args);
  AssertOneCall(CallMethodA(&env, receiver, method, args, 'V', 1), 0);

  ResetCallState(args);
  AssertOneCall(CallMethodA(&env, receiver, method, args, 'L', 2),
                kConstructedObjectValue);
  ResetCallState(nullptr);
  AssertOneCall(CallMethodA(&env, receiver, method, nullptr, 'V', 0), 0);

  ResetCallState(args);
  AssertNoCall(CallMethodA(&env, receiver, method, args, 'X', 0));
  ResetCallState(args);
  AssertNoCall(CallMethodA(&env, receiver, method, args, 'L', 3));
  ResetCallState(args);
  AssertNoCall(CallMethodA(&env, receiver, method, args, 'I', 2));
  ResetCallState(args);
  AssertNoCall(CallMethodA(&env, receiver, method, args, 'V', -1));
  ResetCallState(args);
  AssertNoCall(CallMethodA(nullptr, receiver, method, args, 'I', 0));
  ResetCallState(args);
  AssertNoCall(CallMethodA(&env, nullptr, method, args, 'I', 0));
  ResetCallState(args);
  AssertNoCall(CallMethodA(&env, receiver, nullptr, args, 'I', 0));

  std::puts("android-jni-method-call: PASS A-slots instance-static-constructor "
            "signed-results fp-bits pending-preserved invalid-null-guards");
  return 0;
}
