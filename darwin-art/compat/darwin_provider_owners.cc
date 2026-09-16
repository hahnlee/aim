#include "darwin_provider_owners.h"
#include "filesystem/archive_open.h"

#include <sys/stat.h>
#include <unistd.h>

#include <array>
#include <cstdint>
#include <cstdlib>
#include <mutex>
#include <string>
#include <utility>

#include "darwin_art_bionic_fs.h"
#include "darwin_art_bionic_ioctl.h"
#include "darwin_art_bionic_sendfile.h"
#include "darwin_art_bionic_socket_broker.h"
#include "darwin_art_bionic_stdio.h"
#include "darwin_art_bionic_strftime.h"
extern "C" void darwin_art_bionic_process_state_bind_jit_fault_recovery(
    int (*recovery)(uintptr_t fault_address, int execution_fault));
extern "C" void darwin_art_bionic_process_state_bind_sigchain(
    int (*owns_signal)(int signal_number),
    void (*ensure_front)(int signal_number));
extern "C" int darwin_art_sigchain_owns_signal(int signal_number);
extern "C" void EnsureFrontOfChain(int signal_number);
extern "C" int darwin_art_bionic_vm_bind_file_descriptor_resolver(
    int (*resolver)(int guest_fd, int *host_fd));
extern "C" int darwin_art_bionic_vm_process_install();
extern "C" int darwin_art_bionic_vm_process_uninstall();
extern "C" int
darwin_art_bionic_vm_recover_jit_execution_fault(uintptr_t fault_address,
                                                 int execution_fault);

__attribute__((constructor)) static void BindJitFaultRecovery() {
  if (darwin_art_bionic_fs_bind_host_descriptor_resolver(
          &darwin_art_bionic_fd_dup_host_fd_core) != 0)
    std::abort();
  darwin_art_bionic_process_state_bind_jit_fault_recovery(
      &darwin_art_bionic_vm_recover_jit_execution_fault);
  darwin_art_bionic_process_state_bind_sigchain(
      &darwin_art_sigchain_owns_signal, &EnsureFrontOfChain);
}

__attribute__((destructor)) static void UnbindJitFaultRecovery() {
  darwin_art_bionic_process_state_bind_sigchain(nullptr, nullptr);
  darwin_art_bionic_process_state_bind_jit_fault_recovery(nullptr);
  if (darwin_art_bionic_fs_bind_host_descriptor_resolver(nullptr) != 0)
    std::abort();
}

extern "C" DarwinArtBionicSendfileTransferStatus
darwin_art_bionic_fs_sendfile_transfer(
    void *context, const DarwinArtBionicSendfileRequest *request,
    DarwinArtBionicSendfileResult *result);

