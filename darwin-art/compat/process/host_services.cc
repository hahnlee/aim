#include "host_services.h"

#include <mutex>

namespace darwin_art::process {
namespace {

std::mutex g_host_services_mutex;
const darwin_art_host_services_t* g_host_services = nullptr;

bool IsValid(const darwin_art_host_services_t* services) {
  return services == nullptr ||
         (services->struct_size >= sizeof(*services) &&
          services->abi_version == DARWIN_ART_ABI_VERSION &&
          services->context != nullptr && services->spawn_service != nullptr &&
          services->release_service != nullptr);
}

}  // namespace

bool InstallHostServices(const darwin_art_host_services_t* services) {
  if (!IsValid(services)) return false;
  std::lock_guard<std::mutex> lock(g_host_services_mutex);
  g_host_services = services;
  return true;
}

int32_t SpawnServiceProcess(const char* component, const char* instance_name,
                            const char* process_name, bool isolated,
                            int32_t* host_pid, int32_t* control_fd) {
  const darwin_art_host_services_t* services = nullptr;
  {
    std::lock_guard<std::mutex> lock(g_host_services_mutex);
    services = g_host_services;
  }
  if (services == nullptr) return -2;

  darwin_art_service_spawn_request_t request{
      sizeof(request), DARWIN_ART_ABI_VERSION, component, instance_name,
      process_name, isolated ? 1 : 0};
  darwin_art_service_spawn_result_t result{
      sizeof(result), DARWIN_ART_ABI_VERSION, -1, -1};
  const int32_t status =
      services->spawn_service(services->context, &request, &result);
  if (status == 0 && host_pid != nullptr && control_fd != nullptr) {
    *host_pid = result.host_pid;
    *control_fd = result.control_fd;
  }
  return status;
}

int32_t ReleaseServiceProcess(int32_t host_pid) {
  const darwin_art_host_services_t* services = nullptr;
  {
    std::lock_guard<std::mutex> lock(g_host_services_mutex);
    services = g_host_services;
  }
  if (services == nullptr) return -2;
  return services->release_service(services->context, host_pid);
}

void ClearHostServices() {
  std::lock_guard<std::mutex> lock(g_host_services_mutex);
  g_host_services = nullptr;
}

}  // namespace darwin_art::process
