#include "process_namespaces.h"
#include "namespace_handles.h"
#include "android_dlext_types.h"
#include "namespace_loader_abi.h"
#include <nativeloader/dlext_namespaces.h>

extern "C" android_namespace_t* __loader_android_create_namespace(const char* name,
    const char* search, const char* defaults, uint64_t type, const char* permitted,
    android_namespace_t* parent, const void* caller_address) {
  const auto caller = reinterpret_cast<uintptr_t>(caller_address);
  auto owner = darwin_art::loader::AcquireProcessNamespaces();
  if (!owner) {
    darwin_art_linker_set_error("Android process namespaces are not installed");
    return nullptr;
  }
  std::string error;
  auto* result = owner->CreateForAddress(caller, name, search, defaults, type, permitted, parent, &error);
  if (!result) darwin_art_linker_set_error(error.c_str());
  return result;
}

extern "C" android_namespace_t* android_create_namespace(const char* name,
    const char* search, const char* defaults, uint64_t type, const char* permitted,
    android_namespace_t* parent) {
  return __loader_android_create_namespace(name, search, defaults, type, permitted,
      parent, __builtin_return_address(0));
}

extern "C" android_namespace_t* android_get_exported_namespace(const char* name) {
  auto owner = darwin_art::loader::AcquireProcessNamespaces();
  if (!owner) {
    darwin_art_linker_set_error("Android process namespaces are not installed");
    return nullptr;
  }
  // Like AOSP get_exported_namespace, an unknown/null name does not set dlerror.
  return owner->Exported(name);
}

extern "C" bool android_link_namespaces(android_namespace_t* from,
    android_namespace_t* to, const char* names) {
  auto owner = darwin_art::loader::AcquireProcessNamespaces();
  if (!owner) {
    darwin_art_linker_set_error("Android process namespaces are not installed");
    return false;
  }
  std::string error;
  if (owner->Link(from, to, names, &error)) return true;
  darwin_art_linker_set_error(error.c_str());
  return false;
}

extern "C" android_namespace_t* __loader_android_get_exported_namespace(const char* name) {
  return android_get_exported_namespace(name);
}
extern "C" bool __loader_android_link_namespaces(android_namespace_t* from,
    android_namespace_t* to, const char* names) {
  return android_link_namespaces(from, to, names);
}
