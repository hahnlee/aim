// Focused production-TU test for the Android managed-receive owner. The
// socket/SCM path and central broker are real; FS admission is an explicit
// test-only mock.
#include "../bionic-central-fd-broker/include/darwin_art_bionic_fd_broker.h"
#include "../bionic-socket-broker-adapter/src/fd_inheritance.h"
#include "../bionic-socket-broker-adapter/src/scm_android_receive.h"

#include <array>
#include <cassert>
#include <cerrno>
#include <cstring>
#include <fcntl.h>
#include <sys/socket.h>
#include <sys/stat.h>
#include <unistd.h>

namespace {
using namespace darwin_art::bionic::scm;
namespace android = darwin_art::bionic::scm::android;

struct FsMock {
  bool fail = false;
  int calls = 0;
  int consumed = 0;
};
FsMock *g_fs = nullptr;

extern "C" int darwin_art_bionic_fs_adopt_group(
    const DarwinArtFsOwnedDescriptor *entries, size_t count,
    DarwinArtFsCommitGroup commit, void *context, int *output) {
  assert(g_fs != nullptr);
  ++g_fs->calls;
  for (size_t i = 0; i < count; ++i) {
    assert(entries[i].host_fd >= 0);
    assert(fcntl(entries[i].host_fd, F_GETFD) >= 0);
    close(entries[i].host_fd);
    ++g_fs->consumed;
  }
  if (g_fs->fail) return 5;
  int staged[16]{};
  for (size_t i = 0; i < count; ++i) staged[i] = 900 + static_cast<int>(i);
  const int status = commit == nullptr ? 0 : commit(context, staged, count);
  if (status != 0) return status;
  for (size_t i = 0; i < count; ++i) output[i] = staged[i];
  return 0;
}

struct CentralState {
  int closes = 0;
};

int CloseObject(void *opaque, uint64_t object, int *android_errno) {
  auto *state = static_cast<CentralState *>(opaque);
  ++state->closes;
  const int result = close(static_cast<int>(object));
  if (android_errno != nullptr) *android_errno = result == 0 ? 0 : 5;
  return result;
}

DarwinArtFdOwnerV1 Owner(CentralState *state) {
  DarwinArtFdOwnerV1 owner{};
  owner.abi_version = DARWIN_ART_FD_OWNER_ABI_V1;
  owner.struct_size = offsetof(DarwinArtFdOwnerV1, read_at);
  owner.context = state;
  owner.close = &CloseObject;
  return owner;
}

struct Provider {
  uint8_t authority[16]{};
  int next_ticket = 41;
  uint64_t last_ticket = 0;
  int holder_releases = 0;
  int releases = 0;
  DarwinArtScmEndpointProviderV1 table{};
};

void *Retain(void *opaque) { return opaque; }
void Release(void *opaque) { ++static_cast<Provider *>(opaque)->releases; }
int ReleaseHolder(void *opaque, const uint8_t *) {
  ++static_cast<Provider *>(opaque)->holder_releases;
  return 0;
}

int RegisterPair(void *opaque, const DarwinArtScmPairInstallerV1 *installer) {
  auto *provider = static_cast<Provider *>(opaque);
  DarwinArtScmPairOfferV1 offer{};
  std::memcpy(offer.authority, provider->authority, 16);
  offer.carrier = 700;
  offer.holder_a[0] = 1;
  offer.holder_b[0] = 2;
  return installer->install(installer->target, &offer);
}

int Prepare(void *opaque, const DarwinArtScmPrepareRequestV2 *request,
            DarwinArtScmPreparedV2 *output) {
  auto *provider = static_cast<Provider *>(opaque);
  if (request == nullptr || output == nullptr)
    return EINVAL;
  int guardian[2] = {-1, -1};
  if (pipe(guardian) != 0) return errno;
  char metadata_path[] = "/tmp/scm-android-metadata-XXXXXX";
  const int metadata = mkstemp(metadata_path);
  if (metadata < 0) {
    close(guardian[0]);
    close(guardian[1]);
    return errno;
  }
  unlink(metadata_path);
  std::array<unsigned char, 1024> bytes{};
  const std::size_t body_size = 28 + request->managed_count * 24 + 12;
  if (body_size > bytes.size() - 8) {
    close(metadata);
    close(guardian[0]);
    close(guardian[1]);
    return E2BIG;
  }
  bytes[0] = 2;
  bytes[1] = 2;
  bytes[4] = static_cast<unsigned char>(body_size);
  bytes[5] = static_cast<unsigned char>(body_size >> 8);
  std::memcpy(bytes.data() + 8, provider->authority, 16);
  const uint16_t payload_count = static_cast<uint16_t>(request->payload_count);
  const uint16_t managed_count = static_cast<uint16_t>(request->managed_count);
  std::memcpy(bytes.data() + 32, &payload_count, 2);
  std::memcpy(bytes.data() + 34, &managed_count, 2);
  for (size_t i = 0; i < request->managed_count; ++i) {
    std::memcpy(bytes.data() + 36 + i * 24, &request->managed[i].ordinal, 8);
    std::memcpy(bytes.data() + 44 + i * 24, request->managed[i].holder, 16);
  }
  const size_t credentials = 36 + request->managed_count * 24;
  const int32_t pid = static_cast<int32_t>(getpid());
  const uint32_t uid = static_cast<uint32_t>(getuid());
  const uint32_t gid = static_cast<uint32_t>(getgid());
  std::memcpy(bytes.data() + credentials, &pid, 4);
  std::memcpy(bytes.data() + credentials + 4, &uid, 4);
  std::memcpy(bytes.data() + credentials + 8, &gid, 4);
  if (write(metadata, bytes.data(), body_size + 8) != static_cast<ssize_t>(body_size + 8)) {
    close(metadata);
    close(guardian[0]);
    close(guardian[1]);
    return EIO;
  }
  lseek(metadata, 0, SEEK_SET);
  std::memcpy(output->authority, provider->authority, 16);
  output->ticket = provider->next_ticket++;
  provider->last_ticket = output->ticket;
  output->metadata_fd = metadata;
  output->guardian_fd = guardian[1];
  output->payload_count = request->payload_count;
  close(guardian[0]);
  return 0;
}

int Admit(void *opaque, const DarwinArtScmAdmitRequestV2 *request,
          DarwinArtScmAdmissionV2 *output) {
  auto *provider = static_cast<Provider *>(opaque);
  if (request == nullptr || output == nullptr || request->metadata_fd < 0)
    return EINVAL;
  std::memcpy(output->authority, provider->authority, 16);
  output->ticket = provider->last_ticket;
  output->credentials = {static_cast<int32_t>(getpid()), static_cast<uint32_t>(getuid()),
                         static_cast<uint32_t>(getgid())};
  output->claim_count = static_cast<uint32_t>(request->publish_count);
  for (size_t i = 0; i < request->publish_count; ++i) {
    output->claims[i].ordinal = request->publish_ordinals[i];
    std::memcpy(output->claims[i].grant.authority, provider->authority, 16);
    output->claims[i].grant.carrier = 700;
    output->claims[i].grant.holder[0] = static_cast<uint8_t>(10 + i);
    output->claims[i].grant.side = 0;
  }
  return 0;
}

int Settle(void *, const uint8_t *, uint64_t, uint32_t) { return 0; }
int Bind(void *, const uint8_t *, const DarwinArtScmBinderBindingV2 *, uint8_t *) {
  return 0;
}
int Cancel(void *, const DarwinArtScmBinderBindingV2 *) { return 0; }
int Claim(void *, const DarwinArtScmBinderBindingV2 *, const uint8_t *, uint32_t,
          DarwinArtScmGrantV2 *) {
  return 0;
}

DarwinArtScmEndpointProviderV1 Table(Provider *provider) {
  return {DARWIN_ART_SCM_ENDPOINT_ABI_VERSION,
          sizeof(DarwinArtScmEndpointProviderV1), provider,
          Retain, Release, RegisterPair, ReleaseHolder, Prepare, Admit, Settle,
          Bind, Cancel, Claim};
}

struct ReceiveState {
  DarwinArtFdBroker *broker = nullptr;
  DarwinArtFdOwnerHandle owner = 0;
  Provider *provider = nullptr;
  std::array<EndpointLease, 16> adopted{};
  size_t adopted_count = 0;
  int fail_after = -1;
};

int PreparePayload(void *opaque, int native, GrantLease *grant,
                   int descriptor_flags, PreparedGuestDescriptor *prepared) {
  auto *state = static_cast<ReceiveState *>(opaque);
  if (state->fail_after == 0) {
    close(native);
    return 12;
  }
  if (state->fail_after > 0) --state->fail_after;
  *prepared = {};
  prepared->filesystem_fd = -1;
  prepared->descriptor_flags = descriptor_flags;
  struct stat status{};
  assert(fstat(native, &status) == 0);
  if (S_ISREG(status.st_mode) || S_ISCHR(status.st_mode)) {
    prepared->filesystem_fd = native;
    return 0;
  }
  if (grant != nullptr && grant->valid()) {
    assert(state->adopted_count < state->adopted.size());
    if (state->adopted[state->adopted_count].AdoptConfirmedGrant(
            state->provider->table, *grant->grant()) != 0) {
      close(native);
      return 5;
    }
    ++state->adopted_count;
  }
  prepared->central = {state->owner, static_cast<uint64_t>(native), 0,
                       static_cast<int32_t>(descriptor_flags)};
  return 0;
}

void ReleaseCentral(void *, uint64_t object) noexcept { close(static_cast<int>(object)); }

int AndroidErrno(int error) { return error == EOPNOTSUPP ? 95 : error; }

bool DecodeAddress(void *, const sockaddr *, socklen_t, void *, uint32_t,
                   uint32_t *length) {
  if (length != nullptr) *length = 0;
  return false;
}

intptr_t Boundary(darwin_art::bionic::fd_inheritance::FdOperation operation,
                  void *context) {
  return operation(context);
}

struct Fixture {
  Provider provider;
  CentralState central;
  ReceiveState receive;
  DarwinArtFdBroker *broker = nullptr;
  EndpointLease sender;
  EndpointLease receiver;
  int carrier[2] = {-1, -1};

