#pragma once
#include <sys/system_properties.h>
// libbase's platform CachedProperty API uses these bionic entrypoints, which
// are not declared by the pinned public/NDK property header. Declarations only:
// unresolved backend symbols must not acquire a host-store implementation.
extern "C" uint32_t __system_property_area_serial();
extern "C" uint32_t __system_property_serial(const prop_info*);
