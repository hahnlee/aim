#include "namespace_loader_abi.h"
#include "namespace_handles.h"
#include "process_namespaces.h"
#include <cstdlib>

extern "C" void __loader_android_set_application_target_sdk_version(int target) {
  auto owner = aim::loader::AcquireProcessNamespaces();
  // A live Android linker is required: there is no independent shadow SDK
  // state before installation or after process namespace teardown.
  if (!owner || !owner->SetTargetSdkVersion(target)) std::abort();
}

extern "C" int __loader_android_get_application_target_sdk_version() {
  auto owner = aim::loader::AcquireProcessNamespaces();
  if (!owner) std::abort();
  return owner->TargetSdkVersion();
}
