#include "process_namespaces.h"
#include "namespace_handles.h"
#include "android_dlext_types.h"
#include "namespace_loader_abi.h"

namespace {
void* Open(uintptr_t caller, const char* path, int flags, const android_dlextinfo* info) {
  auto owner = darwin_art::loader::AcquireProcessNamespaces();
  if (!owner) {
    darwin_art_linker_set_error("Android process namespaces are not installed");
    return nullptr;
  }
  const uint64_t extensions = info ? info->flags : 0;
  auto* target = (extensions & ANDROID_DLEXT_USE_NAMESPACE) ? info->library_namespace : nullptr;
  std::string error;
  const auto handle = owner->OpenLibrary(caller, path, flags, extensions, target, &error);
  if (!handle) darwin_art_linker_set_error(error.c_str());
  return reinterpret_cast<void*>(handle);
}
}
extern "C" void* __loader_dlopen(const char* path, int flags, const void* caller) {
  return Open(reinterpret_cast<uintptr_t>(caller), path, flags, nullptr);
}
extern "C" void* __loader_android_dlopen_ext(const char* path, int flags,
    const android_dlextinfo* info, const void* caller) {
  return Open(reinterpret_cast<uintptr_t>(caller), path, flags, info);
}
extern "C" void* android_dlopen_ext(const char* path, int flags, const android_dlextinfo* info) {
  return __loader_android_dlopen_ext(path, flags, info, __builtin_return_address(0));
}
extern "C" void* darwin_art_linker_dlopen(const char* path, int flags) {
  return __loader_dlopen(path, flags, __builtin_return_address(0));
}
