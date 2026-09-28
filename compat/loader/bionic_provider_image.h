#pragma once
#include "bionic_provider_set.h"
#include "aim_linker_namespace.h"
#include "android_unwind_image.h"

namespace aim::loader {
using SharedBionicProviders = std::shared_ptr<AimBionicNamespace>;
// Publishes a ported native provider, never an ELF or dyld handle. The trusted
// startup owner supplies the actual guest identity/placement. Finalizers must
// run before releasing their image leases and the last shared provider owner.
// Do not manually teardown/destroy the shared set while image leases exist.
bool PublishBionicProviderImage(LinkerRegistry*, uint64_t, const char* soname,
    const char* canonical, const SharedBionicProviders&, std::string* error,
    const SharedAndroidUnwind& unwind = {});
// Lease must remain live across use of a resolved address. Version/SONAME
// admission remains in the existing provider namespace; no dlsym fallback.
AimBionicNamespaceResult ResolveBionicProviderImage(const LinkerImageLease*,
    const char* symbol, const char* version);
}
