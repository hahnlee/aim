// Focused SCMChannel production-TU probe.  The provider callbacks are a
// receipt-only test double; socket rights and ancillary ownership are native.
#include "../bionic-socket-broker-adapter/src/fd_inheritance.h"
#include "../bionic-socket-broker-adapter/src/scm_channel.h"

#include <algorithm>
#include <cassert>
#include <cerrno>
#include <chrono>
#include <cstring>
#include <fcntl.h>
#include <sys/socket.h>
#include <sys/time.h>
#include <unistd.h>

#include <array>

namespace {
using namespace darwin_art::bionic::scm;

struct FakeProvider {
  uint8_t authority[16]{};
  int releases = 0;
  int holder_releases = 0;
  int next_ticket = 9;
  int last_ticket = 0;
  int guardian_reader = -1;
  bool lose_finished_ack = false;
  bool authority_finished = false;
  ~FakeProvider() { if (guardian_reader >= 0) close(guardian_reader); }
};

void *Retain(void *opaque) { return opaque; }
void Release(void *opaque) { ++static_cast<FakeProvider *>(opaque)->releases; }
int ReleaseHolder(void *opaque, const uint8_t *) {
  ++static_cast<FakeProvider *>(opaque)->holder_releases;
  return 0;
}
int RegisterPair(void *opaque, const DarwinArtScmPairInstallerV1 *installer) {
  DarwinArtScmPairOfferV1 offer{};
  std::memcpy(offer.authority, static_cast<FakeProvider *>(opaque)->authority, 16);
  offer.carrier = 41;
  offer.holder_a[0] = 1;
  offer.holder_b[0] = 2;
  return installer->install(installer->target, &offer);
}
int Prepare(void *opaque, const DarwinArtScmPrepareRequestV2 *request,
            DarwinArtScmPreparedV2 *output) {
  auto *provider = static_cast<FakeProvider *>(opaque);
  if (request == nullptr || output == nullptr)
    return EINVAL;
  int guardian[2] = {-1, -1};
  if (pipe(guardian) != 0) return errno;
  char metadata_path[] = "/tmp/scm-channel-metadata-XXXXXX";
  const int metadata = mkstemp(metadata_path);
  if (metadata < 0) { close(guardian[0]); close(guardian[1]); return errno; }
  unlink(metadata_path);
  std::array<unsigned char, 1024> bytes{};
  const std::size_t body_size = 28 + request->managed_count * 24 + 12;
  if (body_size > bytes.size() - 8) { close(metadata); close(guardian[0]); close(guardian[1]); return E2BIG; }
  bytes[0] = 2; bytes[1] = 2;
  bytes[4] = static_cast<unsigned char>(body_size);
  bytes[5] = static_cast<unsigned char>(body_size >> 8);
  bytes[6] = static_cast<unsigned char>(body_size >> 16);
  bytes[7] = static_cast<unsigned char>(body_size >> 24);
  std::memcpy(bytes.data() + 8, provider->authority, 16);
  const uint16_t payload_count = static_cast<uint16_t>(request->payload_count);
  std::memcpy(bytes.data() + 32, &payload_count, 2);
  const uint16_t managed_count = static_cast<uint16_t>(request->managed_count);
  std::memcpy(bytes.data() + 34, &managed_count, 2);
  for (std::size_t i = 0; i < request->managed_count; ++i) {
    std::memcpy(bytes.data() + 36 + i * 24, &request->managed[i].ordinal, 8);
    std::memcpy(bytes.data() + 44 + i * 24, request->managed[i].holder, 16);
  }
  const std::size_t credentials = 36 + request->managed_count * 24;
  const int32_t pid = static_cast<int32_t>(getpid());
  const uint32_t uid = static_cast<uint32_t>(getuid());
  const uint32_t gid = static_cast<uint32_t>(getgid());
  std::memcpy(bytes.data() + credentials, &pid, 4);
  std::memcpy(bytes.data() + credentials + 4, &uid, 4);
  std::memcpy(bytes.data() + credentials + 8, &gid, 4);
  if (write(metadata, bytes.data(), body_size + 8) != static_cast<ssize_t>(body_size + 8)) {
    close(metadata); close(guardian[0]); close(guardian[1]); return EIO;
  }
  lseek(metadata, 0, SEEK_SET);
  std::memcpy(output->authority, provider->authority, 16);
  output->ticket = provider->next_ticket++;
  provider->last_ticket = output->ticket;
  output->metadata_fd = metadata;
  output->guardian_fd = guardian[1];
  output->payload_count = request->payload_count;
  if (provider->guardian_reader >= 0) close(provider->guardian_reader);
  provider->guardian_reader = guardian[0];
  assert(fcntl(guardian[0], F_SETFL, O_NONBLOCK) == 0);
  return 0;
}
int Admit(void *opaque, const DarwinArtScmAdmitRequestV2 *request,
          DarwinArtScmAdmissionV2 *output) {
  auto *provider = static_cast<FakeProvider *>(opaque);
  if (request == nullptr || output == nullptr || request->metadata_fd < 0)
    return EINVAL;
  std::memcpy(output->authority, provider->authority, 16);
  output->ticket = provider->last_ticket;
  output->credentials = {static_cast<int32_t>(getpid()), static_cast<uint32_t>(getuid()),
                         static_cast<uint32_t>(getgid())};
  output->claim_count = request->publish_count == 0 ? 0 : 1;
  if (output->claim_count != 0) {
    output->claims[0].ordinal = request->publish_ordinals[0];
    std::memcpy(output->claims[0].grant.authority, provider->authority, 16);
    output->claims[0].grant.carrier = 41;
    output->claims[0].grant.holder[0] = 8;
    output->claims[0].grant.side = 0;
  }
  return 0;
}
int Settle(void *opaque, const uint8_t *, uint64_t, uint32_t outcome) {
  auto &provider = *static_cast<FakeProvider *>(opaque);
  if (provider.lose_finished_ack) {
    if (outcome == DARWIN_ART_SCM_FINISHED) {
      provider.authority_finished = true;
      return EIO; // Test-only: authority commits but its ACK is interrupted.
    }
    if (provider.authority_finished) return EALREADY;
  }
  return 0;
}
int Bind(void *, const uint8_t *, const DarwinArtScmBinderBindingV2 *, uint8_t *) { return 0; }
int Cancel(void *, const DarwinArtScmBinderBindingV2 *) { return 0; }
int Claim(void *, const DarwinArtScmBinderBindingV2 *, const uint8_t *, uint32_t,
          DarwinArtScmGrantV2 *) { return 0; }

DarwinArtScmEndpointProviderV1 Table(FakeProvider *provider) {
  DarwinArtScmEndpointProviderV1 table{
      DARWIN_ART_SCM_ENDPOINT_ABI_VERSION, sizeof(DarwinArtScmEndpointProviderV1),
      provider, Retain, Release, RegisterPair, ReleaseHolder, Prepare, Admit,
      Settle, Bind, Cancel, Claim};
  return table;
}

intptr_t DirectBoundary(darwin_art::bionic::fd_inheritance::FdOperation operation,
                        void *context) { return operation(context); }

void ClosePair(int pair[2]) { close(pair[0]); close(pair[1]); }

void NativeSocketRightsRoundTrip() {
  assert(darwin_art_bionic_install_fd_inheritance_boundary(&DirectBoundary) == 0);
  FakeProvider fake;
  fake.authority[0] = 3;
  auto table = Table(&fake);
  EndpointLease sender;
  EndpointLease receiver;
  PairInstallReceipt receipt(sender, receiver, table);
  assert(receipt.Register() == 0);
  assert(receipt.Commit());

  int carrier[2] = {-1, -1};
  assert(socketpair(AF_UNIX, SOCK_STREAM, 0, carrier) == 0);
  SCMChannel output(sender, carrier[0]);
  SCMChannel input(receiver, carrier[1]);
  int payload[2] = {-1, -1};
  assert(pipe(payload) == 0);
  const char text[] = "native";
  iovec send_vector{const_cast<char *>(text), sizeof(text) - 1};
  NativeMessage send_message{&send_vector, 1, nullptr, 0, 0};
  ManagedPayload managed;
  managed.ordinal = 0;
  managed.holder[0] = 7;
  ssize_t sent = -1;
  assert(output.Send(send_message, &payload[0], 1, &managed, 1, &sent) == 0);
  assert(sent == static_cast<ssize_t>(sizeof(text) - 1));

  char received_bytes[32]{};
  iovec receive_vector{received_bytes, sizeof(received_bytes)};
  NativeMessage receive_message{&receive_vector, 1, nullptr, 0, 0};
  ReceiveOptions options;
  ReceiveResult result;
  assert(input.Receive(receive_message, options, &result) == 0);
  assert(result.private_envelope() && result.payload_count() == 1);
  assert(result.bytes() == static_cast<ssize_t>(sizeof(text) - 1));
  assert(std::memcmp(received_bytes, text, sizeof(text) - 1) == 0);
  const uint64_t publish = 0;
  assert(result.Admit(&publish, 1) == 0);
  assert(result.Finish() == 0);
  char guard_byte = 0;
  errno = 0;
  assert(read(fake.guardian_reader, &guard_byte, 1) == -1 && errno == EAGAIN);
  int imported = -1;
  assert(result.TakePayloadFd(0, &imported));
  GrantLease grant;
  assert(result.TakeGrant(0, &grant));
  assert(result.Commit());
  assert(read(fake.guardian_reader, &guard_byte, 1) == 0);
  assert(grant.Commit());
  close(imported);
  close(payload[0]);
  close(payload[1]);

  // A managed consumer with no control ordinals still performs authoritative
  // admission and settlement, then deliberately discards the complete group.
  int second_payload[2] = {-1, -1};
  assert(pipe(second_payload) == 0);
  assert(output.Send(send_message, &second_payload[0], 1, &managed, 1, &sent) == 0);
  ReceiveResult discarded;
  assert(input.Receive(receive_message, ReceiveOptions{}, &discarded) == 0);
  assert(discarded.Admit(nullptr, 0) == 0);
  assert(discarded.Finish() == 0);
  assert(discarded.DiscardPayloadFd(0));
  assert(discarded.Commit());
  close(second_payload[0]);
  close(second_payload[1]);

  int failed_payload[2] = {-1, -1};
  assert(pipe(failed_payload) == 0);
  assert(output.Send(send_message, &failed_payload[0], 1, &managed, 1, &sent) == 0);
  const int releases_before = fake.holder_releases;
  fake.lose_finished_ack = true;
  {
    ReceiveResult failed;
    assert(input.Receive(receive_message, ReceiveOptions{}, &failed) == 0);
    assert(failed.Admit(&publish, 1) == 0);
    assert(failed.Finish() == -1 && fake.authority_finished);
  }
  assert(fake.holder_releases == releases_before + 1);
  assert(read(fake.guardian_reader, &guard_byte, 1) == 0);
  close(failed_payload[0]); close(failed_payload[1]);
  ClosePair(carrier);
}

void PeekAndOrdinaryRoundTrip() {
  FakeProvider fake;
  fake.authority[0] = 4;
  auto table = Table(&fake);
  EndpointLease sender;
  EndpointLease receiver;
  PairInstallReceipt receipt(sender, receiver, table);
  assert(receipt.Register() == 0 && receipt.Commit());
  int carrier[2] = {-1, -1};
  assert(socketpair(AF_UNIX, SOCK_STREAM, 0, carrier) == 0);
  SCMChannel output(sender, carrier[0]);
  SCMChannel input(receiver, carrier[1]);
  const char text[] = "ordinary";
  iovec vector{const_cast<char *>(text), sizeof(text) - 1};
  NativeMessage message{&vector, 1, nullptr, 0, 0};
  ssize_t sent = -1;
  assert(output.Send(message, nullptr, 0, nullptr, 0, &sent) == 0);
  ReceiveResult peek;
  ReceiveOptions peek_options;
  peek_options.flags = MSG_PEEK;
  errno = 0;
  assert(input.Receive(message, peek_options, &peek) == -1 && errno == EOPNOTSUPP);
  char bytes[32]{};
  iovec receive{bytes, sizeof(bytes)};
  NativeMessage read_message{&receive, 1, nullptr, 0, 0};
  ReceiveResult ordinary;
  assert(input.Receive(read_message, ReceiveOptions{}, &ordinary) == 0);
  assert(ordinary.private_envelope());
  assert(ordinary.bytes() == static_cast<ssize_t>(sizeof(text) - 1));
  assert(ordinary.Admit(nullptr, 0) == 0);
  assert(ordinary.Finish() == 0);
  assert(ordinary.credentials() != nullptr);
  assert(ordinary.Commit());
  const char partial_text[] = "partial";
  iovec partial_send{const_cast<char *>(partial_text), sizeof(partial_text) - 1};
  assert(output.Send(NativeMessage{&partial_send, 1, nullptr, 0, 0}, nullptr, 0,
                     nullptr, 0, &sent) == 0);
  close(carrier[0]);
  char partial_bytes[32]{};
  iovec partial_receive{partial_bytes, sizeof(partial_bytes)};
  ReceiveOptions waitall;
  waitall.flags = MSG_WAITALL;
  ReceiveResult partial;
  assert(input.Receive(NativeMessage{&partial_receive, 1, nullptr, 0, 0},
                       waitall, &partial) == 0);
  assert(partial.bytes() == static_cast<ssize_t>(sizeof(partial_text) - 1));
  assert(partial.Admit(nullptr, 0) == 0);
  assert(partial.Finish() == 0);
  assert(partial.Commit());
  ClosePair(carrier);
}

void DatagramRecordAndZeroWidthTimeout() {
  FakeProvider fake;
  fake.authority[0] = 5;
  auto table = Table(&fake);
  EndpointLease sender;
  EndpointLease receiver;
  PairInstallReceipt receipt(sender, receiver, table);
  assert(receipt.Register() == 0 && receipt.Commit());

  int carrier[2] = {-1, -1};
  assert(socketpair(AF_UNIX, SOCK_DGRAM, 0, carrier) == 0);
  SCMChannel output(sender, carrier[0]);
  SCMChannel input(receiver, carrier[1]);

  ssize_t sent = -1;
  const char first_text[] = "first";
  iovec first_vector{const_cast<char *>(first_text), sizeof(first_text) - 1};
  assert(output.Send(NativeMessage{&first_vector, 1, nullptr, 0, 0}, nullptr,
                     0, nullptr, 0, &sent) == 0);
  const char second_text[] = "second";
  iovec second_vector{const_cast<char *>(second_text), sizeof(second_text) - 1};
  assert(output.Send(NativeMessage{&second_vector, 1, nullptr, 0, 0}, nullptr,
                     0, nullptr, 0, &sent) == 0);

  char received_bytes[32]{};
  iovec receive_vector{received_bytes, sizeof(received_bytes)};
  ReceiveOptions waitall;
  waitall.flags = MSG_WAITALL;
  ReceiveResult first;
  assert(input.Receive(NativeMessage{&receive_vector, 1, nullptr, 0, 0},
                       waitall, &first) == 0);
  assert(first.bytes() == static_cast<ssize_t>(sizeof(first_text) - 1));
  assert(std::memcmp(received_bytes, first_text, sizeof(first_text) - 1) == 0);
  assert((first.flags() & MSG_EOR) == 0);

  std::memset(received_bytes, 0, sizeof(received_bytes));
  ReceiveResult second;
  assert(input.Receive(NativeMessage{&receive_vector, 1, nullptr, 0, 0},
                       waitall, &second) == 0);
  assert(second.bytes() == static_cast<ssize_t>(sizeof(second_text) - 1));
  assert(std::memcmp(received_bytes, second_text, sizeof(second_text) - 1) == 0);

  int payload[2] = {-1, -1};
  assert(pipe(payload) == 0);
  ManagedPayload managed{};
  managed.ordinal = 0;
  managed.holder[0] = 7;
  char one_byte = 'z';
  iovec no_payload_vector{&one_byte, 0};
  iovec zero_vector{nullptr, 0};
  assert(output.Send(NativeMessage{&no_payload_vector, 1, nullptr, 0, 0},
                     &payload[0], 1, &managed, 1, &sent) == 0);
  close(payload[0]);
  close(payload[1]);
  ReceiveResult zero_width;
  assert(input.Receive(NativeMessage{&zero_vector, 1, nullptr, 0, 0},
                       ReceiveOptions{}, &zero_width) == 0);
  assert(zero_width.private_envelope() && zero_width.bytes() == 0 &&
         zero_width.payload_count() == 1);
  assert(zero_width.Admit(nullptr, 0) == 0);
  assert(zero_width.Finish() == 0);
  assert(zero_width.DiscardPayloadFd(0));
  assert(zero_width.Commit());
  char guardian_byte = 0;
  assert(read(fake.guardian_reader, &guardian_byte, 1) == 0);

  timeval timeout{0, 30000};
  assert(setsockopt(carrier[1], SOL_SOCKET, SO_RCVTIMEO, &timeout,
                    sizeof(timeout)) == 0);
  const auto start = std::chrono::steady_clock::now();
  errno = 0;
  ReceiveResult timed;
  assert(input.Receive(NativeMessage{&zero_vector, 1, nullptr, 0, 0},
                       ReceiveOptions{}, &timed) == -1);
  const auto elapsed = std::chrono::duration_cast<std::chrono::milliseconds>(
      std::chrono::steady_clock::now() - start);
  assert(errno == EAGAIN || errno == EWOULDBLOCK);
  assert(elapsed.count() >= 10 && elapsed.count() < 1000);
  ClosePair(carrier);
}

void StreamAncillaryPressureIsRetryable() {
  FakeProvider fake;
  fake.authority[0] = 6;
  auto table = Table(&fake);
  EndpointLease sender;
  EndpointLease receiver;
  PairInstallReceipt receipt(sender, receiver, table);
  assert(receipt.Register() == 0 && receipt.Commit());
  int carrier[2] = {-1, -1};
  assert(socketpair(AF_UNIX, SOCK_STREAM, 0, carrier) == 0);
  const int capacity = 4096;
  assert(setsockopt(carrier[0], SOL_SOCKET, SO_SNDBUF, &capacity,
                    sizeof(capacity)) == 0);
  assert(fcntl(carrier[0], F_SETFL, O_NONBLOCK) == 0);
  std::array<char, 1024> filler{};
  std::size_t filled = 0;
  for (;;) {
    const ssize_t sent = send(carrier[0], filler.data(), filler.size(), MSG_DONTWAIT);
    if (sent < 0) {
      assert(errno == EAGAIN && filled > 1 && filled <= 1 << 20);
      break;
    }
    filled += static_cast<std::size_t>(sent);
    assert(filled <= 1 << 20);
  }
  char byte = 0;
  assert(recv(carrier[1], &byte, 1, 0) == 1);
  SCMChannel output(sender, carrier[0]);
  const char marker = 'm';
  iovec vector{const_cast<char *>(&marker), 1};
  const NativeMessage message{&vector, 1, nullptr, 0, 0};
  ssize_t sent = -1;
  errno = 0;
  assert(output.Send(message, nullptr, 0, nullptr, 0, &sent) == -1);
  assert(errno == EAGAIN && sent == -1);
  std::size_t remaining = filled - 1;
  while (remaining != 0) {
    const ssize_t received = recv(carrier[1], filler.data(),
                                  std::min(remaining, filler.size()), 0);
    assert(received > 0);
    remaining -= static_cast<std::size_t>(received);
  }
  assert(output.Send(message, nullptr, 0, nullptr, 0, &sent) == 0 && sent == 1);
  SCMChannel input(receiver, carrier[1]);
  char received_byte = 0;
  iovec receive_vector{&received_byte, 1};
  ReceiveResult result;
  assert(input.Receive(NativeMessage{&receive_vector, 1, nullptr, 0, 0},
                       ReceiveOptions{}, &result) == 0);
  assert(result.private_envelope() && result.bytes() == 1 && received_byte == marker);
  assert(result.Admit(nullptr, 0) == 0 && result.Finish() == 0 && result.Commit());
  ClosePair(carrier);
}
} // namespace

int main() {
  NativeSocketRightsRoundTrip();
  PeekAndOrdinaryRoundTrip();
  DatagramRecordAndZeroWidthTimeout();
  StreamAncillaryPressureIsRetryable();
  return 0;
}