namespace darwin_art::providers {
namespace {

struct HookState {
  std::mutex mutex;
  void *context = nullptr;
  AcquireHook acquire = nullptr;
  ReleaseHook release = nullptr;
};

HookState g_hooks;

// A process-wide filesystem facade is intentionally singleton-owned.  A
// native library loaded after ActivityThread has installed the app/system
// authority must borrow that owner rather than attempting a second install;
// otherwise JavaVMExt rejects the library even though its path is trusted.
std::mutex g_filesystem_borrow_mutex;
uint32_t g_filesystem_borrowed_leases = 0;

struct HookSnapshot {
  void *context;
  AcquireHook acquire;
  ReleaseHook release;
};

HookSnapshot hooks() {
  std::lock_guard<std::mutex> lock(g_hooks.mutex);
  return {g_hooks.context, g_hooks.acquire, g_hooks.release};
}

} // namespace

bool acquire_filesystem_direct(int directory_fd, std::string *error) {
  struct stat status{};
  if (directory_fd < 0 || fstat(directory_fd, &status) != 0 ||
      !S_ISDIR(status.st_mode)) {
    *error = "trusted filesystem authority is not a live directory";
    return false;
  }
  constexpr uint8_t kGuestRoot[] = {'/'};
  const DarwinArtBionicFsProcessOwnerStatus status_code =
      darwin_art_bionic_fs_process_install(directory_fd, kGuestRoot,
                                           sizeof(kGuestRoot), kGuestRoot,
                                           sizeof(kGuestRoot));
  if (status_code != DARWIN_ART_BIONIC_FS_PROCESS_OWNER_OK) {
    *error = "Bionic filesystem process-owner install failed: " +
             std::to_string(static_cast<int>(status_code));
    return false;
  }
  const char *app_data_guest_dir =
      std::getenv("DARWIN_ART_APK_APP_DATA_GUEST_DIR");
  if (app_data_guest_dir != nullptr && app_data_guest_dir[0] != '\0') {
    constexpr std::array<const char *, 6> kAppDataDirectories = {
        "files",     "cache",     "code_cache",
        "no_backup", "databases", "shared_prefs"};
    if (darwin_art_bionic_fs_seed_private_directory(app_data_guest_dir) != 0) {
      *error = "Bionic private app directory seed failed";
      (void)darwin_art_bionic_fs_process_uninstall();
      return false;
    }
    for (const char *child : kAppDataDirectories) {
      const std::string guest_path =
          std::string(app_data_guest_dir) + "/" + child;
      if (darwin_art_bionic_fs_seed_private_directory(guest_path.c_str()) !=
          0) {
        *error = "Bionic standard app directory seed failed";
        (void)darwin_art_bionic_fs_process_uninstall();
        return false;
      }
    }
  }
  darwin_art_install_archive_filesystem(true);
  return true;
}

bool seed_filesystem_private_directory(const char *guest_path,
                                       std::string *error) {
  if (guest_path == nullptr || guest_path[0] != '/' ||
      darwin_art_bionic_fs_seed_private_directory(guest_path) != 0) {
    *error = "Bionic private app directory seed failed";
    return false;
  }
  return true;
}

void release_filesystem_direct() {
  darwin_art_install_archive_filesystem(false);
  if (darwin_art_bionic_fs_process_uninstall() !=
      DARWIN_ART_BIONIC_FS_PROCESS_OWNER_OK) {
    std::abort();
  }
}

bool acquire_network_direct(std::string *error) {
  if (darwin_art_bionic_socket_broker_activate() != 0) {
    *error = "Bionic socket broker activation failed";
    return false;
  }
  return true;
}

void release_network_direct() {
  if (darwin_art_bionic_socket_broker_deactivate() != 0) {
    std::abort();
  }
}

bool acquire_stdio_direct(std::string *error) {
  if (darwin_art_bionic_stdio_process_install() == 0)
    return true;
  *error = "Bionic stdio process-owner install failed";
  return false;
}

void release_stdio_direct() {
  if (darwin_art_bionic_stdio_process_uninstall() != 0)
    std::abort();
}

bool acquire_ioctl_direct(std::string *error) {
  const DarwinArtBionicIoctlLifecycleStatus status =
      darwin_art_bionic_ioctl_activate(&darwin_art_bionic_fs_ioctl_fd_lookup,
                                       nullptr);
  if (status != DARWIN_ART_BIONIC_IOCTL_LIFECYCLE_OK) {
    *error = "Bionic ioctl process-owner activation failed: " +
             std::to_string(static_cast<int>(status));
    return false;
  }
  return true;
}

void release_ioctl_direct() {
  if (darwin_art_bionic_ioctl_deactivate() !=
      DARWIN_ART_BIONIC_IOCTL_LIFECYCLE_OK)
    std::abort();
}

bool acquire_strftime_direct(std::string *error) {
  const DarwinArtBionicStrftimeLifecycleStatus status =
      darwin_art_bionic_strftime_activate("UTC", 0, "UTC", 0);
  if (status != DARWIN_ART_BIONIC_STRFTIME_OK) {
    *error = "Bionic strftime process-owner activation failed: " +
             std::to_string(static_cast<int>(status));
    return false;
  }
  return true;
}

void release_strftime_direct() {
  if (darwin_art_bionic_strftime_deactivate() != DARWIN_ART_BIONIC_STRFTIME_OK)
    std::abort();
}

bool acquire_sendfile_direct(std::string *error) {
  const DarwinArtBionicSendfileLifecycleStatus status =
      darwin_art_bionic_sendfile_activate(
          &darwin_art_bionic_fs_sendfile_transfer, nullptr);
  if (status != DARWIN_ART_BIONIC_SENDFILE_LIFECYCLE_OK) {
    *error = "Bionic sendfile process-owner activation failed: " +
             std::to_string(static_cast<int>(status));
    return false;
  }
  return true;
}

void release_sendfile_direct() {
  if (darwin_art_bionic_sendfile_deactivate() !=
      DARWIN_ART_BIONIC_SENDFILE_LIFECYCLE_OK)
    std::abort();
}

bool acquire_vm_direct(std::string *error) {
  if (darwin_art_bionic_vm_bind_file_descriptor_resolver(
          &darwin_art_bionic_fd_dup_host_fd_core) != 0) {
    *error = "Bionic VM descriptor bridge failed";
    return false;
  }
  if (darwin_art_bionic_vm_process_install() == 0)
    return true;
  (void)darwin_art_bionic_vm_bind_file_descriptor_resolver(nullptr);
  *error = "Bionic VM process-owner install failed";
  return false;
}

void release_vm_direct() {
  if (darwin_art_bionic_vm_process_uninstall() != 0)
    std::abort();
}

bool acquire_filesystem(int directory_fd, std::string *error) {
  const HookSnapshot hook = hooks();
  if (hook.acquire != nullptr) {
    {
      std::lock_guard<std::mutex> lock(g_filesystem_borrow_mutex);
      if (darwin_art_bionic_fs_process_has_capability_failure() >= 0) {
        ++g_filesystem_borrowed_leases;
        return true;
      }
    }
    const int32_t status = hook.acquire(
        hook.context, static_cast<uint32_t>(Kind::Filesystem), directory_fd);
    if (status == 0)
      return true;
    *error = "Rust filesystem provider owner rejected activation: " +
             std::to_string(status);
    return false;
  }
  return acquire_filesystem_direct(directory_fd, error);
}

void release_filesystem() {
  const HookSnapshot hook = hooks();
  if (hook.release != nullptr) {
    {
      std::lock_guard<std::mutex> lock(g_filesystem_borrow_mutex);
      if (g_filesystem_borrowed_leases != 0) {
        --g_filesystem_borrowed_leases;
        return;
      }
    }
    if (hook.release(hook.context, static_cast<uint32_t>(Kind::Filesystem)) !=
        0) {
      std::abort();
    }
    return;
  }
  release_filesystem_direct();
}

bool acquire_network(std::string *error) {
  const HookSnapshot hook = hooks();
  if (hook.acquire != nullptr) {
    const int32_t status =
        hook.acquire(hook.context, static_cast<uint32_t>(Kind::Network), -1);
    if (status == 0)
      return true;
    *error = "Rust network provider owner rejected activation: " +
             std::to_string(status);
    return false;
  }
  return acquire_network_direct(error);
}

void release_network() {
  const HookSnapshot hook = hooks();
  if (hook.release != nullptr) {
    if (hook.release(hook.context, static_cast<uint32_t>(Kind::Network)) != 0) {
      std::abort();
    }
    return;
  }
  release_network_direct();
}

bool acquire_stdio(std::string *error) {
  const HookSnapshot hook = hooks();
  if (hook.acquire != nullptr) {
    const int32_t status =
        hook.acquire(hook.context, static_cast<uint32_t>(Kind::Stdio), -1);
    if (status == 0)
      return true;
    *error = "Rust stdio provider owner rejected activation: " +
             std::to_string(status);
    return false;
  }
  return acquire_stdio_direct(error);
}

void release_stdio() {
  const HookSnapshot hook = hooks();
  if (hook.release != nullptr) {
    if (hook.release(hook.context, static_cast<uint32_t>(Kind::Stdio)) != 0) {
      std::abort();
    }
    return;
  }
  release_stdio_direct();
}

bool acquire_ioctl(std::string *error) {
  const HookSnapshot hook = hooks();
  if (hook.acquire != nullptr) {
    const int32_t status =
        hook.acquire(hook.context, static_cast<uint32_t>(Kind::Ioctl), -1);
    if (status == 0)
      return true;
    *error = "Rust ioctl provider owner rejected activation: " +
             std::to_string(status);
    return false;
  }
  return acquire_ioctl_direct(error);
}

void release_ioctl() {
  const HookSnapshot hook = hooks();
  if (hook.release != nullptr) {
    if (hook.release(hook.context, static_cast<uint32_t>(Kind::Ioctl)) != 0) {
      std::abort();
    }
    return;
  }
  release_ioctl_direct();
}

bool acquire_strftime(std::string *error) {
  const HookSnapshot hook = hooks();
  if (hook.acquire != nullptr) {
    const int32_t status =
        hook.acquire(hook.context, static_cast<uint32_t>(Kind::Strftime), -1);
    if (status == 0)
      return true;
    *error = "Rust strftime provider owner rejected activation: " +
             std::to_string(status);
    return false;
  }
  return acquire_strftime_direct(error);
}

void release_strftime() {
  const HookSnapshot hook = hooks();
  if (hook.release != nullptr) {
    if (hook.release(hook.context, static_cast<uint32_t>(Kind::Strftime)) !=
        0) {
      std::abort();
    }
    return;
  }
  release_strftime_direct();
}

bool acquire_sendfile(std::string *error) {
  const HookSnapshot hook = hooks();
  if (hook.acquire != nullptr) {
    const int32_t status =
        hook.acquire(hook.context, static_cast<uint32_t>(Kind::Sendfile), -1);
    if (status == 0)
      return true;
    *error = "Rust sendfile provider owner rejected activation: " +
             std::to_string(status);
    return false;
  }
  return acquire_sendfile_direct(error);
}

void release_sendfile() {
  const HookSnapshot hook = hooks();
  if (hook.release != nullptr) {
    if (hook.release(hook.context, static_cast<uint32_t>(Kind::Sendfile)) !=
        0) {
      std::abort();
    }
    return;
  }
  release_sendfile_direct();
}

bool acquire_vm(std::string *error) {
  const HookSnapshot hook = hooks();
  if (hook.acquire != nullptr) {
    const int32_t status =
        hook.acquire(hook.context, static_cast<uint32_t>(Kind::Vm), -1);
    if (status == 0)
      return true;
    *error =
        "Rust VM provider owner rejected activation: " + std::to_string(status);
    return false;
  }
  return acquire_vm_direct(error);
}

void release_vm() {
  const HookSnapshot hook = hooks();
  if (hook.release != nullptr) {
    if (hook.release(hook.context, static_cast<uint32_t>(Kind::Vm)) != 0) {
      std::abort();
    }
    return;
  }
  release_vm_direct();
}

extern "C" void darwin_art_provider_install_hooks(void *context,
                                                  AcquireHook acquire,
                                                  ReleaseHook release) {
  std::lock_guard<std::mutex> lock(g_hooks.mutex);
  g_hooks.context = context;
  g_hooks.acquire = acquire;
  g_hooks.release = release;
}

extern "C" void darwin_art_provider_clear_hooks() {
  std::lock_guard<std::mutex> lock(g_hooks.mutex);
  g_hooks.context = nullptr;
  g_hooks.acquire = nullptr;
  g_hooks.release = nullptr;
}

extern "C" int32_t darwin_art_provider_native_acquire(uint32_t kind,
                                                      int32_t authority_fd) {
  std::string error;
  bool ok = false;
  switch (static_cast<Kind>(kind)) {
  case Kind::Filesystem:
    ok = acquire_filesystem_direct(authority_fd, &error);
    break;
  case Kind::Network:
    ok = acquire_network_direct(&error);
    break;
  case Kind::Stdio:
    ok = acquire_stdio_direct(&error);
    break;
  case Kind::Ioctl:
    ok = acquire_ioctl_direct(&error);
    break;
  case Kind::Strftime:
    ok = acquire_strftime_direct(&error);
    break;
  case Kind::Sendfile:
    ok = acquire_sendfile_direct(&error);
    break;
  case Kind::Vm:
    ok = acquire_vm_direct(&error);
    break;
  }
  return ok ? 0 : -1;
}

extern "C" int32_t darwin_art_provider_native_release(uint32_t kind) {
  switch (static_cast<Kind>(kind)) {
  case Kind::Filesystem:
    release_filesystem_direct();
    break;
  case Kind::Network:
    release_network_direct();
    break;
  case Kind::Stdio:
    release_stdio_direct();
    break;
  case Kind::Ioctl:
    release_ioctl_direct();
    break;
  case Kind::Strftime:
    release_strftime_direct();
    break;
  case Kind::Sendfile:
    release_sendfile_direct();
    break;
  case Kind::Vm:
    release_vm_direct();
    break;
  default:
    return -1;
  }
  return 0;
}

} // namespace darwin_art::providers
