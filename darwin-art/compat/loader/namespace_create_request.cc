#include "namespace_handles.h"
#include "namespace_operation.h"
#include "android_dlext_types.h"
#include <nativeloader/dlext_namespaces.h>

namespace darwin_art::loader {
android_namespace_t* NamespaceHandles::CreateForAddress(uintptr_t address, const char* name,
    const char* search, const char* defaults, uint64_t type, const char* permitted,
    android_namespace_t* parent, std::string* error) {
  if (error) error->clear();
  auto fail = [error](const char* text) -> android_namespace_t* {
    if (error) *error = text;
    return nullptr;
  };
  if (!name) return fail("namespace name is null");
  constexpr uint64_t supported = ANDROID_NAMESPACE_TYPE_ISOLATED |
      ANDROID_NAMESPACE_TYPE_SHARED | ANDROID_NAMESPACE_TYPE_ALSO_USED_AS_ANONYMOUS;
  if (type & ~supported) return fail("namespace type requires unsupported linker policy (including exempt-list)");
  NamespaceOperation operation(configured_->registry());
  if (!operation) return fail("cannot enter linker operation");
  LinkerImageLease* image = nullptr;
  if (!parent && FindElfCaller(address, &image, error) < 0) return nullptr;
  std::unique_ptr<LinkerImageLease, decltype(&darwin_art_linker_image_release)> caller(
      image, darwin_art_linker_image_release);
  auto* effective = ParentForCaller(parent, caller.get(), error);
  if (!effective) return nullptr;
  return CreateChild(effective, type & ANDROID_NAMESPACE_TYPE_ISOLATED,
      type & ANDROID_NAMESPACE_TYPE_SHARED, type & ANDROID_NAMESPACE_TYPE_ALSO_USED_AS_ANONYMOUS,
      search, defaults, permitted, error, name);
}
}
