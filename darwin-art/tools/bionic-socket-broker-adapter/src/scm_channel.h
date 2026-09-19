#ifndef DARWIN_ART_BIONIC_SCM_CHANNEL_H_
#define DARWIN_ART_BIONIC_SCM_CHANNEL_H_

#include "ancillary_intake.h"
#include "scm_endpoint_lease.h"

#include <array>
#include <cstddef>
#include <cstdint>
#include <ctime>
#include <sys/socket.h>
#include <sys/types.h>
#include <sys/uio.h>

namespace darwin_art::bionic::scm {

// The native socket is borrowed.  The EndpointLease and its exact Description
// must remain installed until every operation and every ReceiveResult is gone.
struct NativeMessage {
  const struct iovec *vectors = nullptr;
  std::size_t vector_count = 0;
  const void *name = nullptr;
  socklen_t name_length = 0;
  int flags = 0;
};

struct ManagedPayload {
  uint64_t ordinal = 0;
  std::array<uint8_t, 16> holder{};
};

enum class ReceiveBlocking : uint8_t { kNonBlocking, kWait };

struct ReceiveOptions {
  ReceiveBlocking blocking = ReceiveBlocking::kWait;
  int flags = 0;
  // Optional absolute CLOCK_MONOTONIC deadline.  The pointed-to timespec is
  // borrowed only for this call; a null deadline means wait indefinitely.
  const timespec *deadline = nullptr;
};

// Authenticated by the daemon at Prepare and carried in the private metadata
// envelope.  A receive result exposes these only after successful Admit.
struct ScmCredentials {
  int32_t process_id = 0;
  uint32_t user_id = 0;
  uint32_t group_id = 0;
};

class GrantLease final {
public:
  GrantLease() noexcept = default;
  GrantLease(const GrantLease &) = delete;
  GrantLease &operator=(const GrantLease &) = delete;
  GrantLease(GrantLease &&other) noexcept;
  GrantLease &operator=(GrantLease &&other) noexcept;
  ~GrantLease() noexcept;

  bool valid() const noexcept { return owned_; }
  const DarwinArtScmGrantV2 *grant() const noexcept {
    return owned_ ? &grant_ : nullptr;
  }

  // The caller invokes this only after the corresponding guest publication is
  // complete.  Until then destruction releases the fresh holder exactly once.
  bool Commit() noexcept;

  // Transfer fresh-holder cleanup to an unpublished EndpointLease.  The
  // endpoint retains the provider context and owns rollback on success; a
  // failed adoption leaves this lease unchanged.
  bool AdoptInto(EndpointLease &endpoint) noexcept;

private:
  friend class ReceiveResult;
  void Reset() noexcept;
  DarwinArtScmEndpointProviderV1 provider_{};
  DarwinArtScmGrantV2 grant_{};
  bool owned_ = false;
};

class ReceiveResult final {
public:
  ReceiveResult() noexcept = default;
  ReceiveResult(const ReceiveResult &) = delete;
  ReceiveResult &operator=(const ReceiveResult &) = delete;
  ReceiveResult(ReceiveResult &&other) noexcept;
  ReceiveResult &operator=(ReceiveResult &&other) noexcept;
  ~ReceiveResult() noexcept;

  bool valid() const noexcept { return valid_; }
  bool private_envelope() const noexcept { return private_envelope_; }
  ssize_t bytes() const noexcept { return bytes_; }
  int flags() const noexcept { return flags_; }
  socklen_t name_length() const noexcept { return name_length_; }
  std::size_t payload_count() const noexcept { return payload_count_; }
  std::size_t claim_count() const noexcept { return claim_count_; }
  uint64_t claim_ordinal(std::size_t index) const noexcept {
    return index < claim_count_ ? claims_[index].ordinal : UINT64_MAX;
  }
  bool finished() const noexcept { return finished_; }
  const ScmCredentials *credentials() const noexcept {
    return credentials_valid_ ? &credentials_ : nullptr;
  }

