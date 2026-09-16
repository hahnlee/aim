#pragma once
#include "darwin_art_linker_namespace.h"
#include "linker_image.h"
#include "graphics_ndk_image.h"
#include "vulkan_image.h"
#include <memory>
#include <string>

namespace darwin_art::loader {
// Convert one owned lookup result into one explicit ELF open. Mach-O/provider
// representations retain their own native resource contract; never fabricate
// an ELF group for them. The public guest libdl adapter is still separate.
inline int AcquireNamespaceOpen(LinkerImageLease** result, std::string* error, bool nodelete = false) {
  if (!result || !*result) return -1;
  std::unique_ptr<LinkerImageLease, decltype(&darwin_art_linker_image_release)> lookup(
      *result, darwin_art_linker_image_release);
  *result = nullptr;
  void* payload = nullptr;
  if (darwin_art_linker_image_typed_payload(lookup.get(), DARWIN_ART_IMAGE_MACHO, &payload) == 0 ||
      darwin_art_linker_image_typed_payload(lookup.get(), DARWIN_ART_IMAGE_BIONIC_PROVIDER, &payload) == 0 ||
      darwin_art_linker_image_typed_payload(lookup.get(), DARWIN_ART_IMAGE_ANGLE, &payload) == 0 ||
      darwin_art_linker_image_typed_payload(lookup.get(), DARWIN_ART_IMAGE_NATIVE_WINDOW, &payload) == 0 ||
      IsVulkanImage(lookup.get()) ||
      (IsGraphicsNdkImage(lookup.get()) || IsLinkerImage(lookup.get()))) {
    *result = lookup.release();
    return 0;
  }
  if (darwin_art_linker_image_acquire_open(lookup.get(), result) == 0) {
    if (!nodelete || darwin_art_linker_image_promote_nodelete(*result) == 0) return 0;
    darwin_art_linker_image_release(*result);
    *result = nullptr;
  }
  if (error) *error = "cannot acquire original ELF local-group open reference";
  return -1;
}
}
