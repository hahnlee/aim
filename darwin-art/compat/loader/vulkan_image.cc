#include "vulkan_image.h"

#include "darwin_art_dso_namespace.h"

#include <cstring>
#include <mutex>
#include <new>
#include <string>
#include <unordered_set>

namespace darwin_art::loader {
namespace {

constexpr char kSoname[] = "libvulkan.so";
constexpr uint64_t kPayloadTag = 0x44415257494e564bULL;  // "DARWINVK".

struct VulkanImagePayload {
  uint64_t tag;
  std::string soname;
  std::string canonical;
};

// The registry borrows a typed payload while a lease is live.  Keep ownership
// validation private to this provider so a lease from another image owner is
// rejected before its payload is inspected.
std::mutex& PayloadMutex() {
  static auto* mutex = new std::mutex();
  return *mutex;
}

std::unordered_set<const VulkanImagePayload*>& LivePayloads() {
  static auto* payloads = new std::unordered_set<const VulkanImagePayload*>();
  return *payloads;
}

void SetError(std::string* error, const char* message) {
  if (error) *error = message;
}

bool IsOurSoname(const char* soname) {
  return soname != nullptr && std::strcmp(soname, kSoname) == 0;
}

bool IsValidCanonicalPath(const char* canonical) {
  return canonical != nullptr && canonical[0] == '/';
}

bool IsLivePayload(const void* payload) {
  if (!payload) return false;
  const auto* image = static_cast<const VulkanImagePayload*>(payload);
  std::lock_guard<std::mutex> lock(PayloadMutex());
  // Membership is checked before reading the object, so an arbitrary typed
  // payload cannot be treated as this provider's C++ representation.
  if (LivePayloads().find(image) == LivePayloads().end()) return false;
  return image->tag == kPayloadTag && IsOurSoname(image->soname.c_str());
}

bool LeasePayload(const LinkerImageLease* lease) {
  if (!lease) return false;
  void* payload = nullptr;
  return darwin_art_linker_image_typed_payload(
             lease, DARWIN_ART_IMAGE_VULKAN, &payload) == 0 &&
      IsLivePayload(payload);
}

void* Retain(void* source) {
  if (!source) return nullptr;
  const auto* image = static_cast<const VulkanImagePayload*>(source);
  if (image->tag != kPayloadTag || !IsOurSoname(image->soname.c_str()))
    return nullptr;

  VulkanImagePayload* retained = nullptr;
  try {
    retained = new VulkanImagePayload(*image);
    std::lock_guard<std::mutex> lock(PayloadMutex());
    LivePayloads().insert(retained);
  } catch (...) {
    delete retained;
    return nullptr;
  }
  return retained;
}

void Release(void* source) {
  if (!source) return;
  auto* image = static_cast<VulkanImagePayload*>(source);
  bool owned = false;
  {
    std::lock_guard<std::mutex> lock(PayloadMutex());
    const auto it = LivePayloads().find(image);
    if (it != LivePayloads().end()) {
      LivePayloads().erase(it);
      owned = true;
    }
  }
  if (owned) delete image;
}

}  // namespace

bool PublishVulkanImage(LinkerRegistry* registry, uint64_t id,
    const char* soname, const char* canonical, std::string* error) {
  if (error) error->clear();
  if (!registry || !IsOurSoname(soname) || !IsValidCanonicalPath(canonical)) {
    SetError(error,
        "Vulkan image requires libvulkan.so and an absolute canonical path");
    return false;
  }

  // The Rust boundary reports readiness only when the process-wide MoltenVK
  // owner exists.  This deliberately rejects the legacy no-driver fallback
  // PFNs exposed through bionic dlsym.
  if (darwin_art_bionic_vulkan_provider_ready() != 1) {
    SetError(error, "Vulkan image provider is not ready");
    return false;
  }

  try {
    VulkanImagePayload image{kPayloadTag, soname, canonical};
    if (darwin_art_linker_namespace_publish_image(
            registry, id, soname, canonical, &image, Retain, Release, 0, 0,
            DARWIN_ART_IMAGE_VULKAN) != 0) {
      SetError(error, "Vulkan image publication failed");
      return false;
    }
  } catch (...) {
    SetError(error, "Vulkan image allocation failed");
    return false;
  }
  return true;
}

bool IsVulkanImage(const LinkerImageLease* lease) {
  return LeasePayload(lease);
}

int ResolveVulkanImage(const LinkerImageLease* lease, const char* symbol,
    const char* version, uintptr_t* out, std::string* error) {
  if (error) error->clear();
  if (out) *out = 0;
  if (!lease || !symbol || !*symbol || !out) {
    SetError(error, "invalid Vulkan image lookup arguments");
    return -1;
  }
  if (!LeasePayload(lease)) {
    SetError(error, "lease is not a Vulkan image owned by this provider");
    return -1;
  }
  if (version) {
    SetError(error, "versioned Vulkan symbols are unsupported");
    return 1;
  }

  const int status = darwin_art_bionic_vulkan_symbol(symbol, version, out);
  switch (status) {
    case 0:
      if (*out != 0) return 0;
      SetError(error, "Vulkan provider returned a null symbol");
      return 1;
    case 1:
      SetError(error, "Vulkan symbol is unavailable");
      return 1;
    case 2:
      SetError(error, "versioned Vulkan symbols are unsupported");
      return 1;
    case 3:
      SetError(error, "Vulkan provider rejected lookup arguments");
      return -1;
    default:
      SetError(error, "Vulkan provider returned an invalid lookup status");
      return 1;
  }
}

}  // namespace darwin_art::loader
