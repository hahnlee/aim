#pragma once

#include <cstdint>

#include "aim/aim.h"

namespace aim::embedding {

// Android app_process-equivalent entry for an installed APK or the shared
// service process.  Fixture activities, ELF checks and upstream test runners
// are intentionally outside this ABI and remain probe-owned.
int32_t RunProcess(const aim_process_config_t* config,
                   aim_process_result_t* result);

}  // namespace aim::embedding
