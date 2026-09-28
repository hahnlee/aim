#pragma once
#include "aim_bionic_provider_namespace.h"

namespace aim::loader {
// A lookup miss permits the next image; lifecycle/provider failures do not.
bool IsBionicSymbolMiss(AimBionicNamespaceStatus);
// Borrowed sealed owner. Search in supplied order, never dyld/global fallback.
// Ordered graph relocation supplies one selected image; standalone callers may
// supply an ordered dependency list. First definition wins, not unique match.
AimBionicNamespaceResult LookupBionicDependencies(AimBionicNamespace*,
    const char* const* sonames, size_t count, const char* symbol);
}
