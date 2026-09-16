#pragma once

#include "darwin_art_linker_namespace.h"

#include <cstdint>
#include <string>

namespace darwin_art::loader {

// libvulkan.so is an Android ABI facade over the process-resident MoltenVK
// owner.  The typed image carries only immutable Android image identity; it
// is not a dyld handle and closing an image must never unload MoltenVK.
bool PublishVulkanImage(LinkerRegistry* registry, uint64_t id,
    const char* soname, const char* canonical, std::string* error);

// Resolve Android Vulkan entry points only through the real provider.  A
// non-null Android symbol version is unsupported because this virtual image
// has no version definition.  Returns zero for a resolved symbol, one for an
// unavailable/unsupported symbol, and -1 for an invalid lease or argument.
int ResolveVulkanImage(const LinkerImageLease* lease, const char* symbol,
    const char* version, uintptr_t* out, std::string* error);

// Identifies an image published by PublishVulkanImage without dereferencing an
// arbitrary image payload.  A generic image publication is not sufficient.
bool IsVulkanImage(const LinkerImageLease* lease);

}  // namespace darwin_art::loader
