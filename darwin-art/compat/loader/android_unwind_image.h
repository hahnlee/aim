#pragma once
#include "bionic_provider_set.h"
#include "darwin_art_elf_loader.h"
#include <memory>

namespace darwin_art::loader {
struct AndroidUnwindImage;
using SharedAndroidUnwind = std::shared_ptr<AndroidUnwindImage>;
// Trusted installed Android libunwind provider, not host libunwind/dyld.
// Keeps its Bionic dependencies alive until after the ELF is unloaded.
SharedAndroidUnwind LoadAndroidUnwindImage(const char* path,
    std::shared_ptr<DarwinArtBionicNamespace> providers, std::string* error);
DarwinArtElfStatus LookupAndroidUnwind(const SharedAndroidUnwind&,
    const char* symbol, uintptr_t* address);
}
