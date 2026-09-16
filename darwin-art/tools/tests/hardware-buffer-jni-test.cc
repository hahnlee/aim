#include <android/hardware_buffer.h>
#include <android/hardware_buffer_jni.h>
#include <cassert>
#include <cstdarg>
#include <cstdio>
#include <cstring>

// Test-only resource spy; the production module links the IOSurface owner.
struct AHardwareBuffer { int refs = 1; };
extern "C" void AHardwareBuffer_acquire(AHardwareBuffer* b) { ++b->refs; }
extern "C" void AHardwareBuffer_release(AHardwareBuffer* b) { --b->refs; assert(b->refs >= 0); }
extern "C" int AHardwareBuffer_allocate(const AHardwareBuffer_Desc*, AHardwareBuffer**) { return -12; }
extern "C" int AHardwareBuffer_isSupported(const AHardwareBuffer_Desc*) { return 0; }
extern "C" void AHardwareBuffer_describe(const AHardwareBuffer*, AHardwareBuffer_Desc* d) { *d = {}; }
namespace {
bool pending, correct_class = true, fail_create;
jlong value;
int clears;
jclass Find(JNIEnv*, const char*) { return reinterpret_cast<jclass>(1); }
jclass Class(JNIEnv*, jobject) { return reinterpret_cast<jclass>(1); }
jboolean Instance(JNIEnv*, jobject object, jclass) { return object && correct_class; }
jfieldID Field(JNIEnv*, jclass, const char* name, const char* type) {
  assert(std::strcmp(name, "mNativeObject") == 0 && std::strcmp(type, "J") == 0);
  return reinterpret_cast<jfieldID>(2);
}
jmethodID Method(JNIEnv*, jclass, const char* name, const char* type) {
  assert(std::strcmp(name, "<init>") == 0 && std::strcmp(type, "(J)V") == 0);
  return reinterpret_cast<jmethodID>(3);
}
jlong Value(JNIEnv*, jobject, jfieldID) { return value; }
jobject New(JNIEnv*, jclass, jmethodID, va_list args) {
  value = va_arg(args, jlong);
  if (fail_create) { pending = true; return nullptr; }
  return reinterpret_cast<jobject>(4);
}
jboolean Exception(JNIEnv*) { return pending; }
void Clear(JNIEnv*) { ++clears; pending = false; }
void Delete(JNIEnv*, jobject) {}
jint Throw(JNIEnv*, jclass, const char*) { pending = true; return 0; }
}
int main() {
  JNINativeInterface table{};
  table.FindClass = Find; table.GetObjectClass = Class; table.IsInstanceOf = Instance;
  table.GetFieldID = Field; table.GetLongField = Value; table.GetMethodID = Method;
  table.NewObjectV = New; table.ExceptionCheck = Exception; table.ExceptionClear = Clear;
  table.DeleteLocalRef = Delete; table.ThrowNew = Throw;
  JNIEnv env{&table};
  AHardwareBuffer buffer;
  auto object = reinterpret_cast<jobject>(4);
  assert(!AHardwareBuffer_fromHardwareBuffer(nullptr, object));
  assert(!AHardwareBuffer_fromHardwareBuffer(&env, nullptr));
  value = reinterpret_cast<jlong>(&buffer);
  assert(AHardwareBuffer_fromHardwareBuffer(&env, object) == &buffer && buffer.refs == 1);
  correct_class = false;
  assert(!AHardwareBuffer_fromHardwareBuffer(&env, object));
  correct_class = true;
  value = 0;
  assert(!AHardwareBuffer_fromHardwareBuffer(&env, object));
  assert(AHardwareBuffer_toHardwareBuffer(&env, &buffer) == object && buffer.refs == 2);
  AHardwareBuffer_release(&buffer); // Java finalizer's owned reference.
  fail_create = true;
  assert(!AHardwareBuffer_toHardwareBuffer(&env, &buffer) && buffer.refs == 1);
  assert(pending && clears == 0);
  assert(!AHardwareBuffer_toHardwareBuffer(&env, &buffer) && buffer.refs == 1);
  assert(!AHardwareBuffer_fromHardwareBuffer(&env, object));
  std::puts("HardwareBuffer JNI: borrowed from-Java, owned to-Java, failed constructor release, exceptions PASS");
}
