#pragma once

#include <cstdint>

#include "darwin_art/darwin_art.h"

namespace darwin_art::process {

// The host owns the callback table and keeps it alive for the duration of an
// ART process. This boundary validates and borrows that table; it does not
// copy or free the host-owned context.
bool InstallHostServices(const darwin_art_host_services_t* services);

int32_t SpawnServiceProcess(const char* component, const char* instance_name,
                            const char* process_name, bool isolated,
                            int32_t* host_pid, int32_t* control_fd);

int32_t ReleaseServiceProcess(int32_t host_pid);

void ClearHostServices();

}  // namespace darwin_art::process
