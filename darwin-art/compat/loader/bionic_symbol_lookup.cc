#include "bionic_symbol_lookup.h"

namespace darwin_art::loader {
bool IsBionicSymbolMiss(DarwinArtBionicNamespaceStatus status) {
  return status == DARWIN_ART_BIONIC_NAMESPACE_UNKNOWN_SONAME ||
      status == DARWIN_ART_BIONIC_NAMESPACE_UNKNOWN_VERSION ||
      status == DARWIN_ART_BIONIC_NAMESPACE_UNSUPPORTED_SYMBOL;
}
DarwinArtBionicNamespaceResult LookupBionicDependencies(DarwinArtBionicNamespace* owner,
    const char* const* sonames, size_t count, const char* symbol) {
  if (!owner || !symbol || !*symbol || (count && !sonames))
    return {DARWIN_ART_BIONIC_NAMESPACE_INVALID_ARGUMENT, DARWIN_ART_BIONIC_PROVIDER_COUNT, 0};
  for (size_t i = 0; i < count; ++i) {
    const auto result = darwin_art_bionic_namespace_resolve(owner, sonames[i], symbol, nullptr);
    if (!IsBionicSymbolMiss(result.status)) return result;
  }
  return {DARWIN_ART_BIONIC_NAMESPACE_UNSUPPORTED_SYMBOL, DARWIN_ART_BIONIC_PROVIDER_COUNT, 0};
}
}
