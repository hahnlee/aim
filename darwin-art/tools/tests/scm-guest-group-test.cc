// Focused production-TU test for SCM guest descriptor publication.  The FS
// port below is a label-component mock: it consumes every host FD and invokes
// the real central-broker callback while preserving the FS contract.
#include "../bionic-central-fd-broker/include/darwin_art_bionic_fd_broker.h"
#include "../bionic-socket-broker-adapter/src/scm_guest_group.h"

#include <cassert>
#include <cerrno>
#include <fcntl.h>
#include <unistd.h>

namespace {
using darwin_art::bionic::scm::GuestDescriptorGroup;
using darwin_art::bionic::scm::PreparedGuestDescriptor;

struct FsMock {
  int calls = 0;
  int consumed = 0;
  int forced_error = 0;
};

FsMock *g_fs = nullptr;

extern "C" int darwin_art_bionic_fs_adopt_group(
    const DarwinArtFsOwnedDescriptor *entries, size_t count,
    DarwinArtFsCommitGroup commit, void *context, int *output) {
  assert(g_fs != nullptr);
  ++g_fs->calls;
  // The FS boundary consumes every input on every outcome.  It stages guest
  // numbers privately, then exposes them only after the central callback.
  for (size_t i = 0; i < count; ++i) {
    assert(entries[i].host_fd >= 0);
    assert(fcntl(entries[i].host_fd, F_GETFD) >= 0);
    close(entries[i].host_fd);
    ++g_fs->consumed;
  }
  if (g_fs->forced_error != 0) return g_fs->forced_error;
  int staged[16];
  for (size_t i = 0; i < count; ++i) staged[i] = 700 + static_cast<int>(i);
  const int status = commit == nullptr ? 0 : commit(context, staged, count);
  if (status != 0) return status;
  for (size_t i = 0; i < count; ++i) output[i] = staged[i];
  return 0;
}

struct CentralMock {
  int closes = 0;
};

int CentralClose(void *opaque, uint64_t, int *android_errno) {
  ++static_cast<CentralMock *>(opaque)->closes;
  if (android_errno != nullptr) *android_errno = 0;
  return 0;
}

void CentralCleanup(void *opaque, uint64_t) noexcept {
  ++static_cast<CentralMock *>(opaque)->closes;
}

DarwinArtFdOwnerV1 Owner(CentralMock *mock) {
  DarwinArtFdOwnerV1 owner{};
  owner.abi_version = DARWIN_ART_FD_OWNER_ABI_V1;
  owner.struct_size = offsetof(DarwinArtFdOwnerV1, read_at);
  owner.context = mock;
  owner.close = &CentralClose;
  return owner;
}

PreparedGuestDescriptor FileEntry(int fd, int flags = 0) {
  PreparedGuestDescriptor entry{};
  entry.filesystem_fd = fd;
  entry.descriptor_flags = flags;
  return entry;
}

PreparedGuestDescriptor CentralEntry(DarwinArtFdOwnerHandle owner,
                                     uint64_t object) {
  PreparedGuestDescriptor entry{};
  entry.central.owner = owner;
  entry.central.object = object;
  return entry;
}

int OpenNull() {
  const int fd = open("/dev/null", O_RDONLY | O_CLOEXEC);
  assert(fd >= 0);
  return fd;
}

void MixedOrderSuccessAndExactlyOnceCleanup() {
  FsMock fs;
  g_fs = &fs;
  CentralMock central;
  DarwinArtFdBroker *broker = darwin_art_fd_broker_create();
  assert(broker != nullptr);
  const auto callbacks = Owner(&central);
  DarwinArtFdOwnerHandle owner = 0;
  assert(darwin_art_fd_broker_install_owner(broker, DARWIN_ART_FD_FS_FILE,
                                             &callbacks, &owner) ==
         DARWIN_ART_FD_BROKER_OK);
  const int first = OpenNull();
  const int second = OpenNull();
  {
    GuestDescriptorGroup group(broker, &central, &CentralCleanup);
    assert(group.Add(FileEntry(first, DARWIN_ART_FD_CLOEXEC)));
    assert(group.Add(CentralEntry(owner, 101)));
    assert(group.Add(FileEntry(second)));
    assert(group.Publish() == 0);
    assert(group.count() == 3);
    assert(group.guest_fd(0) == 700);
    assert(group.guest_fd(1) >= 0);
    assert(group.guest_fd(2) == 701);
    assert(fs.calls == 1 && fs.consumed == 2);
    assert(fcntl(first, F_GETFD) == -1 && errno == EBADF);
    assert(fcntl(second, F_GETFD) == -1 && errno == EBADF);
    DarwinArtFdIoResult close_result{};
    assert(darwin_art_fd_broker_close(broker, group.guest_fd(1),
                                      &close_result) == DARWIN_ART_FD_BROKER_OK);
    assert(central.closes == 1);
  }
  assert(central.closes == 1);
  assert(darwin_art_fd_broker_uninstall_owner(broker, owner) ==
         DARWIN_ART_FD_BROKER_OK);
  assert(darwin_art_fd_broker_destroy(broker) == DARWIN_ART_FD_BROKER_OK);
  g_fs = nullptr;
}

void CentralOnlyAndEmptyBypassFs() {
  FsMock fs;
  g_fs = &fs;
  CentralMock central;
  DarwinArtFdBroker *broker = darwin_art_fd_broker_create();
  assert(broker != nullptr);
  const auto callbacks = Owner(&central);
  DarwinArtFdOwnerHandle owner = 0;
  assert(darwin_art_fd_broker_install_owner(broker, DARWIN_ART_FD_FS_FILE,
                                             &callbacks, &owner) ==
         DARWIN_ART_FD_BROKER_OK);
  {
    GuestDescriptorGroup group(broker, &central, &CentralCleanup);
    assert(group.Add(CentralEntry(owner, 202)));
    assert(group.Publish() == 0 && fs.calls == 0);
    DarwinArtFdIoResult close_result{};
    assert(darwin_art_fd_broker_close(broker, group.guest_fd(0),
                                      &close_result) == DARWIN_ART_FD_BROKER_OK);
  }
  {
    GuestDescriptorGroup empty(nullptr, nullptr, nullptr);
    assert(empty.Publish() == 0);
    assert(empty.count() == 0 && empty.guest_fd(0) == -1);
    assert(fs.calls == 0);
  }
  assert(central.closes == 1);
  assert(darwin_art_fd_broker_uninstall_owner(broker, owner) ==
         DARWIN_ART_FD_BROKER_OK);
  assert(darwin_art_fd_broker_destroy(broker) == DARWIN_ART_FD_BROKER_OK);
  g_fs = nullptr;
}

void CentralFailureRollsBackFsAndUnpublishedObjects() {
  FsMock fs;
  g_fs = &fs;
  CentralMock central;
  DarwinArtFdBroker *broker = darwin_art_fd_broker_create();
  assert(broker != nullptr);
  const auto callbacks = Owner(&central);
  DarwinArtFdOwnerHandle owner = 0;
  assert(darwin_art_fd_broker_install_owner(broker, DARWIN_ART_FD_FS_FILE,
                                             &callbacks, &owner) ==
         DARWIN_ART_FD_BROKER_OK);
  assert(darwin_art_fd_broker_uninstall_owner(broker, owner) ==
         DARWIN_ART_FD_BROKER_OK);
  const int file = OpenNull();
  {
    GuestDescriptorGroup group(broker, &central, &CentralCleanup);
    // Keep the exact owner/object pair from the old owner; publication must
    // fail atomically and must not expose a filesystem prefix.
    assert(group.Add(FileEntry(file)));
    PreparedGuestDescriptor stale = CentralEntry(owner, 303);
    assert(group.Add(stale));
    assert(group.Publish() == 9);
    assert(group.guest_fd(0) == -1 && group.guest_fd(1) == -1);
    assert(fs.calls == 1 && fs.consumed == 1);
    assert(fcntl(file, F_GETFD) == -1 && errno == EBADF);
  }
  assert(central.closes == 1);
  assert(darwin_art_fd_broker_destroy(broker) == DARWIN_ART_FD_BROKER_OK);
  g_fs = nullptr;
}

void FileOnlyFailureAndInvalidPartialCentralInputs() {
  FsMock fs;
  g_fs = &fs;
  const int first = OpenNull();
  const int second = OpenNull();
  fs.forced_error = 12;
  {
    GuestDescriptorGroup group(nullptr, nullptr, nullptr);
    assert(group.Add(FileEntry(first)));
    assert(group.Add(FileEntry(second)));
    assert(group.Publish() == 12);
    assert(group.guest_fd(0) == -1);
    assert(fcntl(first, F_GETFD) == -1 && errno == EBADF);
    assert(fcntl(second, F_GETFD) == -1 && errno == EBADF);
  }
  CentralMock central;
  DarwinArtFdBroker *broker = darwin_art_fd_broker_create();
  assert(broker != nullptr);
  GuestDescriptorGroup invalid(broker, &central, &CentralCleanup);
  PreparedGuestDescriptor partial = CentralEntry(0, 404);
  assert(!invalid.Add(partial));
  partial = CentralEntry(405, 0);
  assert(!invalid.Add(partial));
  assert(invalid.count() == 0);
  assert(darwin_art_fd_broker_destroy(broker) == DARWIN_ART_FD_BROKER_OK);
  g_fs = nullptr;
}
} // namespace

int main() {
  MixedOrderSuccessAndExactlyOnceCleanup();
  CentralOnlyAndEmptyBypassFs();
  CentralFailureRollsBackFsAndUnpublishedObjects();
  FileOnlyFailureAndInvalidPartialCentralInputs();
  return 0;
}
