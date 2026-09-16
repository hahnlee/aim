#include "linker_image.h"

#include "namespace_loader_abi.h"

#include <cstddef>
#include <cstring>

namespace darwin_art::loader {
namespace {

constexpr char kSoname[] = "ld-android.so";
constexpr uint64_t kPayloadTag = 0x44415257494e4c44ULL;  // "DARWINLD".

// This marker is process-lifetime state only. It deliberately carries no
// owner/resource pointer and is compared by address before any dereference.
const void* Marker() {
  static const uint64_t marker = kPayloadTag;
  return &marker;
}

void SetError(std::string* error, const char* message) {
  if (error) *error = message;
}

bool IsOurSoname(const char* soname) {
  return soname != nullptr && std::strcmp(soname, kSoname) == 0;
}

bool IsValidCanonicalPath(const char* path) {
  return path != nullptr && path[0] == '/';
}

bool IsOurPayload(const void* payload) {
  return payload == Marker();
}

bool LeasePayload(const LinkerImageLease* lease) {
  if (!lease) return false;
  void* payload = nullptr;
  return darwin_art_linker_image_typed_payload(
             lease, DARWIN_ART_IMAGE_LINKER, &payload) == 0 &&
      IsOurPayload(payload);
}

void* Retain(void* source) {
  return IsOurPayload(source) ? source : nullptr;
}

void Release(void*) {}

uintptr_t LookupSymbol(const char* symbol) {
  // These are intentionally direct references, rather than host dlsym calls:
  // a missing owner is a link failure and cannot silently become a fake ABI.
  if (std::strcmp(symbol, "__loader_android_get_LD_LIBRARY_PATH") == 0)
    return reinterpret_cast<uintptr_t>(&__loader_android_get_LD_LIBRARY_PATH);
  if (std::strcmp(symbol, "__loader_android_update_LD_LIBRARY_PATH") == 0)
    return reinterpret_cast<uintptr_t>(&__loader_android_update_LD_LIBRARY_PATH);
  if (std::strcmp(symbol, "__loader_android_set_application_target_sdk_version") == 0)
    return reinterpret_cast<uintptr_t>(
        &__loader_android_set_application_target_sdk_version);
  if (std::strcmp(symbol, "__loader_android_init_anonymous_namespace") == 0)
    return reinterpret_cast<uintptr_t>(&__loader_android_init_anonymous_namespace);
  if (std::strcmp(symbol, "__loader_android_create_namespace") == 0)
    return reinterpret_cast<uintptr_t>(&__loader_android_create_namespace);
  if (std::strcmp(symbol, "__loader_android_link_namespaces") == 0)
    return reinterpret_cast<uintptr_t>(&__loader_android_link_namespaces);
  if (std::strcmp(symbol, "__loader_android_dlwarning") == 0)
    return reinterpret_cast<uintptr_t>(&__loader_android_dlwarning);
  if (std::strcmp(symbol, "__loader_android_get_exported_namespace") == 0)
    return reinterpret_cast<uintptr_t>(&__loader_android_get_exported_namespace);
  if (std::strcmp(symbol, "__loader_android_set_16kb_appcompat_mode") == 0)
    return reinterpret_cast<uintptr_t>(&__loader_android_set_16kb_appcompat_mode);

  // These existing owner entry points are part of the same private loader
  // boundary and are useful to libdl_android-adjacent callers.
  if (std::strcmp(symbol, "__loader_dlopen") == 0)
    return reinterpret_cast<uintptr_t>(&__loader_dlopen);
  if (std::strcmp(symbol, "__loader_android_dlopen_ext") == 0)
    return reinterpret_cast<uintptr_t>(&__loader_android_dlopen_ext);
  if (std::strcmp(symbol, "__loader_dlsym") == 0)
    return reinterpret_cast<uintptr_t>(&__loader_dlsym);
  if (std::strcmp(symbol, "__loader_dlvsym") == 0)
    return reinterpret_cast<uintptr_t>(&__loader_dlvsym);
  if (std::strcmp(symbol, "__loader_dlclose") == 0)
    return reinterpret_cast<uintptr_t>(&__loader_dlclose);
  if (std::strcmp(symbol, "__loader_dlerror") == 0)
    return reinterpret_cast<uintptr_t>(&__loader_dlerror);
  if (std::strcmp(symbol, "__loader_android_get_application_target_sdk_version") == 0)
    return reinterpret_cast<uintptr_t>(
        &__loader_android_get_application_target_sdk_version);
  return 0;
}

}  // namespace

bool PublishLinkerImage(LinkerRegistry* registry, uint64_t id,
    const char* soname, const char* canonical, std::string* error) {
  if (error) error->clear();
  if (!registry || !IsOurSoname(soname) || !IsValidCanonicalPath(canonical)) {
    SetError(error, "linker image requires ld-android.so and an absolute canonical path");
    return false;
  }
  if (darwin_art_linker_namespace_publish_image(
          registry, id, soname, canonical, const_cast<void*>(Marker()), Retain,
          Release, 0, 0, DARWIN_ART_IMAGE_LINKER) != 0) {
    SetError(error, "linker image publication failed");
    return false;
  }
  return true;
}

bool IsLinkerImage(const LinkerImageLease* lease) {
  return LeasePayload(lease);
}

int ResolveLinkerImage(const LinkerImageLease* lease, const char* symbol,
    const char* version, uintptr_t* out, std::string* error) {
  if (error) error->clear();
  if (out) *out = 0;
  if (!lease || !symbol || !*symbol || !out) {
    SetError(error, "invalid linker image lookup arguments");
    return -1;
  }
  if (!LeasePayload(lease)) {
    SetError(error, "lease is not a linker image owned by this provider");
    return -1;
  }
  if (version) {
    SetError(error, "versioned linker symbols are unsupported");
    return 1;
  }
  const uintptr_t address = LookupSymbol(symbol);
  if (!address) {
    SetError(error, "linker symbol is unavailable");
    return 1;
  }
  *out = address;
  return 0;
}

}  // namespace darwin_art::loader
