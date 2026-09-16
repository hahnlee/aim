#include "jni/art_registration.h"

#include <jni.h>

#include <array>
#include <cassert>
#include <cstddef>
#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <new>

namespace {

bool g_fail_allocations = false;

void *Allocate(std::size_t size) {
  if (g_fail_allocations)
    throw std::bad_alloc();
  if (void *memory = std::malloc(size == 0 ? 1 : size))
    return memory;
  throw std::bad_alloc();
}

} // namespace

void *operator new(std::size_t size) { return Allocate(size); }
void *operator new[](std::size_t size) { return Allocate(size); }
void operator delete(void *pointer) noexcept { std::free(pointer); }
void operator delete[](void *pointer) noexcept { std::free(pointer); }
void operator delete(void *pointer, std::size_t) noexcept {
  std::free(pointer);
}
void operator delete[](void *pointer, std::size_t) noexcept {
  std::free(pointer);
}

namespace {

constexpr uintptr_t kClassValue = 0x1111222233334444ull;
constexpr uintptr_t kFunctionOne = 0x5555666677778888ull;
constexpr uintptr_t kFunctionTwo = 0x9999aaaabbbbccccull;
constexpr jint kRegistrationStatus = 37;

jclass ClassValue() { return reinterpret_cast<jclass>(kClassValue); }
void *FunctionOne() { return reinterpret_cast<void *>(kFunctionOne); }
void *FunctionTwo() { return reinterpret_cast<void *>(kFunctionTwo); }

int g_register_calls = 0;
JNIEnv *g_register_env = nullptr;
jclass g_register_class = nullptr;
std::array<JNINativeMethod, 2> g_register_copy{};
jint g_register_count = -1;
jint g_return_status = JNI_OK;
bool g_pending_exception = false;

jint RegisterNatives(JNIEnv *env, jclass clazz, const JNINativeMethod *methods,
                     jint count) {
  ++g_register_calls;
  g_register_env = env;
  g_register_class = clazz;
  g_register_count = count;
  assert(count >= 0 && count <= static_cast<jint>(g_register_copy.size()));
  if (count == 0) {
    assert(methods == nullptr);
  } else {
    assert(methods != nullptr);
    for (jint index = 0; index < count; ++index)
      g_register_copy[static_cast<std::size_t>(index)] = methods[index];
  }
  return g_return_status;
}

jboolean ExceptionCheck(JNIEnv *) {
  return g_pending_exception ? JNI_TRUE : JNI_FALSE;
}

jclass FindClass(JNIEnv *, const char *name) {
  assert(name != nullptr);
  assert(std::strcmp(name, "java/lang/OutOfMemoryError") == 0);
  return ClassValue();
}

jint ThrowNew(JNIEnv *, jclass clazz, const char *message) {
  assert(clazz == ClassValue());
  assert(message != nullptr);
  g_pending_exception = true;
  return JNI_OK;
}

void DeleteLocalRef(JNIEnv *, jobject object) {
  assert(object == ClassValue());
}

void ResetObservations() {
  g_register_calls = 0;
  g_register_env = nullptr;
  g_register_class = nullptr;
  g_register_copy = {};
  g_register_count = -1;
  g_return_status = JNI_OK;
  g_pending_exception = false;
}

JNIEnv MakeRegistrationEnv() {
  static JNINativeInterface functions{};
  functions = JNINativeInterface{};
  functions.RegisterNatives = RegisterNatives;
  return JNIEnv{&functions};
}

JNIEnv MakeAllocationFailureEnv() {
  static JNINativeInterface functions{};
  functions = JNINativeInterface{};
  functions.RegisterNatives = RegisterNatives;
  functions.ExceptionCheck = ExceptionCheck;
  functions.FindClass = FindClass;
  functions.ThrowNew = ThrowNew;
  functions.DeleteLocalRef = DeleteLocalRef;
  return JNIEnv{&functions};
}

void TestExactForwardingAndStatus() {
  JNIEnv env = MakeRegistrationEnv();
  darwin_art::jni::ArtRegistration registration;
  const char name_one[] = "first";
  const char signature_one[] = "(I)V";
  const char name_two[] = "second";
  const char signature_two[] = "(Ljava/lang/Object;)I";
  const DarwinArtJniNativeMethod methods[] = {
      {name_one, signature_one, FunctionOne()},
      {name_two, signature_two, FunctionTwo()},
  };

  ResetObservations();
  g_return_status = kRegistrationStatus;
  assert(registration.Register(&env, ClassValue(), methods, 2) ==
         kRegistrationStatus);
  assert(g_register_calls == 1);
  assert(g_register_env == &env);
  assert(g_register_class == ClassValue());
  assert(g_register_count == 2);
  assert(g_register_copy[0].name == name_one);
  assert(g_register_copy[0].signature == signature_one);
  assert(g_register_copy[0].fnPtr == FunctionOne());
  assert(g_register_copy[1].name == name_two);
  assert(g_register_copy[1].signature == signature_two);
  assert(g_register_copy[1].fnPtr == FunctionTwo());
  assert(!g_pending_exception);

  // A pending exception and ART's exact status pass through untouched.  The
  // only installed callback is RegisterNatives, so lookup/clear/rollback
  // would be an immediate test failure.
  ResetObservations();
  g_pending_exception = true;
  g_return_status = JNI_ERR;
  assert(registration.Register(&env, ClassValue(), methods, 2) == JNI_ERR);
  assert(g_register_calls == 1);
  assert(g_register_count == 2);
  assert(g_pending_exception);
}

void TestZeroCountAndInvalidInputs() {
  JNIEnv env = MakeRegistrationEnv();
  darwin_art::jni::ArtRegistration registration;
  const char name[] = "one";
  const char signature[] = "()V";
  const DarwinArtJniNativeMethod valid[] = {{name, signature, FunctionOne()}};

  ResetObservations();
  assert(registration.Register(&env, ClassValue(), nullptr, 0) == JNI_OK);
  assert(g_register_calls == 1);
  assert(g_register_count == 0);

  assert(registration.Register(nullptr, ClassValue(), valid, 1) == JNI_ERR);
  assert(registration.Register(&env, nullptr, valid, 1) == JNI_ERR);
  assert(registration.Register(&env, ClassValue(), valid, -1) == JNI_ERR);
  assert(registration.Register(&env, ClassValue(), nullptr, 1) == JNI_ERR);

  const DarwinArtJniNativeMethod missing_name[] = {
      {nullptr, signature, FunctionOne()}};
  const DarwinArtJniNativeMethod missing_signature[] = {
      {name, nullptr, FunctionOne()}};
  const DarwinArtJniNativeMethod missing_function[] = {
      {name, signature, nullptr}};
  g_return_status = kRegistrationStatus;
  assert(registration.Register(&env, ClassValue(), missing_name, 1) ==
         kRegistrationStatus);
  assert(g_register_copy[0].name == nullptr);
  assert(g_register_copy[0].signature == signature);
  assert(g_register_copy[0].fnPtr == FunctionOne());
  assert(registration.Register(&env, ClassValue(), missing_signature, 1) ==
         kRegistrationStatus);
  assert(g_register_copy[0].name == name);
  assert(g_register_copy[0].signature == nullptr);
  assert(g_register_copy[0].fnPtr == FunctionOne());
  assert(registration.Register(&env, ClassValue(), missing_function, 1) ==
         kRegistrationStatus);
  assert(g_register_copy[0].name == name);
  assert(g_register_copy[0].signature == signature);
  assert(g_register_copy[0].fnPtr == nullptr);
  assert(g_register_calls == 4);
}

void TestAllocationFailureTranslation() {
  JNIEnv env = MakeAllocationFailureEnv();
  darwin_art::jni::ArtRegistration registration;
  const char name[] = "one";
  const char signature[] = "()V";
  const DarwinArtJniNativeMethod valid[] = {{name, signature, FunctionOne()}};

  // With no pending exception, a table-allocation failure becomes OOME.
  g_pending_exception = false;
  g_fail_allocations = true;
  assert(registration.Register(&env, ClassValue(), valid, 1) == JNI_ERR);
  g_fail_allocations = false;
  assert(g_pending_exception);

  // An existing exception is preserved and no replacement lookup is attempted.
  g_pending_exception = true;
  g_fail_allocations = true;
  assert(registration.Register(&env, ClassValue(), valid, 1) == JNI_ERR);
  g_fail_allocations = false;
  assert(g_pending_exception);
}

} // namespace

int main() {
  TestExactForwardingAndStatus();
  TestZeroCountAndInvalidInputs();
  TestAllocationFailureTranslation();
  std::puts("android-jni-art-registration: PASS direct-forward count0 status "
            "pending-exception allocation-failure");
  return 0;
}
