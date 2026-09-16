// NativeLoader's original SDK helpers use public bionic symbol names. Keep
// the ABI alias here; values and getter semantics belong to the process-state
// facade and the imported original bionic getter, not to NativeLoader policy.
#include "darwin_art_bionic_process_state.h"

extern "C" int android_get_device_api_level() {
  return darwin_art_bionic_android_get_device_api_level();
}
