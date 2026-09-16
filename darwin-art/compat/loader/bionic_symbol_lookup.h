#pragma once
#include "darwin_art_bionic_provider_namespace.h"

namespace darwin_art::loader {
// A lookup miss permits the next image; lifecycle/provider failures do not.
bool IsBionicSymbolMiss(DarwinArtBionicNamespaceStatus);
// Borrowed sealed owner. Search in supplied order, never dyld/global fallback.
// Ordered graph relocation supplies one selected image; standalone callers may
// supply an ordered dependency list. First definition wins, not unique match.
DarwinArtBionicNamespaceResult LookupBionicDependencies(DarwinArtBionicNamespace*,
    const char* const* sonames, size_t count, const char* symbol);
}
