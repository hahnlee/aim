#include "retained_scm_export.h"

#include <assert.h>
#include <cerrno>
#include <fcntl.h>
#include <memory>
#include <unistd.h>

namespace scm = darwin_art::bionic::scm;

namespace {

struct ExportState {
  bool fail_export = false;
  int exported_fd = -1;
  int captured_fd = -1;
  int operations = 0;
  int releases = 0;
};

int OwnerClose(void *, uint64_t object, int *android_errno) {
  const int result = close(static_cast<int>(object));
  *android_errno = result == 0 ? 0 : 5;
  return result;
}

int OwnerExport(void *context, uint64_t object, int *host_fd,
                int *android_errno) {
  auto *state = static_cast<ExportState *>(context);
  const int duplicate = fcntl(static_cast<int>(object), F_DUPFD_CLOEXEC, 0);
  if (duplicate < 0) {
    *android_errno = 5;
    return -1;
  }
  *host_fd = duplicate;
  state->exported_fd = duplicate;
  if (state->fail_export) {
    *android_errno = 5;
    errno = ERANGE;
    return -1;
  }
  *android_errno = 0;
  return 0;
}

intptr_t RecordThenFail(void *context, const DarwinArtFdDescriptionSnapshotV1 *,
                        int fd, int *android_errno) {
  auto *state = static_cast<ExportState *>(context);
  ++state->operations;
  state->captured_fd = fd;
  *android_errno = 22;
  errno = ERANGE;
  return -1;
}

void ReleaseExport(void *context, int fd) {
  const int saved_errno = errno;
  auto *state = static_cast<ExportState *>(context);
  ++state->releases;
  if (state->captured_fd == fd)
    state->captured_fd = -1;
  assert(close(fd) == 0);
  errno = saved_errno;
}

int UnmanagedDuplicate(int descriptor) {
  return fcntl(descriptor, F_DUPFD_CLOEXEC, 0);
}

int FailingUnmanagedExport(int) {
  errno = EBADF;
  return -1;
}

} // namespace

