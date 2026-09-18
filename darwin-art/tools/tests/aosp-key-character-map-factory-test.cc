#include <jni.h>

#include <atomic>
#include <cstddef>
#include <cstdlib>
#include <iostream>
#include <new>
#include <cstdint>

namespace {
std::atomic<int> g_allocations{0};
std::atomic<int> g_frees{0};
std::atomic<bool> g_fail_next_allocation{false};

void* Allocate(std::size_t size) {
  if (g_fail_next_allocation.exchange(false)) {
    throw std::bad_alloc();
  }
  void* result = std::malloc(size == 0 ? 1 : size);
  if (result == nullptr) {
    throw std::bad_alloc();
  }
  g_allocations.fetch_add(1);
  return result;
}

void Release(void* pointer) noexcept {
  if (pointer != nullptr) {
    g_frees.fetch_add(1);
    std::free(pointer);
  }
}
}  // namespace

void* operator new(std::size_t size) { return Allocate(size); }
void* operator new[](std::size_t size) { return Allocate(size); }
void operator delete(void* pointer) noexcept { Release(pointer); }
void operator delete[](void* pointer) noexcept { Release(pointer); }
void operator delete(void* pointer, std::size_t) noexcept { Release(pointer); }
void operator delete[](void* pointer, std::size_t) noexcept { Release(pointer); }

// The build owner applies 0009-darwin-key-character-map-factory-handoff.patch
// to this pinned original translation unit before compiling this test. A
// caller may point the include at that patched staging copy explicitly.
#ifndef DARWIN_ART_KCM_JNI_SOURCE
#define DARWIN_ART_KCM_JNI_SOURCE \
  "../../_aosp/android16-key-character-map/frameworks/base/core/jni/android_view_KeyCharacterMap.cpp"
#endif
#include DARWIN_ART_KCM_JNI_SOURCE

namespace {
enum class NewObjectMode { kReturnObject, kReturnNull };
NewObjectMode g_new_object_mode = NewObjectMode::kReturnObject;
void* g_last_native_map = nullptr;
bool g_pending_exception = false;
int g_new_object_calls = 0;

jobject NewObjectV(JNIEnv*, jclass, jmethodID, va_list args) {
  g_last_native_map = reinterpret_cast<void*>(va_arg(args, jlong));
  ++g_new_object_calls;
  if (g_new_object_mode == NewObjectMode::kReturnNull) {
    return nullptr;
  }
  return reinterpret_cast<jobject>(static_cast<uintptr_t>(0x1234));
}

jboolean ExceptionCheck(JNIEnv*) {
  return g_pending_exception ? JNI_TRUE : JNI_FALSE;
}

void Require(bool condition, const char* contract) {
  if (!condition) {
    std::cerr << "aosp-key-character-map-factory: FAIL " << contract << '\n';
    std::exit(1);
  }
}

JNIEnv MakeEnvironment(JNINativeInterface& table) {
  table = {};
  table.NewObjectV = &NewObjectV;
  table.ExceptionCheck = &ExceptionCheck;
  return JNIEnv{&table};
}

int LiveAllocations() {
  return g_allocations.load() - g_frees.load();
}
}  // namespace

int main() {
  JNINativeInterface table{};
  JNIEnv env = MakeEnvironment(table);
  android::gKeyCharacterMapClassInfo.clazz =
      reinterpret_cast<jclass>(static_cast<uintptr_t>(1));
  android::gKeyCharacterMapClassInfo.ctor =
      reinterpret_cast<jmethodID>(static_cast<uintptr_t>(2));

  const int baseline = LiveAllocations();
  g_new_object_mode = NewObjectMode::kReturnNull;
  g_pending_exception = false;
  jobject rejected = android::android_view_KeyCharacterMap_create(&env, 7, nullptr);
  Require(rejected == nullptr, "Java allocation null is returned");
  Require(!env.ExceptionCheck(), "null allocation has no pending exception");
  Require(g_new_object_calls == 1, "NewObjectV called once on null allocation");
  Require(g_last_native_map != nullptr, "native allocation reached Java boundary");
  Require(LiveAllocations() == baseline, "null Java allocation releases native owner");

  g_pending_exception = true;
  jobject rejected_with_exception =
      android::android_view_KeyCharacterMap_create(&env, 7, nullptr);
  Require(rejected_with_exception == nullptr, "Java exception allocation returns null");
  Require(env.ExceptionCheck(), "pending Java exception is preserved");
  Require(LiveAllocations() == baseline,
          "null allocation with pending exception releases native owner");
  g_pending_exception = false;

  g_new_object_mode = NewObjectMode::kReturnObject;
  jobject accepted = android::android_view_KeyCharacterMap_create(&env, 8, nullptr);
  Require(accepted != nullptr, "successful Java allocation returns object");
  Require(g_last_native_map != nullptr, "successful handoff carries native owner");
  Require(LiveAllocations() == baseline + 1, "successful handoff transfers ownership");
  delete static_cast<android::NativeKeyCharacterMap*>(g_last_native_map);
  g_last_native_map = nullptr;
  Require(LiveAllocations() == baseline, "transferred native owner remains deletable");

  g_fail_next_allocation = true;
  const int calls_before_bad_alloc = g_new_object_calls;
  bool caught_bad_alloc = false;
  try {
    (void)android::android_view_KeyCharacterMap_create(&env, 9, nullptr);
  } catch (const std::bad_alloc&) {
    caught_bad_alloc = true;
  }
  Require(caught_bad_alloc, "C++ allocation failure propagates as bad_alloc");
  Require(g_new_object_calls == calls_before_bad_alloc,
          "Java allocation is not attempted after C++ bad_alloc");
  Require(LiveAllocations() == baseline, "bad_alloc leaves no native owner");

  std::cout << "aosp-key-character-map-factory: PASS actual original JNI factory handoff\n";
}
