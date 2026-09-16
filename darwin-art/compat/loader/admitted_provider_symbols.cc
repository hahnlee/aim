#include "admitted_provider_symbols.h"
#include "bionic_provider_image.h"
#include "namespace_elf_group.h"
#include "angle_image.h"
#include "native_window_image.h"
#include "vulkan_image.h"
#include "linker_image.h"
#include "graphics_ndk_image.h"
#include <cstring>
#include <algorithm>

namespace darwin_art::loader {
DarwinArtElfResolveStatus ResolveAdmittedProvider(void* context,
    const DarwinArtElfSymbolRequest* request, uintptr_t* address,
    DarwinArtElfErrorBuffer* error) noexcept {
  if (address) *address = 0;
  if (error) {
    error->required = 0;
    if (error->data && error->capacity) error->data[0] = '\0';
  }
  auto fail = [error](const char* detail) {
    if (error) {
      error->required = std::strlen(detail) + 1;
      if (error->data && error->capacity) {
        const auto n = std::min(error->capacity - 1, error->required - 1);
        std::memcpy(error->data, detail, n);
        error->data[n] = '\0';
      }
    }
    return DARWIN_ART_ELF_RESOLVE_ERROR;
  };
  try {
    if (!context || !address || !request ||
        request->abi_version != DARWIN_ART_ELF_ABI_VERSION ||
        request->needed_library_count != 1 || !request->needed_libraries ||
        !request->needed_libraries[0] || !*request->needed_libraries[0])
      return fail("missing single selected image in resolver request");
    const auto* name = request->needed_libraries[0];
    if ((request->version_soname == nullptr) != (request->version_name == nullptr) ||
        (request->version_soname && std::strcmp(name, request->version_soname) != 0))
      return fail("symbol version does not identify the selected image");
    std::string detail;
    const auto result = ResolveAdmittedProviderSymbol(
        static_cast<const DarwinArtElfDiscoveredGraph*>(context), name,
        request->symbol, request->version_name, address, &detail);
    if (result == 0) return DARWIN_ART_ELF_RESOLVE_FOUND;
    if (result == 1) return DARWIN_ART_ELF_RESOLVE_NOT_FOUND;
    return fail(detail.c_str());
  } catch (...) {
    return fail("exception during admitted provider lookup");
  }
}

int ResolveAdmittedProviderSymbol(const DarwinArtElfDiscoveredGraph* graph,
    const char* soname, const char* symbol, const char* version,
    uintptr_t* address, std::string* error) {
  if (error) error->clear();
  if (address) *address = 0;
  auto fail = [error](const char* text) {
    if (error) *error = text;
    return -1;
  };
  if (!graph || !soname || !*soname || !symbol || !*symbol || !address)
    return fail("invalid admitted symbol request");
  size_t count = 0;
  if (darwin_art_elf_discovered_graph_resident_count(graph, &count, nullptr) != DARWIN_ART_ELF_OK)
    return fail("cannot inspect admitted images");
  for (size_t i = 0; i < count; ++i) {
    const char* name = nullptr;
    uint64_t id = 0;
    void* lease = nullptr;
    if (darwin_art_elf_discovered_graph_resident(graph, i, &name, &id,
        &lease, nullptr) != DARWIN_ART_ELF_OK)
      return fail("cannot borrow admitted image");
    if (std::strcmp(name, soname) != 0) continue;
    auto* image = static_cast<LinkerImageLease*>(lease);
    if (IsLinkerImage(image)) return ResolveLinkerImage(image, symbol, version, address, error);
    if (IsGraphicsNdkImage(image)) return ResolveGraphicsNdkImage(image, symbol, version, address, error);
    if (IsAngleImage(image)) return ResolveAngleImage(image, symbol, version, address, error);
    if (IsNativeWindowImage(image)) return ResolveNativeWindowImage(image, symbol, version, address, error);
    if (IsVulkanImage(image)) return ResolveVulkanImage(image, symbol, version, address, error);
    void* selected = nullptr;
    if (darwin_art_linker_image_typed_payload(image, DARWIN_ART_IMAGE_ELF_SELECTED, &selected) == 0)
      return ResolveNamespaceElf(image, symbol, version, address, error);
    const auto result = ResolveBionicProviderImage(
        static_cast<LinkerImageLease*>(lease), symbol, version);
    switch (result.status) {
      case DARWIN_ART_BIONIC_NAMESPACE_OK:
        if (!result.address) return fail("resolved provider has null address");
        *address = result.address;
        return 0;
      case DARWIN_ART_BIONIC_NAMESPACE_UNSUPPORTED_SYMBOL:
      case DARWIN_ART_BIONIC_NAMESPACE_UNKNOWN_VERSION:
        return 1;
      default:
        return fail("admitted provider owner cannot resolve symbols");
    }
  }
  return 1;
}
}
