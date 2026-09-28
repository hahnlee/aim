#include "bionic_symbol_lookup.h"

namespace aim::loader {
bool IsBionicSymbolMiss(AimBionicNamespaceStatus status) {
  return status == AIM_BIONIC_NAMESPACE_UNKNOWN_SONAME ||
      status == AIM_BIONIC_NAMESPACE_UNKNOWN_VERSION ||
      status == AIM_BIONIC_NAMESPACE_UNSUPPORTED_SYMBOL;
}
AimBionicNamespaceResult LookupBionicDependencies(AimBionicNamespace* owner,
    const char* const* sonames, size_t count, const char* symbol) {
  if (!owner || !symbol || !*symbol || (count && !sonames))
    return {AIM_BIONIC_NAMESPACE_INVALID_ARGUMENT, AIM_BIONIC_PROVIDER_COUNT, 0};
  for (size_t i = 0; i < count; ++i) {
    const auto result = aim_bionic_namespace_resolve(owner, sonames[i], symbol, nullptr);
    if (!IsBionicSymbolMiss(result.status)) return result;
  }
  return {AIM_BIONIC_NAMESPACE_UNSUPPORTED_SYMBOL, AIM_BIONIC_PROVIDER_COUNT, 0};
}
}
