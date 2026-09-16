#include "bionic_provider_image.h"
#include "libandroid_symbols.h"
#include <new>
#include <cstring>

namespace darwin_art::loader {
namespace {
struct Image {
  SharedBionicProviders providers;
  std::string soname;
  SharedAndroidUnwind unwind;
};
void* Retain(void* source) {
  try { return new Image(*static_cast<Image*>(source)); }
  catch (...) { return nullptr; }
}
void Release(void* image) { delete static_cast<Image*>(image); }
}
bool PublishBionicProviderImage(LinkerRegistry* registry, uint64_t id,
    const char* soname, const char* canonical, const SharedBionicProviders& providers,
    std::string* error, const SharedAndroidUnwind& unwind) {
  if (error) error->clear();
  if (!providers || !soname || !*soname) {
    if (error) *error = "missing provider image identity/owner";
    return false;
  }
  if (darwin_art_bionic_namespace_image_status(providers.get(), soname)
      != DARWIN_ART_BIONIC_NAMESPACE_OK) {
    if (error) *error = "provider is unsealed, unavailable or does not own this SONAME";
    return false;
  }
  Image image{providers, soname, unwind};
  if (darwin_art_linker_namespace_publish_image(registry, id, soname, canonical,
      &image, Retain, Release, 0, 0, DARWIN_ART_IMAGE_BIONIC_PROVIDER) != 0) {
    if (error) *error = "provider image publication failed";
    return false;
  }
  return true;
}
DarwinArtBionicNamespaceResult ResolveBionicProviderImage(const LinkerImageLease* lease,
    const char* symbol, const char* version) {
  void* payload = nullptr;
  if (darwin_art_linker_image_typed_payload(lease, DARWIN_ART_IMAGE_BIONIC_PROVIDER, &payload) != 0)
    return {DARWIN_ART_BIONIC_NAMESPACE_INVALID_ARGUMENT, DARWIN_ART_BIONIC_PROVIDER_COUNT, 0};
  const auto* image = static_cast<const Image*>(payload);
  if (image->soname == "libc.so" && version && std::strcmp(version, "LIBC_R") == 0 && image->unwind) {
    uintptr_t address = 0;
    const auto status = LookupAndroidUnwind(image->unwind, symbol, &address);
    return {status == DARWIN_ART_ELF_OK ? DARWIN_ART_BIONIC_NAMESPACE_OK :
        status == DARWIN_ART_ELF_SYMBOL_NOT_FOUND ? DARWIN_ART_BIONIC_NAMESPACE_UNSUPPORTED_SYMBOL :
        DARWIN_ART_BIONIC_NAMESPACE_PROVIDER_REJECTED, DARWIN_ART_BIONIC_PROVIDER_COUNT, address};
  }
  const auto bionic = darwin_art_bionic_namespace_resolve(image->providers.get(), image->soname.c_str(), symbol, version);
  if (image->soname != "libandroid.so") return bionic;
  uintptr_t address = 0;
  const int platform = ResolveLibandroidPlatformSymbol(symbol, version, &address);
  if (platform < 0 || (platform == 0 && bionic.status == DARWIN_ART_BIONIC_NAMESPACE_OK))
    return {DARWIN_ART_BIONIC_NAMESPACE_PROVIDER_REJECTED, DARWIN_ART_BIONIC_PROVIDER_COUNT, 0};
  if (platform == 0) return {DARWIN_ART_BIONIC_NAMESPACE_OK, DARWIN_ART_BIONIC_PROVIDER_COUNT, address};
  return bionic;
}
}