  // Admit may include ordinary payload ordinals.  The daemon filters them
  // against its managed manifest and returns claims only for managed entries.
  int Admit(const uint64_t *publish_ordinals, std::size_t publish_count) noexcept;
  int Finish() noexcept;

  // These handoffs are valid only after Finish.  Every payload and every claim
  // must be handed off before Commit, which leaves cleanup ownership with the
  // caller's Raw FD / GrantLease objects.
  bool TakePayloadFd(std::size_t index, int *descriptor) noexcept;
  bool DiscardPayloadFd(std::size_t index) noexcept;
  bool TakeGrant(std::size_t index, GrantLease *lease) noexcept;
  bool ReadyToCommit() const noexcept;
  bool Commit() noexcept;

private:
  friend class SCMChannel;
  class NativeFd final {
  public:
    NativeFd() noexcept = default;
    explicit NativeFd(int descriptor) noexcept : descriptor_(descriptor) {}
    NativeFd(const NativeFd &) = delete;
    NativeFd &operator=(const NativeFd &) = delete;
    NativeFd(NativeFd &&other) noexcept : descriptor_(other.Release()) {}
    NativeFd &operator=(NativeFd &&other) noexcept {
      if (this != &other) { Reset(); descriptor_ = other.Release(); }
      return *this;
    }
    ~NativeFd() noexcept { Reset(); }
    int get() const noexcept { return descriptor_; }
    int Release() noexcept { int result = descriptor_; descriptor_ = -1; return result; }
    void Reset(int descriptor = -1) noexcept;
  private:
    int descriptor_ = -1;
  };

  void Reset() noexcept;
  void AbortAndRelease() noexcept;
  bool AllPayloadsHandedOff() const noexcept;

  DarwinArtScmEndpointProviderV1 provider_{};
  std::array<uint8_t, 16> authority_{};
  std::array<uint8_t, 16> carrier_holder_{};
  uint64_t ticket_ = 0;
  std::size_t payload_count_ = 0;
  std::size_t claim_count_ = 0;
  std::array<DarwinArtScmClaimV2, DARWIN_ART_SCM_MAX_PAYLOADS> claims_{};
  std::array<bool, DARWIN_ART_SCM_MAX_PAYLOADS> claim_taken_{};
  std::array<bool, DARWIN_ART_SCM_MAX_PAYLOADS> payload_taken_{};
  ancillary::OwnedRights payload_rights_;
  NativeFd metadata_;
  NativeFd guardian_;
  ssize_t bytes_ = -1;
  int flags_ = 0;
  socklen_t name_length_ = 0;
  bool valid_ = false;
  bool private_envelope_ = false;
  bool admitted_ = false;
  bool finished_ = false;
  bool committed_ = false;
  bool terminal_ = false;
  ScmCredentials credentials_{};
  bool credentials_valid_ = false;
};

class SCMChannel final {
public:
  // No socket ownership is transferred.  EndpointLease and native_socket must
  // outlive this channel and every result it creates.
  SCMChannel(EndpointLease &endpoint, int native_socket) noexcept;
  SCMChannel(const SCMChannel &) = delete;
  SCMChannel &operator=(const SCMChannel &) = delete;
  ~SCMChannel() noexcept;

  bool valid() const noexcept { return socket_ >= 0 && endpoint_ != nullptr && provider_.context != nullptr; }
  int Send(const NativeMessage &message, const int *payload_fds,
           std::size_t payload_count, const ManagedPayload *managed,
           std::size_t managed_count, ssize_t *bytes_sent) noexcept;
  int Receive(const NativeMessage &message, const ReceiveOptions &options,
              ReceiveResult *result) noexcept;

private:
  EndpointLease *endpoint_ = nullptr;
  int socket_ = -1;
  DarwinArtScmEndpointProviderV1 provider_{};
};

} // namespace darwin_art::bionic::scm

#endif // DARWIN_ART_BIONIC_SCM_CHANNEL_H_