  Fixture() {
    provider.authority[0] = 3;
    provider.table = Table(&provider);
    broker = darwin_art_fd_broker_create();
    assert(broker != nullptr);
    DarwinArtFdOwnerV1 owner = Owner(&central);
    assert(darwin_art_fd_broker_install_owner(
               broker, DARWIN_ART_FD_SOCKET, &owner, &receive.owner) ==
           DARWIN_ART_FD_BROKER_OK);
    receive.broker = broker;
    receive.provider = &provider;
    PairInstallReceipt receipt(sender, receiver, provider.table);
    assert(receipt.Register() == 0 && receipt.Commit());
    assert(socketpair(AF_UNIX, SOCK_DGRAM, 0, carrier) == 0);
  }

  ~Fixture() {
    close(carrier[0]);
    close(carrier[1]);
    assert(darwin_art_fd_broker_uninstall_owner(broker, receive.owner) ==
           DARWIN_ART_FD_BROKER_OK);
    assert(darwin_art_fd_broker_destroy(broker) == DARWIN_ART_FD_BROKER_OK);
  }
};

void Receive(Fixture *fixture, std::size_t control_capacity,
             std::array<uint8_t, 64> *control, android::ReceiveOutput *output,
             ssize_t *bytes, int flags = 0, bool pass_credentials = false) {
  SCMChannel input(fixture->receiver, fixture->carrier[1]);
  char payload_byte = 0;
  iovec vector{&payload_byte, sizeof(payload_byte)};
  NativeMessage message{&vector, 1, nullptr, 0, 0};
  ReceiveOptions options{};
  options.flags = flags;
  GuestDescriptorGroup group(fixture->broker, fixture, &ReleaseCentral);
  android::VerifiedPeerCredentials credentials{};
  android::ReceiveRequest request{
      {}, {control->data(), control_capacity}, 0,
      pass_credentials ? &credentials : nullptr};
  android::Callbacks callbacks{&fixture->receive, &PreparePayload,
                               &ReleaseCentral, &DecodeAddress, &AndroidErrno};
  assert(android::ReceiveManagedMessage(input, message, options, group, request,
                                        callbacks, output, bytes) == 0);
}

void Send(Fixture *fixture, const int *fds, size_t count,
          const ManagedPayload *managed, size_t managed_count) {
  SCMChannel output(fixture->sender, fixture->carrier[0]);
  const char byte = 'x';
  iovec vector{const_cast<char *>(&byte), 1};
  NativeMessage message{&vector, 1, nullptr, 0, 0};
  ssize_t sent = -1;
  assert(output.Send(message, fds, count, managed, managed_count, &sent) == 0);
  assert(sent == 1);
}

void PartialAndNoControl() {
  Fixture fixture;
  FsMock fs;
  g_fs = &fs;
  int pipe_fds[2] = {-1, -1};
  assert(pipe(pipe_fds) == 0);
  const int file = open("/dev/null", O_RDONLY | O_CLOEXEC);
  assert(file >= 0);
  const int payloads[] = {pipe_fds[0], file};
  ManagedPayload managed{};
  managed.ordinal = 0;
  managed.holder[0] = 7;
  Send(&fixture, payloads, 2, &managed, 1);
  close(pipe_fds[0]);
  close(file);
  std::array<uint8_t, 64> control{};
  android::ReceiveOutput output{};
  ssize_t bytes = -1;
  Receive(&fixture, 20, &control, &output, &bytes, MSG_DONTWAIT);
  assert(bytes == 1 && output.control_length == 20 && (output.flags & 8) != 0 &&
         (output.flags & 0x80) == 0);
  const int guest = *reinterpret_cast<const int *>(control.data() + 16);
  assert(guest >= 0);
  DarwinArtFdIoResult close_result{};
  assert(darwin_art_fd_broker_close(fixture.broker, guest, &close_result) ==
         DARWIN_ART_FD_BROKER_OK);

  int no_control[2] = {-1, -1};
  assert(pipe(no_control) == 0);
  Send(&fixture, &no_control[0], 1, &managed, 1);
  close(no_control[0]);
  close(no_control[1]);
  output = {};
  Receive(&fixture, 0, &control, &output, &bytes);
  assert(output.control_length == 0 && (output.flags & 8) != 0);
  g_fs = nullptr;
}

void MixedFsAndCentralPublication() {
  Fixture fixture;
  FsMock fs;
  g_fs = &fs;
  int pipe_fds[2] = {-1, -1};
  assert(pipe(pipe_fds) == 0);
  const int file = open("/dev/null", O_RDONLY | O_CLOEXEC);
  assert(file >= 0);
  const int payloads[] = {pipe_fds[0], file};
  ManagedPayload managed{};
  managed.ordinal = 0;
  managed.holder[0] = 8;
  Send(&fixture, payloads, 2, &managed, 1);
  close(pipe_fds[0]);
  close(file);
  std::array<uint8_t, 64> control{};
  android::ReceiveOutput output{};
  ssize_t bytes = -1;
  Receive(&fixture, 24, &control, &output, &bytes);
  assert(bytes == 1 && output.control_length == 24 && output.flags == 0);
  const int guest = *reinterpret_cast<const int *>(control.data() + 16);
  assert(guest >= 0 && fs.calls == 1 && fs.consumed == 1);
  DarwinArtFdIoResult close_result{};
  assert(darwin_art_fd_broker_close(fixture.broker, guest, &close_result) ==
         DARWIN_ART_FD_BROKER_OK);
  g_fs = nullptr;
}

void PassCredentialsAfterAdmission() {
  Fixture fixture;
  Send(&fixture, nullptr, 0, nullptr, 0);
  std::array<uint8_t, 64> control{};
  android::ReceiveOutput output{};
  ssize_t bytes = -1;
  Receive(&fixture, control.size(), &control, &output, &bytes, 0, true);
  assert(bytes == 1 && output.flags == 0 && output.control_length == 32);
  int32_t pid = 0;
  std::memcpy(&pid, control.data() + 16, sizeof(pid));
  assert(pid == getpid());
}

void FactoryAndPublicationFailureLeaveNoOutput() {
  Fixture fixture;
  FsMock fs;
  g_fs = &fs;
  int first[2] = {-1, -1};
  int second[2] = {-1, -1};
  assert(pipe(first) == 0 && pipe(second) == 0);
  const int payloads[] = {first[0], second[0]};
  ManagedPayload managed[2]{};
  managed[0].ordinal = 0;
  managed[0].holder[0] = 1;
  managed[1].ordinal = 1;
  managed[1].holder[0] = 2;
  Send(&fixture, payloads, 2, managed, 2);
  close(first[0]);
  close(first[1]);
  close(second[0]);
  close(second[1]);
  fixture.receive.fail_after = 1;
  std::array<uint8_t, 64> control{};
  android::ReceiveOutput output{99, 99, 99};
  ssize_t bytes = 99;
  bool failed = false;
  {
    SCMChannel input(fixture.receiver, fixture.carrier[1]);
    iovec vector{nullptr, 0};
    GuestDescriptorGroup group(fixture.broker, &fixture, &ReleaseCentral);
    NativeMessage message{&vector, 1, nullptr, 0, 0};
    android::ReceiveRequest request{{}, {control.data(), 24}, 0, nullptr};
    android::Callbacks callbacks{&fixture.receive, &PreparePayload,
                                 &ReleaseCentral, &DecodeAddress, &AndroidErrno};
    failed = android::ReceiveManagedMessage(input, message, ReceiveOptions{},
                                             group, request, callbacks, &output,
                                             &bytes) == 12;
  }
  assert(failed && output.control_length == 99 && bytes == 99);
  g_fs = nullptr;

  // The same mixed group reaches the real FS boundary, which consumes its
  // file before reporting failure; central publication remains unpublished.
  Fixture rollback;
  fs = {};
  g_fs = &fs;
  int pipe_fds[2] = {-1, -1};
  assert(pipe(pipe_fds) == 0);
  const int file = open("/dev/null", O_RDONLY | O_CLOEXEC);
  assert(file >= 0);
  const int mixed[] = {pipe_fds[0], file};
  Send(&rollback, mixed, 2, &managed[0], 1);
  close(pipe_fds[0]);
  close(pipe_fds[1]);
  close(file);
  fs.fail = true;
  output = {77, 77, 77};
  bytes = 77;
  bool rolled_back = false;
  {
    SCMChannel input(rollback.receiver, rollback.carrier[1]);
    iovec vector{nullptr, 0};
    GuestDescriptorGroup group(rollback.broker, &rollback, &ReleaseCentral);
    NativeMessage message{&vector, 1, nullptr, 0, 0};
    android::ReceiveRequest request{{}, {control.data(), 24}, 0, nullptr};
    android::Callbacks callbacks{&rollback.receive, &PreparePayload,
                                 &ReleaseCentral, &DecodeAddress, &AndroidErrno};
    rolled_back = android::ReceiveManagedMessage(
                      input, message, ReceiveOptions{}, group, request, callbacks,
                      &output, &bytes) == 5;
  }
  assert(rolled_back && output.control_length == 77 && bytes == 77 && fs.calls == 1);
  g_fs = nullptr;
}

}  // namespace

int main() {
  assert(darwin_art_bionic_install_fd_inheritance_boundary(&Boundary) == 0);
  PartialAndNoControl();
  MixedFsAndCentralPublication();
  PassCredentialsAfterAdmission();
  FactoryAndPublicationFailureLeaveNoOutput();
  return 0;
}
