#include "bionic_provider_set.h"
#include "aim_bionic_builtin_adapters.h"

namespace aim::loader {
void BionicProviderDrop::operator()(AimBionicNamespace* value) const {
  aim_bionic_namespace_destroy(value);
}
BionicProviderSet CreateBionicProviderSet(std::string* error) {
  if (error) error->clear();
  BionicProviderSet owner(aim_bionic_namespace_create());
  if (!owner) {
    if (error) *error = "Bionic provider namespace allocation failed";
    return nullptr;
  }
  auto status = aim_bionic_namespace_bind_builtins(owner.get(), nullptr);
  if (status == AIM_BIONIC_NAMESPACE_OK)
    status = aim_bionic_namespace_seal(owner.get());
  if (status != AIM_BIONIC_NAMESPACE_OK) {
    if (error) *error = std::string("Bionic provider setup failed: ") +
        aim_bionic_namespace_status_name(status);
    return nullptr;
  }
  return owner;
}
}
