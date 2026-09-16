#include <jni.h>

#include "darwin_runtime_adapters_internal.h"
#include "jni/android_varargs.h"

#include <atomic>
#include <cassert>
#include <cstdint>
#include <cstdio>
#include <cstring>
#include <string>

namespace {

JNIEnv *g_test_env = nullptr;
int g_find_class_calls = 0;
const char *g_last_name_pointer = nullptr;
std::string g_last_name;
bool g_pending_exception = false;
int g_method_calls = 0;
const jobject kArgument = reinterpret_cast<jobject>(uintptr_t{0x3210});

jobject NewObject(JNIEnv*, jclass, jmethodID, const jvalue* values) {
  ++g_method_calls;
  assert(values && values[0].l == kArgument);
  return kArgument;
}

jobject CallObject(JNIEnv*, jobject, jmethodID, const jvalue* values) {
  ++g_method_calls;
  assert(values == nullptr);
  return kArgument;
}
constexpr const char *kExpectedName = "android/view/View";
constexpr const char *kMissingName = "android/missing/Thing";
constexpr uintptr_t kExpectedClassValue = 0x1234;

jclass FindClass(JNIEnv *, const char *name) {
  assert(name != nullptr);
  ++g_find_class_calls;
  g_last_name_pointer = name;
  g_last_name = name;
  if (std::strcmp(name, kExpectedName) == 0) {
    return reinterpret_cast<jclass>(kExpectedClassValue);
  }
  g_pending_exception = true;
  return nullptr;
}

} // namespace

namespace android {

std::atomic<int> g_elf_fixture_status{0};

JNIEnv *CurrentArtEnv() { return g_test_env; }

} // namespace android

int main() {
  JNINativeInterface functions{};
  // Lookup, direct object invocation and construction only. Reflection, string
  // rewriting and ExceptionClear remain unavailable to this production caller.
  functions.FindClass = FindClass;
  functions.CallObjectMethodA = CallObject;
  functions.NewObjectA = NewObject;
  JNIEnv test_env{&functions};
  g_test_env = &test_env;

  android::ElfLibrary library{};
  library.fixture_graph = false;
  library.app_loader = reinterpret_cast<void *>(uintptr_t{0xfeedface});

  const char requested_name[] = "android/view/View";
  void *found = android::ProxyFindClass(&library, requested_name);
  assert(reinterpret_cast<uintptr_t>(found) == kExpectedClassValue);
  assert(g_find_class_calls == 1);
  assert(g_last_name_pointer == requested_name);
  assert(g_last_name == kExpectedName);
  assert(!g_pending_exception);
  assert(android::g_elf_fixture_status.load(std::memory_order_relaxed) == 0);

  const char missing_name[] = "android/missing/Thing";
  assert(android::ProxyFindClass(&library, missing_name) == nullptr);
  assert(g_find_class_calls == 2);
  assert(g_last_name_pointer == missing_name);
  assert(g_last_name == kMissingName);
  // FindClass owns the pending-exception result. ProxyFindClass must neither
  // clear it nor translate the failure through another JNI operation.
  assert(g_pending_exception);
  assert(android::g_elf_fixture_status.load(std::memory_order_relaxed) == 0);

  g_test_env = nullptr;
  assert(android::ProxyFindClass(&library, requested_name) == nullptr);
  assert(g_find_class_calls == 2);

  // Execute the actual ProxyCallMethodV path as well. JNI references supplied
  // as arguments must reach ART unchanged, including constructor parameters.
  g_test_env = &test_env;
  g_pending_exception = false;
  jmethodID method = reinterpret_cast<jmethodID>(uintptr_t{0x7650});
  library.method_descriptors[method] = "(Ljava/lang/String;)V";
  uint64_t gp = reinterpret_cast<uintptr_t>(kArgument);
  darwin_art::jni::AndroidArm64VaList args{
      nullptr, reinterpret_cast<uint8_t*>(&gp) + sizeof(gp), nullptr, -8, 0};
  assert(android::ProxyCallMethodV(&library, kArgument, method, &args, 'L', 2) ==
         reinterpret_cast<uintptr_t>(kArgument));
  assert(g_method_calls == 1 && args.gr_offs == -8);
  library.method_descriptors[method] = "()Ljava/lang/String;";
  assert(android::ProxyCallMethodV(&library, kArgument, method, &args, 'L', 0) ==
         reinterpret_cast<uintptr_t>(kArgument));
  assert(g_method_calls == 2);

  g_test_env = &test_env;
  assert(android::ProxyFindClass(&library, nullptr) == nullptr);
  assert(g_find_class_calls == 2);

  std::puts("android-jni-class-lookup: PASS direct FindClass slash-name "
            "pending-preserved null-guards loader-unconsulted methodV-passthrough");
  return 0;
}
