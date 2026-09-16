#include "darwin_art_bionic_process_state.h"
#include <stdlib.h>

// Original bionic getter, with only exported names redirected to our property
// ABI. Source: bionic 361ba86734fb2821a6adcfdf775db8abd04e0de0.
#define __BIONIC_GET_DEVICE_API_LEVEL_INLINE
#define android_get_device_api_level darwin_art_bionic_android_get_device_api_level
#define __system_property_get darwin_art_bionic___system_property_get
#ifndef __attribute_pure__
#define __attribute_pure__ __attribute__((pure))
#endif
#pragma clang diagnostic push
#pragma clang diagnostic ignored "-Wstrict-prototypes"
#pragma clang diagnostic ignored "-Wnullability-extension"
#include "../upstream/get_device_api_level_inlines.h"
#pragma clang diagnostic pop
