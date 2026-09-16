#include "namespace_handles.h"
#include "namespace_group_release.h"
#include "namespace_operation.h"
#include "namespace_elf_group.h"
#include "bionic_provider_image.h"
#include "process_namespaces.h"
#include "angle_image.h"
#include "native_window_image.h"
#include "vulkan_image.h"
#include "linker_image.h"
#include "graphics_ndk_image.h"
#include "namespace_symbol_lookup.h"
#include <dlfcn.h>
namespace darwin_art::loader {
uintptr_t NamespaceHandles::LibrarySymbol(uintptr_t handle, const char* symbol, std::string* error,
    const char* version, LinkerImageLease** defining_image) {
  if (defining_image) *defining_image = nullptr;
  if (error) error->clear();
  auto fail = [error](const char* reason) -> uintptr_t { if (error) *error = reason; return 0; };
  if (!symbol || !*symbol) return fail("missing Android symbol name");
  NamespaceOperation operation(configured_->registry());
  if (!operation) return fail("cannot enter linker symbol operation");
  LinkerImageLease* raw = nullptr;
  if (darwin_art_linker_handle_image(configured_->registry(), handle, &raw) != 0)
    return fail("invalid Android symbol handle");
  ImageLease image(raw);
  return LookupNamespaceSymbol(configured_->registry(), image.get(), symbol, error, version, defining_image);
}
int ResolveNamespaceImageSymbol(const LinkerImageLease* image, const char* symbol,
    uintptr_t* address, std::string* error, const char* version) {
  if (error) error->clear();
  *address = 0;
  if (IsLinkerImage(image)) return ResolveLinkerImage(image, symbol, version, address, error);
  if (IsGraphicsNdkImage(image)) return ResolveGraphicsNdkImage(image, symbol, version, address, error);
  if (IsAngleImage(image)) {
    return ResolveAngleImage(image, symbol, version, address, error);
  }
  if (IsNativeWindowImage(image)) {
    return ResolveNativeWindowImage(image, symbol, version, address, error);
  }
  if (IsVulkanImage(image)) {
    return ResolveVulkanImage(image, symbol, version, address, error);
  }
  void* payload = nullptr;
  if (darwin_art_linker_image_typed_payload(image, DARWIN_ART_IMAGE_MACHO, &payload) == 0) {
    // The published Mach-O representation owns a real dyld handle. Only that
    // payload, never the Android handle token or another image kind, goes to
    // host dlsym. The image lease keeps it live through this operation.
    if (!payload) { if (error) *error = "missing owned Mach-O handle"; return -1; }
    if (version) { if (error) *error = "Mach-O image has no Android symbol-version contract"; return -1; }
    dlerror();
    void* found = dlsym(payload, symbol);
    const char* detail = dlerror();
    if (detail) { if (error) *error = detail; return 1; }
    *address = reinterpret_cast<uintptr_t>(found);
    return 0;
  }
  if (darwin_art_linker_image_typed_payload(image, DARWIN_ART_IMAGE_ELF_SELECTED, &payload) == 0) {
    return ResolveNamespaceElf(image, symbol, version, address, error);
  }
  if (darwin_art_linker_image_typed_payload(image, DARWIN_ART_IMAGE_BIONIC_PROVIDER, &payload) == 0) {
    const auto result = ResolveBionicProviderImage(image, symbol, version);
    if (result.status == DARWIN_ART_BIONIC_NAMESPACE_OK) { *address = result.address; return 0; }
    if (error) *error = "Android provider symbol not found";
    return result.status == DARWIN_ART_BIONIC_NAMESPACE_UNSUPPORTED_SYMBOL ||
            result.status == DARWIN_ART_BIONIC_NAMESPACE_UNKNOWN_VERSION ? 1 : -1;
  }
  if (error) *error = "native image symbol contract is not connected";
  return -1;
}
}
