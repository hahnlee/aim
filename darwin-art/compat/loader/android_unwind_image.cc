#include "android_unwind_image.h"
#include "bionic_symbol_lookup.h"
#include <cstring>

namespace darwin_art::loader {
struct AndroidUnwindImage {
  std::shared_ptr<DarwinArtBionicNamespace> providers;
  DarwinArtElfHandle* elf = nullptr;
  ~AndroidUnwindImage() { darwin_art_elf_unload(&elf, nullptr); }
};
namespace {
DarwinArtElfResolveStatus Resolve(void* context, const DarwinArtElfSymbolRequest* request,
    uintptr_t* address, DarwinArtElfErrorBuffer*) {
  if (address) *address = 0;
  if (!context || !request || !address || request->abi_version != DARWIN_ART_ELF_ABI_VERSION ||
      !request->symbol || (request->needed_library_count && !request->needed_libraries))
    return DARWIN_ART_ELF_RESOLVE_ERROR;
  auto* providers = static_cast<DarwinArtBionicNamespace*>(context);
  DarwinArtBionicNamespaceResult result{};
  if (request->version_soname || request->version_name) {
    if (!request->version_soname || !request->version_name) return DARWIN_ART_ELF_RESOLVE_ERROR;
    bool admitted = false;
    for (size_t i = 0; i < request->needed_library_count; ++i)
      if (request->needed_libraries[i] && std::strcmp(request->needed_libraries[i], request->version_soname) == 0)
        admitted = true;
    if (!admitted) return DARWIN_ART_ELF_RESOLVE_ERROR;
    result = darwin_art_bionic_namespace_resolve(providers, request->version_soname,
        request->symbol, request->version_name);
  } else {
    result = LookupBionicDependencies(providers, request->needed_libraries,
        request->needed_library_count, request->symbol);
  }
  if (result.status == DARWIN_ART_BIONIC_NAMESPACE_OK && result.address) {
    *address = result.address;
    return DARWIN_ART_ELF_RESOLVE_FOUND;
  }
  return IsBionicSymbolMiss(result.status) ? DARWIN_ART_ELF_RESOLVE_NOT_FOUND : DARWIN_ART_ELF_RESOLVE_ERROR;
}
}
SharedAndroidUnwind LoadAndroidUnwindImage(const char* path,
    std::shared_ptr<DarwinArtBionicNamespace> providers, std::string* error) {
  if (error) error->clear();
  if (!path || *path != '/' || !providers) {
    if (error) *error = "missing installed unwind image/dependency owner";
    return nullptr;
  }
  auto image = std::make_shared<AndroidUnwindImage>();
  image->providers = std::move(providers);
  DarwinArtElfLoadOptions options{DARWIN_ART_ELF_ABI_VERSION, Resolve, image->providers.get()};
  char text[2048]{};
  DarwinArtElfErrorBuffer detail{text, sizeof(text), 0};
  auto status = darwin_art_elf_load_path(path, &options, &image->elf, &detail);
  if (status == DARWIN_ART_ELF_OK) status = darwin_art_elf_run_initializers(image->elf, &detail);
  if (status != DARWIN_ART_ELF_OK) {
    if (error) *error = *text ? text : darwin_art_elf_status_name(status);
    return nullptr;
  }
  return image;
}
DarwinArtElfStatus LookupAndroidUnwind(const SharedAndroidUnwind& image,
    const char* symbol, uintptr_t* address) {
  if (address) *address = 0;
  if (!image) return DARWIN_ART_ELF_INVALID_ARGUMENT;
  return darwin_art_elf_lookup(image->elf, symbol, address, nullptr);
}
}
