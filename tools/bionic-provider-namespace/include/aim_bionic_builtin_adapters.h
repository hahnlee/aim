#ifndef AIM_BIONIC_BUILTIN_ADAPTERS_H_
#define AIM_BIONIC_BUILTIN_ADAPTERS_H_

#include "aim_bionic_provider_namespace.h"

#ifdef __cplusplus
extern "C" {
#endif

typedef struct AimBionicProviderReleaseHooks {
  void *context[AIM_BIONIC_PROVIDER_COUNT];
  AimBionicProviderRelease release[AIM_BIONIC_PROVIDER_COUNT];
} AimBionicProviderReleaseHooks;

/* Binds adapters for all generated provider resolver entrypoints.
 * Linking this object intentionally requires every standalone provider. */
AimBionicNamespaceStatus aim_bionic_namespace_bind_builtins(
    AimBionicNamespace *namespace_instance,
    const AimBionicProviderReleaseHooks *release_hooks);

#ifdef __cplusplus
}
#endif

#endif // AIM_BIONIC_BUILTIN_ADAPTERS_H_
