#include "aim_bionic_builtin_adapters.h"
#include "aim_bionic_provider_namespace.h"

_Static_assert(AIM_BIONIC_PROVIDER_COUNT == 36, "provider count");
_Static_assert(sizeof(uintptr_t) == sizeof(void *), "address width");

int main(void) {
  AimBionicNamespaceResult result = {
      AIM_BIONIC_NAMESPACE_OK,
      AIM_BIONIC_PROVIDER_LEAF,
      1,
  };
  return result.address == 1 ? 0 : 1;
}
