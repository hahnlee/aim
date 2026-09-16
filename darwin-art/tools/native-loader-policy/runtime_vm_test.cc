#include "jni/runtime_vm.h"

#include "darwin_art_elf_loader.h"
#include "loader/android_dlext_types.h"
#include "loader/namespace_handles.h"
#include "loader/namespace_group_release.h"

#include <jni.h>

#include <array>
#include <cassert>
#include <cstdint>
#include <cstdio>
#include <memory>
#include <string>

namespace {

constexpr uintptr_t kClassValue = 0x1111222233334444ull;
constexpr uintptr_t kFunctionOne = 0x5555666677778888ull;
constexpr uintptr_t kFunctionTwo = 0x9999aaaabbbbccccull;
constexpr jint kRegistrationStatus = 23;

struct MockVmState {
  JavaVM *vm = nullptr;
  JNIEnv *env = nullptr;
  bool attached = true;
  int get_env_calls = 0;
};

MockVmState *g_vm_state = nullptr;
int g_register_calls = 0;
JNIEnv *g_register_env = nullptr;
jclass g_register_class = nullptr;
std::array<JNINativeMethod, 2> g_register_copy{};
jint g_register_count = -1;
jint g_register_status = JNI_OK;

jclass ClassValue() { return reinterpret_cast<jclass>(kClassValue); }
void *FunctionOne() { return reinterpret_cast<void *>(kFunctionOne); }
void *FunctionTwo() { return reinterpret_cast<void *>(kFunctionTwo); }

jint GetEnv(JavaVM *vm, void **output, jint version) {
  assert(g_vm_state != nullptr);
  assert(vm == g_vm_state->vm);
  ++g_vm_state->get_env_calls;
  assert(output != nullptr);
  *output = nullptr;
  if (version != JNI_VERSION_1_6)
    return JNI_EVERSION;
  if (!g_vm_state->attached)
    return JNI_EDETACHED;
  *output = g_vm_state->env;
  return JNI_OK;
}

jint RegisterNatives(JNIEnv *env, jclass clazz, const JNINativeMethod *methods,
                     jint count) {
  ++g_register_calls;
  g_register_env = env;
  g_register_class = clazz;
  g_register_count = count;
  assert(count == 2);
  assert(methods != nullptr);
  for (jint index = 0; index < count; ++index)
    g_register_copy[static_cast<std::size_t>(index)] = methods[index];
  return g_register_status;
}

// Generated trampoline entries intentionally lack compiler indirect-call
// metadata. Keep the sanitizer exemption limited to this dynamic invocation.
__attribute__((no_sanitize("function"))) int InvokeSyncWait(int (*entry)(int,
                                                                         int)) {
  return entry(-1, 0);
}

void ResetRegistration() {
  g_register_calls = 0;
  g_register_env = nullptr;
  g_register_class = nullptr;
  g_register_copy = {};
  g_register_count = -1;
  g_register_status = JNI_OK;
}

} // namespace

