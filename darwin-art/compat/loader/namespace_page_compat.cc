#include "namespace_handles.h"
#include "namespace_operation.h"
#include "namespace_loader_abi.h"
#include "process_namespaces.h"
#include <android-base/parsebool.h>
#include <cstdlib>

extern "C" const void* darwin_art_bionic___system_property_find(const char*);
extern "C" void darwin_art_bionic___system_property_read_callback(
    const void*, void (*)(void*, const char*, const char*, uint32_t), void*);

namespace darwin_art::loader {
bool NamespaceHandles::Set16KbAppCompatMode(bool enabled) {
  NamespaceOperation operation(configured_->registry());
  if (!operation) return false;
  appcompat_16kb_.store(enabled);
  return true;
}

bool NamespaceHandles::Effective16KbAppCompatMode() const {
  // Read the live Android property for each new load; do not cache it as a
  // Darwin environment setting. AOSP combines this property with the API mode.
  bool property_enabled = false;
  const void* property = darwin_art_bionic___system_property_find(
      "bionic.linker.16kb.app_compat.enabled");
  if (property) {
    darwin_art_bionic___system_property_read_callback(property,
        [](void* result, const char*, const char* value, uint32_t) {
          *static_cast<bool*>(result) = android::base::ParseBool(value) ==
              android::base::ParseBoolResult::kTrue;
        }, &property_enabled);
  }
  return appcompat_16kb_.load() || property_enabled;
}
}

extern "C" void __loader_android_set_16kb_appcompat_mode(bool enabled) {
  auto owner = darwin_art::loader::AcquireProcessNamespaces();
  if (!owner || !owner->Set16KbAppCompatMode(enabled)) std::abort();
}
