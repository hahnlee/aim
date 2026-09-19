#include "binder-retained-provider-fixture.h"
#include "binder/fd_transport.h"
#include "darwin_art_bionic_socket_broker.h"
#include "scm_endpoint_provider.h"
#include <cerrno>
#include <atomic>
#include <chrono>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <fcntl.h>
#include <initializer_list>
#include <poll.h>
#include <sys/socket.h>
#include <thread>
#include <unistd.h>

namespace {
void Check(bool value, const char *message);

struct ProviderFixture {
  uint8_t authority[16]{};
  std::atomic<int> references{1};
  std::atomic<int> bind_calls{0};
  std::atomic<int> cancel_calls{0};
  std::atomic<int> release_holder_calls{0};
  bool fail_bind = false;
};

void *ProviderRetain(void *opaque) {
  auto *fixture = static_cast<ProviderFixture *>(opaque);
  ++fixture->references;
  return fixture;
}
void ProviderRelease(void *opaque) {
  auto *fixture = static_cast<ProviderFixture *>(opaque);
  Check(fixture->references > 0, "provider reference underflow");
  --fixture->references;
}
int ProviderRegisterPair(void *, const DarwinArtScmPairInstallerV1 *installer) {
  if (installer == nullptr || installer->install == nullptr) return EINVAL;
  DarwinArtScmPairOfferV1 offer{};
  offer.authority[0] = 0xa1;
  offer.carrier = 0x55;
  offer.holder_a[0] = 0x11;
  offer.holder_b[0] = 0x22;
  return installer->install(installer->target, &offer);
}
int ProviderReleaseHolder(void *opaque, const uint8_t *) {
  ++static_cast<ProviderFixture *>(opaque)->release_holder_calls;
  return 0;
}
int ProviderPrepare(void *, const DarwinArtScmPrepareRequestV2 *,
                    DarwinArtScmPreparedV2 *) { return ENOSYS; }
int ProviderAdmit(void *, const DarwinArtScmAdmitRequestV2 *,
                  DarwinArtScmAdmissionV2 *) { return ENOSYS; }
int ProviderSettle(void *, const uint8_t *, uint64_t, uint32_t) { return 0; }
int ProviderBind(void *opaque, const uint8_t *,
                 const DarwinArtScmBinderBindingV2 *, uint8_t *attributes) {
  auto *fixture = static_cast<ProviderFixture *>(opaque);
  ++fixture->bind_calls;
  if (attributes != nullptr) std::memset(attributes, 0, 40);
  if (fixture->fail_bind) return EIO;
  if (attributes == nullptr) return EINVAL;
  attributes[0] = 1;
  attributes[4] = 1;
  attributes[24] = 1;
  std::memcpy(attributes + 8, fixture->authority, 16);
  return 0;
}
int ProviderCancel(void *opaque, const DarwinArtScmBinderBindingV2 *) {
  ++static_cast<ProviderFixture *>(opaque)->cancel_calls;
  return 0;
}
int ProviderClaim(void *, const DarwinArtScmBinderBindingV2 *, const uint8_t *,
                  uint32_t, DarwinArtScmGrantV2 *) { return ENOSYS; }

DarwinArtScmEndpointProviderV1 ProviderTable(ProviderFixture *fixture) {
  DarwinArtScmEndpointProviderV1 table{
      DARWIN_ART_SCM_ENDPOINT_ABI_VERSION,
      sizeof(DarwinArtScmEndpointProviderV1), fixture, &ProviderRetain,
      &ProviderRelease, &ProviderRegisterPair, &ProviderReleaseHolder,
      &ProviderPrepare, &ProviderAdmit, &ProviderSettle, &ProviderBind,
      &ProviderCancel, &ProviderClaim};
  return table;
}

void Check(bool value, const char *message) {
  if (!value) {
    std::fprintf(stderr, "retained Binder provider FAIL: %s\n", message);
    std::abort();
  }
}
void Exchange(int sender, int receiver) {
  const char value = 'x';
  Check(::send(sender, &value, 1, 0) == 1, "native send");
  pollfd ready{receiver, POLLIN, 0};
  Check(::poll(&ready, 1, 1000) == 1, "native payload readiness");
  char received = 0;
  Check(::recv(receiver, &received, 1, 0) == 1 && received == value,
        "positive exchange");
}
}