void TestRuntimeVm(
    std::shared_ptr<darwin_art::loader::NamespaceHandles> namespaces) {
  using darwin_art::jni::RuntimeVm;

  assert(namespaces);
  std::string error;

  JNINativeInterface env_functions{};
  env_functions.RegisterNatives = RegisterNatives;
  JNIEnv art_env{&env_functions};

  MockVmState vm_state;
  vm_state.env = &art_env;
  g_vm_state = &vm_state;
  JNIInvokeInterface vm_functions{};
  vm_functions.GetEnv = GetEnv;
  JavaVM art_vm{&vm_functions};
  vm_state.vm = &art_vm;

  assert(!RuntimeVm::Create(nullptr, namespaces, &error));
  assert(!RuntimeVm::Create(&art_vm, nullptr, &error));
  auto runtime = RuntimeVm::Create(&art_vm, namespaces, &error);
  assert(runtime && error.empty());
  assert(runtime->art_vm() == &art_vm);
  auto shared_proxy = runtime->proxy();
  std::weak_ptr<darwin_art::jni::ProxyVm> observed_proxy = shared_proxy;

  JavaVM *proxy_vm = static_cast<JavaVM *>(runtime->proxy()->JavaVm());
  assert(proxy_vm != nullptr);
  void *proxy_env_raw = nullptr;
  assert(proxy_vm->GetEnv(&proxy_env_raw, JNI_VERSION_1_6) == JNI_OK);
  assert(proxy_env_raw != nullptr);
  assert(vm_state.get_env_calls == 1);

  vm_state.attached = false;
  proxy_env_raw = reinterpret_cast<void *>(uintptr_t{1});
  const int calls_before_detached = vm_state.get_env_calls;
  assert(proxy_vm->GetEnv(&proxy_env_raw, JNI_VERSION_1_6) == JNI_EDETACHED);
  assert(proxy_env_raw == nullptr);
  assert(vm_state.get_env_calls == calls_before_detached + 1);
  // GetEnv observes detach only; it does not auto-attach the ART thread.
  vm_state.attached = true;
  assert(proxy_vm->GetEnv(&proxy_env_raw, JNI_VERSION_1_6) == JNI_OK);
  auto *proxy_env = static_cast<JNIEnv *>(proxy_env_raw);

  const char name_one[] = "first";
  const char signature_one[] = "(I)V";
  const char name_two[] = "second";
  const char signature_two[] = "(Ljava/lang/Object;)I";
  const JNINativeMethod methods[] = {
      {const_cast<char*>(name_one), const_cast<char*>(signature_one), FunctionOne()},
      {const_cast<char*>(name_two), const_cast<char*>(signature_two), FunctionTwo()},
  };
  ResetRegistration();
  g_register_status = kRegistrationStatus;
  assert(proxy_env->RegisterNatives(ClassValue(), methods, 2) ==
         kRegistrationStatus);
  assert(g_register_calls == 1);
  assert(g_register_env == &art_env);
  assert(g_register_class == ClassValue());
  assert(g_register_count == 2);
  assert(g_register_copy[0].name == name_one);
  assert(g_register_copy[0].signature == signature_one);
  assert(g_register_copy[0].fnPtr == FunctionOne());
  assert(g_register_copy[1].name == name_two);
  assert(g_register_copy[1].signature == signature_two);
  assert(g_register_copy[1].fnPtr == FunctionTwo());

  // Resolve a real ELF symbol through the VM-scoped registry. No Java method
  // lookup is involved: this is only the critical native callable path.
  const uintptr_t handle =
      namespaces->OpenLibrary(0, "libsync.so", 2, 0, nullptr, &error);
  assert(handle && error.empty());
  LinkerImageLease *defining = nullptr;
  const uintptr_t raw_sync = namespaces->LibrarySymbol(
      handle, "sync_wait", &error, nullptr, &defining);
  assert(raw_sync != 0 && defining != nullptr);
  darwin_art::loader::ImageLease defining_owner(defining);

  void *payload = nullptr;
  assert(darwin_art_linker_image_typed_payload(
             defining, DARWIN_ART_IMAGE_ELF_SELECTED, &payload) == 0 &&
         payload != nullptr);
  uint64_t mapping_group = 0;
  uint8_t group_root = 0;
  char detail[512]{};
  DarwinArtElfErrorBuffer buffer{detail, sizeof(detail), 0};
  assert(darwin_art_elf_selected_group_info(
             static_cast<DarwinArtElfSelectedImage *>(payload), &mapping_group,
             &group_root, &buffer) == DARWIN_ART_ELF_OK);
  assert(mapping_group != 0);

  const void *entry = nullptr;
  assert(runtime->Resolve(reinterpret_cast<void *>(raw_sync), false, "III", 3,
                          DARWIN_ART_JNI_CALL_CRITICAL_NATIVE,
                          &entry) == DARWIN_ART_REGISTERED_NATIVE_TRAMPOLINE);
  assert(entry != nullptr);
  assert(InvokeSyncWait(reinterpret_cast<int (*)(int, int)>(
             const_cast<void *>(entry))) == -1);

  // Retire the mapping group before releasing the defining lease or closing
  // the logical library open. The registry must reject stale resolution.
  runtime->RetireGroup(mapping_group);
  const void *retired_entry = nullptr;
  assert(runtime->Resolve(reinterpret_cast<void *>(raw_sync), false, "III", 3,
                          DARWIN_ART_JNI_CALL_CRITICAL_NATIVE,
                          &retired_entry) ==
         DARWIN_ART_REGISTERED_NATIVE_ERROR);
  assert(retired_entry == nullptr);
  defining_owner.reset();
  assert(namespaces->CloseLibrary(handle, &error) == 0);
  assert(error.empty());

  runtime.reset();
  assert(!observed_proxy.expired());
  assert(shared_proxy->JavaVm() == proxy_vm);
  proxy_env_raw = nullptr;
  assert(proxy_vm->GetEnv(&proxy_env_raw, JNI_VERSION_1_6) == JNI_OK);
  assert(proxy_env_raw != nullptr);
  shared_proxy.reset();
  assert(observed_proxy.expired());
  g_vm_state = nullptr;
  std::puts("runtime VM: null guards, shared proxy/GetEnv detach, direct "
            "registration and critical resolve-retire PASS");
}
