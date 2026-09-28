#include "android_unwind_image.h"
#include "bionic_symbol_lookup.h"
#include <cstring>

namespace aim::loader {
struct AndroidUnwindImage {
  std::shared_ptr<AimBionicNamespace> providers;
  AimElfHandle* elf = nullptr;
  ~AndroidUnwindImage() { aim_elf_unload(&elf, nullptr); }
};
namespace {
AimElfResolveStatus Resolve(void* context, const AimElfSymbolRequest* request,
    uintptr_t* address, AimElfErrorBuffer*) {
  if (address) *address = 0;
  if (!context || !request || !address || request->abi_version != AIM_ELF_ABI_VERSION ||
      !request->symbol || (request->needed_library_count && !request->needed_libraries))
    return AIM_ELF_RESOLVE_ERROR;
  auto* providers = static_cast<AimBionicNamespace*>(context);
  AimBionicNamespaceResult result{};
  if (request->version_soname || request->version_name) {
    if (!request->version_soname || !request->version_name) return AIM_ELF_RESOLVE_ERROR;
    bool admitted = false;
    for (size_t i = 0; i < request->needed_library_count; ++i)
      if (request->needed_libraries[i] && std::strcmp(request->needed_libraries[i], request->version_soname) == 0)
        admitted = true;
    if (!admitted) return AIM_ELF_RESOLVE_ERROR;
    result = aim_bionic_namespace_resolve(providers, request->version_soname,
        request->symbol, request->version_name);
  } else {
    result = LookupBionicDependencies(providers, request->needed_libraries,
        request->needed_library_count, request->symbol);
  }
  if (result.status == AIM_BIONIC_NAMESPACE_OK && result.address) {
    *address = result.address;
    return AIM_ELF_RESOLVE_FOUND;
  }
  return IsBionicSymbolMiss(result.status) ? AIM_ELF_RESOLVE_NOT_FOUND : AIM_ELF_RESOLVE_ERROR;
}
}
SharedAndroidUnwind LoadAndroidUnwindImage(const char* path,
    std::shared_ptr<AimBionicNamespace> providers, std::string* error) {
  if (error) error->clear();
  if (!path || *path != '/' || !providers) {
    if (error) *error = "missing installed unwind image/dependency owner";
    return nullptr;
  }
  auto image = std::make_shared<AndroidUnwindImage>();
  image->providers = std::move(providers);
  AimElfLoadOptions options{AIM_ELF_ABI_VERSION, Resolve, image->providers.get()};
  char text[2048]{};
  AimElfErrorBuffer detail{text, sizeof(text), 0};
  auto status = aim_elf_load_path(path, &options, &image->elf, &detail);
  if (status == AIM_ELF_OK) status = aim_elf_run_initializers(image->elf, &detail);
  if (status != AIM_ELF_OK) {
    if (error) *error = *text ? text : aim_elf_status_name(status);
    return nullptr;
  }
  return image;
}
AimElfStatus LookupAndroidUnwind(const SharedAndroidUnwind& image,
    const char* symbol, uintptr_t* address) {
  if (address) *address = 0;
  if (!image) return AIM_ELF_INVALID_ARGUMENT;
  return aim_elf_lookup(image->elf, symbol, address, nullptr);
}
}
