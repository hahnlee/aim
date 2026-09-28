#include "aim_native_bridge_loader.h"

#include <cassert>
#include <cstdio>
#include <cstring>
#include <string>
#include <vector>

#include "native_bridge_state_machine.cc"

using aim::integration::Format;
using aim::integration::OnLoadResult;
using aim::integration::StateMachine;

struct Fake {
  uint64_t next_namespace = 1;
  uintptr_t next_handle = 0x8000;
  int opens = 0;
  int closes = 0;
  int lifecycle = 0;
  int regular = 0;
  int critical = 0;
  std::vector<std::string> requests;
};

static AimLoaderNamespace CreateNamespace(void* context, AimLoaderNamespace,
                                                 const char*, const char*, char*, size_t) {
  return static_cast<Fake*>(context)->next_namespace++;
}

static AimLoaderHandle Open(void* context, AimLoaderNamespace name_space,
                                  const char* path, int flags, char* error, size_t error_capacity) {
  Fake* fake = static_cast<Fake*>(context);
  assert(name_space != 0 && path != nullptr && flags == 2);
  ++fake->opens;
  if (std::strcmp(path, "libopenfail.so") == 0) {
    std::snprintf(error, error_capacity, "fixture open failure");
    return nullptr;
  }
  return reinterpret_cast<void*>(fake->next_handle++);
}

static int Close(void* context, AimLoaderHandle handle, char*, size_t) {
  assert(handle != nullptr);
  ++static_cast<Fake*>(context)->closes;
  return 0;
}

static void* GetTrampoline(void* context, AimLoaderHandle handle, const char* symbol,
                           const char* shorty, uint32_t length,
                           AimJniCallType call_type, char*, size_t) {
  Fake* fake = static_cast<Fake*>(context);
  assert(handle != nullptr && symbol != nullptr);
  fake->requests.emplace_back(symbol);
  if (std::strcmp(symbol, "missingShort") == 0) return nullptr;
  if (std::strcmp(symbol, "JNI_OnLoad") == 0 || std::strcmp(symbol, "JNI_OnUnload") == 0) {
    assert(shorty == nullptr && length == 0 && call_type == AIM_JNI_CALL_REGULAR);
    ++fake->lifecycle;
    if (std::strcmp(symbol, "JNI_OnLoad") == 0 &&
        reinterpret_cast<uintptr_t>(handle) == 0x8001) {
      return nullptr;
    }
  } else {
    assert(shorty != nullptr && length == std::strlen(shorty));
    if (call_type == AIM_JNI_CALL_CRITICAL) {
      ++fake->critical;
    } else {
      ++fake->regular;
    }
  }
  return reinterpret_cast<void*>(0xfeed0000ULL + fake->requests.size());
}

static int Owns(void*, const void* address) { return address != nullptr; }

int main() {
  static_assert(AIM_JNI_CALL_REGULAR == 1);
  static_assert(AIM_JNI_CALL_CRITICAL == 2);
  static_assert(offsetof(AimElfLoaderV1, create_namespace) == 16);

  Fake fake;
  AimElfLoaderV1 loader{
      AIM_ELF_LOADER_ABI_V1, sizeof(AimElfLoaderV1), &fake,
      CreateNamespace, Open, Close, GetTrampoline, Owns};
  StateMachine machine(&loader);
  std::string error;

  assert(machine.Load("libgood.so", 11, Format::kAndroidElf, OnLoadResult::kOkay, &error));
  assert(machine.Load("libgood.so", 11, Format::kAndroidElf, OnLoadResult::kOkay, &error));
  assert(!machine.Load("libgood.so", 12, Format::kAndroidElf, OnLoadResult::kOkay, &error));
  assert(machine.Load("libnoonload.so", 11, Format::kAndroidElf, OnLoadResult::kAbsent, &error));
  assert(!machine.Load("libbad.so", 13, Format::kAndroidElf, OnLoadResult::kJniErr, &error));
  assert(!machine.Load("libbad.so", 13, Format::kAndroidElf, OnLoadResult::kOkay, &error));
  assert(!machine.Load("libopenfail.so", 14, Format::kAndroidElf,
                       OnLoadResult::kAbsent, &error));
  assert(error == "fixture open failure");
  assert(machine.Load("libhost.dylib", 11, Format::kMachO, OnLoadResult::kAbsent, &error));
  assert(machine.library_count() == 4 && fake.opens == 4 && machine.dyld_open_count() == 1);

  void* regular = machine.Find(11, "missingShort", "Java_pkg_Class_method", "IIL",
                               AIM_JNI_CALL_REGULAR);
  void* critical = machine.Find(11, "Java_pkg_Class_critical", "unused", "JJD",
                                AIM_JNI_CALL_CRITICAL);
  assert(regular != nullptr && critical != nullptr && fake.regular == 1 && fake.critical == 1);
  assert(machine.Find(99, "anything", "anything", "V", AIM_JNI_CALL_REGULAR) == nullptr);

  machine.Shutdown();
  assert(fake.closes == 3 && machine.dyld_close_count() == 1);
  // 3 OnLoad + 3 OnUnload lookups. Failed OnLoad remains owned until shutdown.
  assert(fake.lifecycle == 6);
  std::puts("native-bridge-state: PASS handle-tag=atomic classloader-namespace=isolated");
  std::puts("native-bridge-state: PASS OnLoad-failure=resident OnUnload-before-close=1");
  std::puts("native-bridge-state: PASS shorty=len-normalized regular=1 critical=2");
  return 0;
}
