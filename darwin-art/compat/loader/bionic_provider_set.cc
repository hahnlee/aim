#include "bionic_provider_set.h"
#include "darwin_art_bionic_builtin_adapters.h"

namespace darwin_art::loader {
void BionicProviderDrop::operator()(DarwinArtBionicNamespace* value) const {
  darwin_art_bionic_namespace_destroy(value);
}
BionicProviderSet CreateBionicProviderSet(std::string* error) {
  if (error) error->clear();
  BionicProviderSet owner(darwin_art_bionic_namespace_create());
  if (!owner) {
    if (error) *error = "Bionic provider namespace allocation failed";
    return nullptr;
  }
  auto status = darwin_art_bionic_namespace_bind_builtins(owner.get(), nullptr);
  if (status == DARWIN_ART_BIONIC_NAMESPACE_OK)
    status = darwin_art_bionic_namespace_seal(owner.get());
  if (status != DARWIN_ART_BIONIC_NAMESPACE_OK) {
    if (error) *error = std::string("Bionic provider setup failed: ") +
        darwin_art_bionic_namespace_status_name(status);
    return nullptr;
  }
  return owner;
}
}
