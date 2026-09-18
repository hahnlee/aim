#pragma once

#include <cstdint>

#include "darwin_art/darwin_art.h"

namespace darwin_art::embedding {

// Android app_process-equivalent entry for an installed APK or the shared
// service process.  Fixture activities, ELF checks and upstream test runners
// are intentionally outside this ABI and remain probe-owned.
int32_t RunProcess(const darwin_art_process_config_t* config,
                   darwin_art_process_result_t* result);

}  // namespace darwin_art::embedding
