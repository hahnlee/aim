#ifndef DARWIN_ANDROID_ELF_IMAGE_REGISTRY_H_
#define DARWIN_ANDROID_ELF_IMAGE_REGISTRY_H_

#include <stddef.h>
#include <stdint.h>

#include <string>

#include "darwin_art_elf_loader.h"

namespace android::darwin_art_image_registry {

class Owner;

// Builds the dependency-first publication plan and process-lifetime
// dl_iterate_phdr snapshot source for one closed Android ELF graph.
Owner* Create(const char* root_soname,
              const char* library_directory,
              const DarwinArtElfGraphSource* sources,
              size_t source_count,
              const char* const* provider_sonames,
              size_t provider_count,
              std::string* error);

// Namespace discovery supplies the exact admitted guest path for each source,
// in source order. Copies paths; never reconstructs them from SONAME or resolves
// them again. Legacy Create remains for the old single-directory loader only.
Owner* CreateWithPaths(const char* root_soname,
                      const DarwinArtElfGraphSource* sources, size_t source_count,
                      const char* const* source_paths,
                      const char* const* provider_sonames, size_t provider_count,
                      std::string* error);

// Called synchronously by the loader's dependency-first lifecycle callback.
int Publish(Owner *owner, uintptr_t start, uintptr_t end);

// Undoes only the most recent exact publication when the following chained
// lifecycle publisher rejects it.
int RollbackPublish(Owner *owner, uintptr_t start, uintptr_t end);

// Removes one exact image in dependency-reverse order after Bionic callback
// ownership has drained, while the loader still keeps the mapping live.
int Finalize(Owner *owner, uintptr_t start, uintptr_t end);

// Returns true only while address belongs to an exact published image in this
// owner. JNI registration uses this before creating a callable host thunk.
bool ContainsAddress(const Owner *owner, uintptr_t address);

// Bionic dladdr-compatible lookup for addresses in published Android ELF
// images. `info` points to the four-field Dl_info ABI used by Bionic.
extern "C" int darwin_art_android_elf_dladdr(const void* address, void* info);

// Transactional load failures can leave an already-published prefix. This
// removes any remaining prefix before its owner is destroyed.
void UnpublishAll(Owner *owner);
void Destroy(Owner *owner);

}  // namespace android::darwin_art_image_registry

#endif  // DARWIN_ANDROID_ELF_IMAGE_REGISTRY_H_
