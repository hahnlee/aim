#include "namespace_handles.h"
#include "namespace_close.h"
#include "process_namespaces.h"
#include "namespace_loader_abi.h"
namespace aim::loader {
int NamespaceHandles::CloseLibrary(uintptr_t handle, std::string* error) {
  return CloseNamespaceLibrary(configured_->registry(), handle, error);
}
}
extern "C" int __loader_dlclose(void* handle) {
  auto owner = aim::loader::AcquireProcessNamespaces();
  if (!owner) {
    aim_linker_set_error("Android process namespaces are not installed");
    return -1;
  }
  std::string error;
  const int status = owner->CloseLibrary(reinterpret_cast<uintptr_t>(handle), &error);
  if (status != 0) aim_linker_set_error(error.c_str());
  return status;
}
extern "C" int aim_linker_dlclose(void* handle) {
  return __loader_dlclose(handle);
}
