#include "runtime/framework/looper/message_queue_jni.h"
#include <cassert>
#include <cstdint>
#include <thread>

namespace {
bool pending = false;
bool fail_read = false;
bool dispose_during_read = false;
bool dispose_during_wake = false;
bool dispose_during_poll = false;
int wakes = 0;
int lookups = 0;
int prepares = 0;
jlong handle = 0;
void* current_owner = reinterpret_cast<void*>(0x1000);
jboolean ExceptionCheck(JNIEnv*) { return pending ? JNI_TRUE : JNI_FALSE; }
jclass GetObjectClass(JNIEnv*, jobject) {
  ++lookups;
  return reinterpret_cast<jclass>(0x20);
}
jfieldID GetFieldID(JNIEnv*, jclass, const char*, const char*) {
  return reinterpret_cast<jfieldID>(0x30);
}
void DeleteLocalRef(JNIEnv*, jobject) {}
jlong GetLongField(JNIEnv* env, jobject, jfieldID) {
  if (fail_read) {
    pending = true;
    return 1; // Must never dereference a value returned with an exception.
  }
  if (dispose_during_read) {
    std::thread quitter([&] {
      darwin_art::framework_system::message_queue_native_destroy(env, nullptr, handle);
    });
    quitter.join();
  }
  return handle;
}
}
extern "C" void* darwin_art_android_platform_prepare_current_looper() {
  ++prepares;
  return current_owner;
}
extern "C" int darwin_art_android_platform_poll_current_looper_timeout(int) {
  if (dispose_during_poll) {
    std::thread quitter([] {
      darwin_art::framework_system::message_queue_native_destroy(nullptr, nullptr, handle);
    });
    quitter.join();
  }
  return 0;
}
extern "C" void darwin_art_android_platform_wake_looper(void*) {
  ++wakes;
  if (dispose_during_wake)
    darwin_art::framework_system::message_queue_native_destroy(nullptr, nullptr, handle);
}

int main() {
  JNINativeInterface table{};
  table.ExceptionCheck = ExceptionCheck;
  table.GetObjectClass = GetObjectClass;
  table.GetFieldID = GetFieldID;
  table.DeleteLocalRef = DeleteLocalRef;
  table.GetLongField = GetLongField;
  JNIEnv env{&table};
  auto queue = reinterpret_cast<jobject>(0x10);
  using namespace darwin_art::framework_system;
  handle = message_queue_native_init(&env, nullptr);
  assert(handle != 0 && prepares == 1);
  current_owner = reinterpret_cast<void*>(0x2000);
  assert(message_queue_looper(&env, queue) == reinterpret_cast<void*>(0x1000));
  assert(prepares == 1); // Lookup never prepares the caller's different Looper.
  const int previous_lookups = lookups;
  pending = true;
  assert(message_queue_looper(&env, queue) == nullptr);
  assert(pending && lookups == previous_lookups);
  pending = false;
  fail_read = true;
  assert(message_queue_looper(&env, queue) == nullptr && pending);
  fail_read = false;
  pending = false;
  dispose_during_read = true;
  assert(message_queue_looper(&env, queue) == nullptr);
  dispose_during_read = false;
  const jlong retired_handle = handle;
  handle = message_queue_native_init(&env, nullptr);
  assert(handle != retired_handle);
  message_queue_native_wake(&env, nullptr, retired_handle);
  assert(wakes == 0);
  assert(message_queue_native_is_polling(&env, nullptr, retired_handle) == JNI_FALSE);
  dispose_during_wake = true;
  message_queue_native_wake(&env, nullptr, handle);
  dispose_during_wake = false;
  assert(wakes == 1);
  assert(message_queue_looper(&env, queue) == nullptr);
  handle = message_queue_native_init(&env, nullptr);
  dispose_during_poll = true;
  message_queue_native_poll_once(&env, nullptr, handle, 0);
  dispose_during_poll = false;
  assert(message_queue_looper(&env, queue) == nullptr);
  assert(message_queue_native_is_polling(&env, nullptr, handle) == JNI_FALSE);
  message_queue_native_destroy(&env, nullptr, handle);
  handle = 0;
  assert(message_queue_looper(&env, queue) == nullptr);
  assert(message_queue_looper(&env, nullptr) == nullptr);
}