void TestRetainedBinderProvider() {
  Check(darwin_art_bionic_socket_broker_activate() == 0, "activate");
  ProviderFixture provider_fixture;
  provider_fixture.authority[0] = 0xa1;
  const auto provider = ProviderTable(&provider_fixture);
  Check(darwin_art_bionic_install_scm_endpoint_provider(&provider) == 0,
        "install provider fixture");
  int32_t original[2]{-1, -1};
  Check(darwin_art_bionic_socket_broker_socketpair(1, 1, 0, original) == 0,
        "original pair");
  DarwinArtBinderTransferBinding binding{4, 7, 0, 24};
  DarwinArtBinderRetainedExportedDescriptor output{};
  Check(darwin_art_binder_export_retained_file_descriptor(original[0], &binding,
                                                         &output) == 0,
        "actual retained provider export");
  Check(output.host_fd >= 0 && output.lease != nullptr &&
            output.attributes_length == 40 && output.attributes[0] == 1,
        "managed provider output");
  DarwinArtBinderRetainedExportedDescriptor peer_output{};
  DarwinArtBinderTransferBinding peer_binding{4, 7, 1, 56};
  Check(darwin_art_binder_export_retained_file_descriptor(original[1],
                                                         &peer_binding,
                                                         &peer_output) == 0,
        "managed peer retained export");
  Check(peer_output.host_fd >= 0 && peer_output.lease != nullptr &&
            peer_output.attributes_length == 40,
        "managed peer attributes");
  Check(darwin_art_bionic_fd_export_for_scm(original[1]) == -1,
        "bare managed export rejected");
  for (const int fd : {original[0], original[1]})
    Check(darwin_art_bionic_socket_broker_close(fd) == 0, "close guest alias");
  Check(darwin_art_bionic_socket_broker_live_objects() == 2,
        "exact Descriptions survive guest close");
  Exchange(output.host_fd, peer_output.host_fd);
  errno = ERANGE;
  darwin_art_binder_release_export_lease(output.lease);
  output.lease = nullptr;
  Check(errno == ERANGE, "release preserves host errno");
  Check(close(output.host_fd) == 0, "close left host export");
  output.host_fd = -1;
  const auto deadline = std::chrono::steady_clock::now() + std::chrono::seconds(1);
  while (darwin_art_bionic_socket_broker_live_objects() != 1) {
    Check(std::chrono::steady_clock::now() < deadline,
          "released first Description did not drain");
    std::this_thread::yield();
  }
  darwin_art_binder_release_export_lease(peer_output.lease);
  peer_output.lease = nullptr;
  Check(close(peer_output.host_fd) == 0, "close peer host export");
  peer_output.host_fd = -1;
  const auto empty_deadline = std::chrono::steady_clock::now() + std::chrono::seconds(1);
  while (darwin_art_bionic_socket_broker_live_objects() != 0) {
    Check(std::chrono::steady_clock::now() < empty_deadline,
          "released peer Description did not drain");
    std::this_thread::yield();
  }

  int32_t ordinary_pipe[2]{-1, -1};
  Check(darwin_art_bionic_socket_broker_pipe(ordinary_pipe) == 0,
        "ordinary pipe");
  DarwinArtBinderRetainedExportedDescriptor ordinary{};
  Check(darwin_art_binder_export_retained_file_descriptor(
            ordinary_pipe[0], &binding, &ordinary) == 0,
        "ordinary retained export");
  Check(ordinary.host_fd >= 0 && ordinary.lease != nullptr &&
            ordinary.attributes_length == 0 && provider_fixture.bind_calls == 2,
        "ordinary zero-attribute behavior");
  Check(darwin_art_bionic_socket_broker_close(ordinary_pipe[0]) == 0 &&
            darwin_art_bionic_socket_broker_close(ordinary_pipe[1]) == 0,
        "close ordinary pipe");
  darwin_art_binder_release_export_lease(ordinary.lease);
  ordinary.lease = nullptr;
  Check(close(ordinary.host_fd) == 0, "close ordinary host export");

  provider_fixture.fail_bind = true;
  const int cancels_before_failure = provider_fixture.cancel_calls.load();
  int32_t failed_pair[2]{-1, -1};
  Check(darwin_art_bionic_socket_broker_socketpair(1, 1, 0, failed_pair) == 0,
        "failure pair");
  DarwinArtBinderRetainedExportedDescriptor failed{};
  Check(darwin_art_binder_export_retained_file_descriptor(
            failed_pair[0], &binding, &failed) == -1 && failed.host_fd == -1 &&
            failed.lease == nullptr && provider_fixture.cancel_calls == cancels_before_failure + 1,
        "bind failure cancels pending receipt");
  Check(darwin_art_bionic_socket_broker_close(failed_pair[0]) == 0 &&
            darwin_art_bionic_socket_broker_close(failed_pair[1]) == 0,
        "close failed pair");
  provider_fixture.fail_bind = false;
  const auto failed_deadline = std::chrono::steady_clock::now() + std::chrono::seconds(1);
  while (darwin_art_bionic_socket_broker_live_objects() != 0) {
    Check(std::chrono::steady_clock::now() < failed_deadline, "failed pair cleanup did not drain");
    std::this_thread::yield();
  }
  Check(darwin_art_bionic_uninstall_scm_endpoint_provider() == 0,
        "uninstall provider fixture");
  Check(provider_fixture.references == 1, "provider context fully released");
  Check(darwin_art_bionic_socket_broker_deactivate() == 0, "quiescent teardown");
}
