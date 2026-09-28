#include "jni/proxy_vm.h"
#include "jni/typed_library.h"
#include "jni/registered_methods.h"
#include <jni.h>

#include "loader/android_dlext_types.h"
#include "loader/namespace_handles.h"

#include <cassert>
#include <cstdint>
#include <cstdio>
#include <memory>
#include <string>

namespace {

struct BackendContext {
  int *release_count;
};

void *NoEnvironment(void *) { return nullptr; }

int32_t FailAttach(void *, void *, int32_t) { return AIM_JNI_ERR; }

int32_t FailDetach(void *) { return AIM_JNI_ERR; }

void *FailFindClass(void *, const char *) { return nullptr; }

int32_t FailRegisterNatives(void *, void *, const AimJniNativeMethod *,
                            int32_t) {
  return AIM_JNI_ERR;
}

int32_t FailThrowNew(void *, void *, const char *) {
  return AIM_JNI_ERR;
}

void *FailGetMethodId(void *, void *, const char *, const char *, int32_t) {
  return nullptr;
}

uint64_t FailCallMethodV(void *, void *, void *, void *, int32_t, int32_t) {
  return 0;
}

std::shared_ptr<void> MakeBackendContext(int *release_count) {
  return std::shared_ptr<void>(
      new BackendContext{release_count}, [](void *raw) {
        auto *context = static_cast<BackendContext *>(raw);
        ++*context->release_count;
        delete context;
      });
}

AimJniBackend MakeBackend(void *context) {
  return AimJniBackend{
      context,       &NoEnvironment,   &FailAttach,
      &FailDetach,   &FailFindClass,   &FailRegisterNatives,
      &FailThrowNew, &FailGetMethodId, &FailCallMethodV,
  };
}

using SyncWaitEntry = int (*)(int, int);

// Generated entries intentionally do not carry the compiler's indirect-call
// metadata. Keep the sanitizer exemption limited to this dynamic call.
__attribute__((no_sanitize("function"))) int
InvokeSyncWait(SyncWaitEntry entry) {
  return entry(-1, 0);
}

} // namespace