int main() {
  DarwinArtFdBroker *broker = darwin_art_fd_broker_create();
  assert(broker != nullptr);

  DarwinArtFdOwnerV1 callbacks{};
  ExportState state;
  callbacks.abi_version = DARWIN_ART_FD_OWNER_ABI_V7;
  callbacks.struct_size = sizeof(callbacks);
  callbacks.context = &state;
  callbacks.close = &OwnerClose;
  callbacks.export_host_fd = &OwnerExport;
  DarwinArtFdOwnerHandle owner = 0;
  assert(darwin_art_fd_broker_install_owner(broker, DARWIN_ART_FD_FS_FILE,
                                            &callbacks,
                                            &owner) == DARWIN_ART_FD_BROKER_OK);

  const int source = open("/dev/null", O_RDONLY | O_CLOEXEC);
  assert(source >= 0);
  int guest = -1;
  assert(darwin_art_fd_broker_publish(broker, owner,
                                      static_cast<uint64_t>(source),
                                      &guest) == DARWIN_ART_FD_BROKER_OK);

  DarwinArtFdDescriptionPin *failed_pin = nullptr;
  DarwinArtFdIoResult failed_result{};
  assert(darwin_art_fd_broker_retain_exported_description(
             broker, guest, &RecordThenFail, nullptr, &state, &failed_pin,
             &failed_result) == DARWIN_ART_FD_BROKER_INVALID_ARGUMENT);
  assert(state.exported_fd == -1 && state.releases == 0);

  assert(darwin_art_fd_broker_retain_exported_description(
             broker, guest, &RecordThenFail, &ReleaseExport, &state,
             &failed_pin, &failed_result) == DARWIN_ART_FD_BROKER_OK);
  assert(failed_pin == nullptr && failed_result.value == -1 &&
         failed_result.android_errno == 22);
  assert(state.operations == 1 && state.releases == 1 &&
         state.captured_fd == -1);
  assert(errno == ERANGE);
  assert(fcntl(state.exported_fd, F_GETFD) == -1 && errno == EBADF);

  state.fail_export = true;
  state.operations = 0;
  state.releases = 0;
  assert(darwin_art_fd_broker_retain_exported_description(
             broker, guest, &RecordThenFail, &ReleaseExport, &state,
             &failed_pin, &failed_result) == DARWIN_ART_FD_BROKER_OK);
  assert(failed_pin == nullptr && failed_result.value == -1 &&
         failed_result.android_errno == 5);
  assert(state.operations == 0 && state.releases == 1);
  assert(errno == ERANGE);
  assert(fcntl(state.exported_fd, F_GETFD) == -1 && errno == EBADF);
  state.fail_export = false;

  std::unique_ptr<scm::RetainedScmExport> retained;
  DarwinArtFdIoResult result{};
  assert(scm::RetainedScmExport::Create(broker, guest, &retained, &result) ==
         DARWIN_ART_FD_BROKER_OK);
  assert(retained != nullptr);
  assert(retained->host_fd() >= 0);
  assert(retained->snapshot().owner == owner);
  assert(retained->snapshot().kind == DARWIN_ART_FD_FS_FILE);
  assert(fcntl(retained->host_fd(), F_GETFD) >= 0);

  std::unique_ptr<scm::ScopedCarrierExport> managed_carrier;
  DarwinArtFdIoResult carrier_result{};
  assert(scm::ScopedCarrierExport::Create(
             broker, guest, true, &UnmanagedDuplicate, &managed_carrier,
             &carrier_result) == DARWIN_ART_FD_BROKER_OK);
  assert(managed_carrier != nullptr);
  assert(managed_carrier->snapshot() != nullptr);
  assert(managed_carrier->snapshot()->object == static_cast<uint64_t>(source));
  assert(fcntl(managed_carrier->host_fd(), F_GETFD) >= 0);
  managed_carrier.reset();

  const int unmanaged_source = open("/dev/null", O_RDONLY | O_CLOEXEC);
  assert(unmanaged_source >= 0);
  std::unique_ptr<scm::ScopedCarrierExport> unmanaged_carrier;
  carrier_result = {};
  assert(scm::ScopedCarrierExport::Create(
             broker, unmanaged_source, false, &UnmanagedDuplicate,
             &unmanaged_carrier, &carrier_result) ==
         DARWIN_ART_FD_BROKER_OK);
  assert(unmanaged_carrier != nullptr);
  assert(unmanaged_carrier->snapshot() == nullptr);
  const int owned_unmanaged_fd = unmanaged_carrier->host_fd();
  assert(owned_unmanaged_fd >= 0);
  assert(fcntl(owned_unmanaged_fd, F_GETFD) >= 0);
  unmanaged_carrier.reset();
  assert(fcntl(owned_unmanaged_fd, F_GETFD) == -1 && errno == EBADF);
  assert(close(unmanaged_source) == 0);

  unmanaged_carrier.reset();
  carrier_result = {};
  errno = EAGAIN;
  assert(scm::ScopedCarrierExport::Create(
             nullptr, 123, false, &FailingUnmanagedExport, &unmanaged_carrier,
             &carrier_result) == DARWIN_ART_FD_BROKER_OK);
  assert(unmanaged_carrier == nullptr && carrier_result.value == -1 &&
         carrier_result.android_errno == 0 && errno == EBADF);

  DarwinArtFdIoResult close_result{};
  assert(darwin_art_fd_broker_close(broker, guest, &close_result) ==
         DARWIN_ART_FD_BROKER_OK);
  // The exact Description remains pinned until the scoped export is released.
  assert(fcntl(retained->host_fd(), F_GETFD) >= 0);
  const int transport_fd = retained->take_host_fd();
  assert(transport_fd >= 0 && retained->host_fd() == -1);
  assert(retained->take_host_fd() == -1);
  // Moving the transport FD does not release source Description ownership.
  assert(fcntl(source, F_GETFD) >= 0);
  errno = ERANGE;
  retained.reset();
  assert(errno == ERANGE);
  assert(fcntl(transport_fd, F_GETFD) >= 0);

  assert(darwin_art_fd_broker_flush_deferred_closes(broker) ==
         DARWIN_ART_FD_BROKER_OK);
  assert(fcntl(source, F_GETFD) == -1 && errno == EBADF);
  assert(close(transport_fd) == 0);
  assert(darwin_art_fd_broker_uninstall_owner(broker, owner) ==
         DARWIN_ART_FD_BROKER_OK);
  assert(darwin_art_fd_broker_destroy(broker) == DARWIN_ART_FD_BROKER_OK);
  return 0;
}
