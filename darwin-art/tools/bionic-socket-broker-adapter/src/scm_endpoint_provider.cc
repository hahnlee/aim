#include "scm_endpoint_provider.h"

#include <cerrno>
#include <cstdlib>
#include <limits>
#include <mutex>
#include <new>

namespace darwin_art::bionic::scm {
namespace {
struct Instance {
  explicit Instance(const DarwinArtScmEndpointProviderV1 &table) : original(table) {}
  const DarwinArtScmEndpointProviderV1 original;
  size_t references = 1; // Installation reference, removed only by uninstall.
  bool accepting = true;
};
std::mutex g_mutex;
Instance *g_instance = nullptr;
enum class Phase { Empty, Installing, Installed, Stopping, Retiring };
Phase g_phase = Phase::Empty;

bool Valid(const DarwinArtScmEndpointProviderV1 &table) noexcept {
  return table.abi_version == DARWIN_ART_SCM_ENDPOINT_ABI_VERSION &&
         table.struct_size == sizeof(table) && table.context != nullptr &&
         table.retain != nullptr && table.release != nullptr &&
         table.register_pair != nullptr && table.release_holder != nullptr;
}

void *Retain(void *context) noexcept {
  if (context == nullptr) return nullptr;
  auto *instance = static_cast<Instance *>(context);
  std::lock_guard lock(g_mutex);
  // An already owned reference can be retained while acquisition is stopped.
  // This permits final cleanup, not creation of new guest endpoints.
  if (g_instance != instance || instance->references == 0) std::abort();
  if (instance->references == std::numeric_limits<size_t>::max()) return nullptr;
  ++instance->references;
  return instance;
}

void Release(void *context) noexcept {
  if (context == nullptr) return;
  auto *instance = static_cast<Instance *>(context);
  std::lock_guard lock(g_mutex);
  if (g_instance != instance || instance->references <= 1) std::abort();
  --instance->references;
  // Never destroy on a foreign endpoint thread. Actual caller quiescence and
  // code-image teardown remain the process owner's uninstall responsibility.
}

int RegisterPair(void *context, const DarwinArtScmPairInstallerV1 *installer) noexcept {
  auto *instance = static_cast<Instance *>(Retain(context));
  if (instance == nullptr) return ENOMEM;
  bool accepting;
  {
    std::lock_guard lock(g_mutex);
    accepting = instance->accepting;
  }
  const int status = accepting
      ? instance->original.register_pair(instance->original.context, installer)
      : ESHUTDOWN;
  Release(instance);
  return status;
}

int ReleaseHolder(void *context, const uint8_t *holder) noexcept {
  auto *instance = static_cast<Instance *>(Retain(context));
  if (instance == nullptr) return ENOMEM;
  // Grant cleanup remains available after stop-acquisition.
  const int status = instance->original.release_holder(instance->original.context, holder);
  Release(instance);
  return status;
}

DarwinArtScmEndpointProviderV1 Wrapped(Instance *instance) noexcept {
  return {DARWIN_ART_SCM_ENDPOINT_ABI_VERSION, sizeof(DarwinArtScmEndpointProviderV1),
          instance, &Retain, &Release, &RegisterPair, &ReleaseHolder};
}
} // namespace

int AcquireProvider(DarwinArtScmEndpointProviderV1 *owned) noexcept {
  if (owned == nullptr) return EINVAL;
  *owned = {};
  std::lock_guard lock(g_mutex);
  if (g_phase != Phase::Installed || g_instance == nullptr) return ENOSYS;
  if (g_instance->references == std::numeric_limits<size_t>::max()) return ENOMEM;
  ++g_instance->references;
  *owned = Wrapped(g_instance);
  return 0;
}

int InstallProvider(const DarwinArtScmEndpointProviderV1 *table) noexcept {
  if (table == nullptr || !Valid(*table)) return EINVAL;
  auto retained = *table;
  {
    std::lock_guard lock(g_mutex);
    if (g_phase != Phase::Empty) return EALREADY;
    g_phase = Phase::Installing;
  }
  // Trusted external callbacks never run under the provider mutex.
  retained.context = table->retain(table->context);
  if (retained.context == nullptr) {
    std::lock_guard lock(g_mutex);
    g_phase = Phase::Empty;
    return ENOMEM;
  }
  auto *instance = new (std::nothrow) Instance(retained);
  if (instance == nullptr) {
    retained.release(retained.context);
    std::lock_guard lock(g_mutex);
    g_phase = Phase::Empty;
    return ENOMEM;
  }
  {
    std::lock_guard lock(g_mutex);
    if (g_phase != Phase::Installing || g_instance != nullptr) std::abort();
    g_instance = instance;
    g_phase = Phase::Installed;
  }
  return 0;
}

int UninstallProvider() noexcept {
  Instance *instance;
  {
    std::lock_guard lock(g_mutex);
    if (g_phase == Phase::Installing || g_phase == Phase::Retiring) return EBUSY;
    instance = g_instance;
    if (instance == nullptr) return 0;
    g_phase = Phase::Stopping;
    instance->accepting = false;
    if (instance->references != 1) return EBUSY;
    g_instance = nullptr;
    g_phase = Phase::Retiring;
  }
  // Owner has already drained actual native callers. Refcount alone does not
  // prove the final Release callback has returned from this code image.
  const auto original = instance->original;
  delete instance;
  original.release(original.context);
  {
    std::lock_guard lock(g_mutex);
    g_phase = Phase::Empty;
  }
  return 0;
}
} // namespace darwin_art::bionic::scm

extern "C" int darwin_art_bionic_install_scm_endpoint_provider(
    const DarwinArtScmEndpointProviderV1 *provider) {
  return darwin_art::bionic::scm::InstallProvider(provider);
}
extern "C" int darwin_art_bionic_uninstall_scm_endpoint_provider() {
  return darwin_art::bionic::scm::UninstallProvider();
}