void TestTypedJniLibrary(
    std::shared_ptr<aim::loader::NamespaceHandles> owner) {
  using aim::android_jni::IsTrampolineEntry;
  using aim::jni::CallKind;
  using aim::jni::ProxyVm;
  using aim::jni::TypedLibrary;

  assert(owner);
  std::string error;
  int context_releases = 0;
  auto backend_context = MakeBackendContext(&context_releases);
  AimJniBackend backend = MakeBackend(backend_context.get());

  // A context mismatch is rejected without constructing a proxy owner.
  AimJniBackend invalid_backend = backend;
  invalid_backend.context = nullptr;
  assert(!ProxyVm::Create(backend_context, invalid_backend, &error));
  assert(!error.empty());
  auto proxy = ProxyVm::Create(backend_context, backend, &error);
  assert(proxy && error.empty());
  JavaVM *cached_vm = static_cast<JavaVM *>(proxy->JavaVm());
  assert(cached_vm != nullptr);
  void *cached_env = reinterpret_cast<void *>(uintptr_t{1});
  assert(cached_vm->GetEnv(&cached_env, JNI_VERSION_1_6) == JNI_EDETACHED);
  assert(cached_env == nullptr);
  backend_context.reset();
  assert(context_releases == 0);
  auto vm_root = proxy;
  proxy.reset();

  // A null VM is rejected without consuming its logical open.
  const uintptr_t invalid_vm_handle =
      owner->OpenLibrary(0, "libsync.so", 2, 0, nullptr, &error);
  assert(invalid_vm_handle && error.empty());
  assert(!TypedLibrary::Adopt(owner, invalid_vm_handle, nullptr, &error));
  assert(!error.empty());
  LinkerImageLease *retained_image = nullptr;
  assert(owner->LibraryImage(invalid_vm_handle, &retained_image) == 0 &&
         retained_image);
  aim_linker_image_release(retained_image);
  assert(owner->CloseLibrary(invalid_vm_handle, &error) == 0);

  const uintptr_t first_handle =
      owner->OpenLibrary(0, "libsync.so", 2, 0, nullptr, &error);
  const uintptr_t second_handle =
      owner->OpenLibrary(0, "libsync.so", 2, 0, nullptr, &error);
  assert(first_handle && second_handle && first_handle == second_handle);
  auto first_library =
      TypedLibrary::Adopt(owner, first_handle, vm_root, &error);
  assert(first_library && error.empty());
  auto second_library =
      TypedLibrary::Adopt(owner, second_handle, vm_root, &error);
  assert(second_library && error.empty());

  auto registered = aim::jni::RegisteredMethods::Create(owner, vm_root);
  assert(registered);
  LinkerImageLease* defining = nullptr;
  uintptr_t raw_sync = owner->LibrarySymbol(first_handle, "sync_wait", &error,
                                            nullptr, &defining);
  aim::loader::ImageLease defining_owner(defining);
  assert(raw_sync && defining);
  void* payload = nullptr;
  assert(aim_linker_image_typed_payload(defining, AIM_IMAGE_ELF_SELECTED,
                                               &payload) == 0 && payload);
  uint64_t mapping_group = 0;
  uint8_t group_root = 0;
  char detail[512]{};
  AimElfErrorBuffer buffer{detail, sizeof(detail), 0};
  assert(aim_elf_selected_group_info(
      static_cast<AimElfSelectedImage*>(payload), &mapping_group, &group_root,
      &buffer) == AIM_ELF_OK);
  const void* registered_entry = nullptr;
  assert(registered->Resolve(reinterpret_cast<void*>(raw_sync), false, "III", 3,
      AIM_JNI_CALL_CRITICAL_NATIVE, &registered_entry) ==
      AIM_REGISTERED_NATIVE_TRAMPOLINE);
  assert(registered_entry && IsTrampolineEntry(registered_entry));
  assert(InvokeSyncWait(reinterpret_cast<SyncWaitEntry>(
      const_cast<void*>(registered_entry))) == -1);
  const void* repeated_entry = nullptr;
  assert(registered->Resolve(registered_entry, true, "III", 3,
      AIM_JNI_CALL_CRITICAL_NATIVE, &repeated_entry) ==
      AIM_REGISTERED_NATIVE_TRAMPOLINE);
  assert(repeated_entry == registered_entry);
  assert(registered->Resolve(reinterpret_cast<void*>(raw_sync), false, "ILI", 3,
      AIM_JNI_CALL_CRITICAL_NATIVE, &repeated_entry) ==
      AIM_REGISTERED_NATIVE_ERROR && !repeated_entry);
  // Regular JNI requires an attached environment; this backend is detached.
  assert(registered->Resolve(reinterpret_cast<void*>(raw_sync), false, "III", 3,
      AIM_JNI_CALL_REGULAR, &repeated_entry) ==
      AIM_REGISTERED_NATIVE_ERROR && !repeated_entry);

  // CriticalNative does not need an attached proxy environment. Both
  // independent logical opens adapt and execute the real typed ELF symbol.
  void *first =
      first_library->Resolve("sync_wait", "III", CallKind::Critical, &error);
  void *second =
      second_library->Resolve("sync_wait", "III", CallKind::Critical, &error);
  assert(first && second && IsTrampolineEntry(first) &&
         IsTrampolineEntry(second));
  assert(InvokeSyncWait(reinterpret_cast<SyncWaitEntry>(first)) == -1);
  assert(InvokeSyncWait(reinterpret_cast<SyncWaitEntry>(second)) == -1);

  // Typed misses and unsupported CriticalNative reference shapes fail closed.
  assert(!first_library->Resolve("typed_jni_symbol_absent", "III",
                                 CallKind::Critical, &error));
  assert(!error.empty());
  assert(
      !first_library->Resolve("sync_wait", "LII", CallKind::Critical, &error));
  assert(!error.empty());
  assert(
      !first_library->Resolve("sync_wait", "IIL", CallKind::Critical, &error));
  assert(!error.empty());
  assert(!first_library->Resolve("sync_wait", "III", static_cast<CallKind>(99),
                                 &error));
  assert(!error.empty());

  // Closing one image must not invalidate the other image's callable or the
  // VM pointer cached by ART. The VM root is still live after both closes.
  assert(first_library->CloseAfterQuiescence(&error));
  assert(error.empty());
  assert(first_library->CloseAfterQuiescence(&error));
  assert(error.empty());
  assert(context_releases == 0);
  cached_env = reinterpret_cast<void *>(uintptr_t{1});
  assert(cached_vm->GetEnv(&cached_env, JNI_VERSION_1_6) == JNI_EDETACHED);
  assert(cached_env == nullptr);
  assert(InvokeSyncWait(reinterpret_cast<SyncWaitEntry>(second)) == -1);
  assert(second_library->Resolve("sync_wait", "III", CallKind::Critical,
                                 &error) == second);
  // Quiesce all registered entries before the final image-group close. The
  // registry must reject this generation even while another lease pins it.
  registered->RetireGroup(mapping_group);
  assert(registered->Resolve(reinterpret_cast<void*>(raw_sync), false, "III", 3,
      AIM_JNI_CALL_CRITICAL_NATIVE, &repeated_entry) ==
      AIM_REGISTERED_NATIVE_ERROR && !repeated_entry);
  registered.reset();
  defining_owner.reset();
  std::puts("registered JNI methods: real ELF execution, attached-env rejection, "
            "adapted-entry identity and group retirement PASS");
  assert(second_library->CloseAfterQuiescence(&error));
  assert(error.empty());
  assert(context_releases == 0);
  cached_env = reinterpret_cast<void *>(uintptr_t{1});
  assert(cached_vm->GetEnv(&cached_env, JNI_VERSION_1_6) == JNI_EDETACHED);
  assert(cached_env == nullptr);
  vm_root.reset();
  assert(context_releases == 1);
  assert(
      !first_library->Resolve("sync_wait", "III", CallKind::Critical, &error));
  assert(!error.empty());
  first_library.reset();
  second_library.reset();

  // Both TypedLibraries consumed the counted opens behind the shared numeric
  // handle; it is invalid only after the second close.
  assert(!owner->LibrarySymbol(first_handle, "sync_wait", &error));
  assert(!error.empty());
  assert(owner->CloseLibrary(first_handle, &error) != 0);
  assert(context_releases == 1);

  // A declared non-callable provider image is rejected at adoption and its
  // independent logical open remains explicitly closable.
  const uintptr_t invalid_kind =
      owner->OpenLibrary(0, "libEGL.so", 2, 0, nullptr, &error);
  assert(invalid_kind && error.empty());
  int invalid_context_releases = 0;
  auto invalid_context = MakeBackendContext(&invalid_context_releases);
  AimJniBackend invalid_kind_backend = MakeBackend(invalid_context.get());
  auto invalid_kind_vm =
      ProxyVm::Create(invalid_context, invalid_kind_backend, &error);
  assert(invalid_kind_vm);
  assert(!TypedLibrary::Adopt(owner, invalid_kind, invalid_kind_vm, &error));
  assert(!error.empty());
  assert(owner->CloseLibrary(invalid_kind, &error) == 0);
  invalid_kind_vm.reset();
  invalid_context.reset();
  assert(invalid_context_releases == 1);

  std::puts("typed JNI library: ELF CriticalNative execution, shape/miss "
            "rejection, owner lifetime PASS");
}
