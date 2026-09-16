#include "namespace_handles.h"
#include "namespace_operation.h"

namespace darwin_art::loader {
namespace {
int32_t Contains(void* image, uintptr_t address, void* context) {
  int32_t found = 0;
  auto* error = static_cast<DarwinArtElfErrorBuffer*>(context);
  if (darwin_art_elf_selected_contains_address(static_cast<DarwinArtElfSelectedImage*>(image),
      address, &found, error) != DARWIN_ART_ELF_OK) return -1;
  return found;
}
}
int NamespaceHandles::FindElfCaller(uintptr_t address, LinkerImageLease** output, std::string* error) {
  if (error) error->clear();
  if (!output) return -1;
  *output = nullptr;
  NamespaceOperation operation(configured_->registry());
  if (!operation) { if (error) *error = "cannot enter linker operation"; return -1; }
  char detail[1024]{};
  DarwinArtElfErrorBuffer buffer{detail, sizeof(detail), 0};
  const int result = darwin_art_linker_find_image_address(configured_->registry(),
      DARWIN_ART_IMAGE_ELF_SELECTED, address, Contains, &buffer, output);
  if (result < 0 && error) *error = *detail ? detail : "registered ELF caller lookup failed";
  return result;
}
}
