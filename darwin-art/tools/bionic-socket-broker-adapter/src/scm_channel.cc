#include "scm_channel.h"

#include "fd_inheritance.h"

#include <algorithm>
#include <cerrno>
#include <climits>
#include <cstring>
#include <fcntl.h>
#include <poll.h>
#include <sys/socket.h>
#include <sys/stat.h>
#include <sys/time.h>
#include <unistd.h>

#include <vector>

namespace darwin_art::bionic::scm {
namespace {

constexpr std::size_t kMaxRights = 2 + DARWIN_ART_SCM_MAX_PAYLOADS;
constexpr std::size_t kMaxMetadata = 8 + 1024;

bool Nonzero(const uint8_t *bytes) noexcept {
  if (bytes == nullptr) return false;
  for (std::size_t i = 0; i < 16; ++i) if (bytes[i] != 0) return true;
  return false;
}

void CloseFd(int *descriptor) noexcept {
  if (descriptor == nullptr || *descriptor < 0) return;
  const int saved_errno = errno;
  (void)close(*descriptor);
  *descriptor = -1;
  errno = saved_errno;
}

void ReleaseProvider(DarwinArtScmEndpointProviderV1 *provider) noexcept {
  if (provider == nullptr || provider->context == nullptr) return;
  const int saved_errno = errno;
  provider->release(provider->context);
  *provider = {};
  errno = saved_errno;
}

bool ValidMessage(const NativeMessage &message) noexcept {
  return message.vector_count <= static_cast<std::size_t>(INT_MAX) &&
         (message.vector_count == 0 || message.vectors != nullptr) &&
         (message.name_length == 0 || message.name != nullptr);
}

bool ReadAuthenticatedCredentials(int descriptor, ScmCredentials *credentials) noexcept {
  if (descriptor < 0 || credentials == nullptr) return false;
  struct stat metadata_stat{};
  if (fstat(descriptor, &metadata_stat) != 0 ||
      (metadata_stat.st_mode & S_IFMT) != S_IFREG || metadata_stat.st_size < 0 ||
      static_cast<uint64_t>(metadata_stat.st_size) > kMaxMetadata) return false;
  const std::size_t size = static_cast<std::size_t>(metadata_stat.st_size);
  if (size < 8) return false;
  std::array<unsigned char, kMaxMetadata> bytes{};
  std::size_t offset = 0;
  while (offset < size) {
    ssize_t read;
    do { read = ::pread(descriptor, bytes.data() + offset, size - offset, offset); }
    while (read < 0 && errno == EINTR);
    if (read <= 0) return false;
    offset += static_cast<std::size_t>(read);
  }
  if (bytes[0] != 2 || bytes[1] != 2 || bytes[2] != 0 || bytes[3] != 0) return false;
  const uint32_t body_size = static_cast<uint32_t>(bytes[4]) |
      (static_cast<uint32_t>(bytes[5]) << 8) |
      (static_cast<uint32_t>(bytes[6]) << 16) |
      (static_cast<uint32_t>(bytes[7]) << 24);
  if (body_size > 1024 || body_size + 8 != size || body_size < 28) return false;
  const uint16_t payload_count = static_cast<uint16_t>(bytes[32]) |
      (static_cast<uint16_t>(bytes[33]) << 8);
  const uint16_t managed_count = static_cast<uint16_t>(bytes[34]) |
      (static_cast<uint16_t>(bytes[35]) << 8);
  if (payload_count > DARWIN_ART_SCM_MAX_PAYLOADS || managed_count > payload_count) return false;
  const std::size_t credential_offset = 36 + static_cast<std::size_t>(managed_count) * 24;
  if (credential_offset + 12 != size) return false;
  const auto load32 = [&](std::size_t at) -> uint32_t {
    return static_cast<uint32_t>(bytes[at]) |
        (static_cast<uint32_t>(bytes[at + 1]) << 8) |
        (static_cast<uint32_t>(bytes[at + 2]) << 16) |
        (static_cast<uint32_t>(bytes[at + 3]) << 24);
  };
  const int32_t pid = static_cast<int32_t>(load32(credential_offset));
  if (pid <= 0) return false;
  credentials->process_id = pid;
  credentials->user_id = load32(credential_offset + 4);
  credentials->group_id = load32(credential_offset + 8);
  return true;
}

struct SendContext {
  int socket;
  msghdr *message;
  int flags;
};

intptr_t SendOperation(void *opaque) noexcept {
  auto *context = static_cast<SendContext *>(opaque);
  ssize_t sent;
  do {
    sent = ::sendmsg(context->socket, context->message, context->flags);
  } while (sent < 0 && errno == EINTR);
  return sent;
}

bool GetNonBlocking(int socket, bool *nonblocking) noexcept {
  int flags;
  do { flags = fcntl(socket, F_GETFL); } while (flags < 0 && errno == EINTR);
  if (flags < 0) return false;
  *nonblocking = (flags & O_NONBLOCK) != 0;
  return true;
}

int64_t NowNanos() noexcept {
  timespec now{};
  if (clock_gettime(CLOCK_MONOTONIC, &now) != 0) return -1;
  return static_cast<int64_t>(now.tv_sec) * 1000000000LL + now.tv_nsec;
}

bool SocketDeadline(int socket, int option, timespec *storage,
                    const timespec **deadline) noexcept {
  if (*deadline != nullptr) return true;
  timeval timeout{};
  socklen_t length = sizeof(timeout);
  if (getsockopt(socket, SOL_SOCKET, option, &timeout, &length) != 0) return false;
  if (timeout.tv_sec == 0 && timeout.tv_usec == 0) return true;
  const int64_t now = NowNanos();
  if (now < 0 || timeout.tv_sec < 0 || timeout.tv_usec < 0 || timeout.tv_usec >= 1000000) {
    errno = EINVAL; return false;
  }
  const int64_t micros = static_cast<int64_t>(timeout.tv_usec) * 1000LL;
  int64_t value = INT64_MAX;
  if (timeout.tv_sec <= (INT64_MAX - now - micros) / 1000000000LL)
    value = now + static_cast<int64_t>(timeout.tv_sec) * 1000000000LL + micros;
  *storage = {static_cast<time_t>(value / 1000000000LL),
              static_cast<long>(value % 1000000000LL)};
  *deadline = storage;
  return true;
}

int PollTimeout(const timespec *deadline) noexcept {
  if (deadline == nullptr) return -1;
  const int64_t now = NowNanos();
  if (now < 0) return 0;
  int64_t value = INT64_MAX;
  if (deadline->tv_sec < INT64_MAX / 1000000000LL)
    value = static_cast<int64_t>(deadline->tv_sec) * 1000000000LL + deadline->tv_nsec;
  if (value <= now) return 0;
  const int64_t nanos = value - now;
  const int64_t millis = nanos / 1000000 + (nanos % 1000000 != 0);
  return millis > INT_MAX ? INT_MAX : static_cast<int>(millis);
}

int WaitReadable(int socket, const ReceiveOptions &options) noexcept {
  if (options.blocking == ReceiveBlocking::kNonBlocking ||
      (options.flags & MSG_DONTWAIT) != 0) return 0;
  bool nonblocking = false;
  if (!GetNonBlocking(socket, &nonblocking)) return -1;
  if (nonblocking) return 0;
  pollfd descriptor{socket, POLLIN | POLLPRI, 0};
  int ready;
  do { ready = poll(&descriptor, 1, PollTimeout(options.deadline)); } while (ready < 0 && errno == EINTR);
  if (ready == 0) { errno = EAGAIN; return -1; }
  if (ready < 0) return -1;
  if ((descriptor.revents & POLLNVAL) != 0) { errno = EBADF; return -1; }
  // Let recvmsg report the actual EOF/error even when readiness reports HUP.

  return 0;
}

int WaitWritable(int socket, const timespec *deadline) noexcept {
  bool nonblocking = false;
  if (!GetNonBlocking(socket, &nonblocking)) return -1;
  if (nonblocking) return 0;
  pollfd descriptor{socket, POLLOUT, 0};
  int ready;
  do { ready = poll(&descriptor, 1, PollTimeout(deadline)); } while (ready < 0 && errno == EINTR);
  if (ready == 0) { errno = EAGAIN; return -1; }
  if (ready < 0) return -1;
  if ((descriptor.revents & POLLNVAL) != 0) { errno = EBADF; return -1; }
  return 0;
}

bool AdvanceVectors(std::vector<iovec> *vectors, std::size_t bytes) noexcept {
  std::size_t index = 0;
  while (index < vectors->size() && bytes != 0) {
    iovec &vector = (*vectors)[index];
    if (bytes >= vector.iov_len) {
      bytes -= vector.iov_len;
      vector.iov_base = static_cast<char *>(vector.iov_base) + vector.iov_len;
      vector.iov_len = 0;
      ++index;
    } else {
      vector.iov_base = static_cast<char *>(vector.iov_base) + bytes;
      vector.iov_len -= bytes;
      bytes = 0;
    }
  }
  return bytes == 0;
}

std::size_t RemainingBytes(const std::vector<iovec> &vectors) noexcept {
  std::size_t result = 0;
  for (const iovec &vector : vectors) {
    if (vector.iov_len > SIZE_MAX - result) return SIZE_MAX;
    result += vector.iov_len;
  }
  return result;
}

} // namespace

void ReceiveResult::NativeFd::Reset(int descriptor) noexcept {
  if (descriptor_ >= 0) {
    const int saved_errno = errno;
    (void)close(descriptor_);
    errno = saved_errno;
  }
  descriptor_ = descriptor;
}

void GrantLease::Reset() noexcept {
  if (provider_.context == nullptr) return;
  const int saved_errno = errno;
  if (owned_) (void)provider_.release_holder(provider_.context, grant_.holder);
  provider_.release(provider_.context);
  provider_ = {};
  owned_ = false;
  errno = saved_errno;
}

GrantLease::~GrantLease() noexcept { Reset(); }

GrantLease::GrantLease(GrantLease &&other) noexcept
    : provider_(other.provider_), grant_(other.grant_), owned_(other.owned_) {
  other.provider_ = {};
  other.owned_ = false;
}

GrantLease &GrantLease::operator=(GrantLease &&other) noexcept {
  if (this == &other) return *this;
  Reset();
  provider_ = other.provider_;
  grant_ = other.grant_;
  owned_ = other.owned_;
  other.provider_ = {};
  other.owned_ = false;
  return *this;
}

bool GrantLease::Commit() noexcept {
  if (!owned_ || provider_.context == nullptr) return false;
  const int saved_errno = errno;
  owned_ = false;
  provider_.release(provider_.context);
  provider_ = {};
  errno = saved_errno;
  return true;
}

bool GrantLease::AdoptInto(EndpointLease &endpoint) noexcept {
  if (!owned_ || provider_.context == nullptr) {
    errno = EINVAL;
    return false;
  }
  const int status = endpoint.AdoptConfirmedGrant(provider_, grant_);
  if (status != 0) { errno = status; return false; }
  const int saved_errno = errno;
  owned_ = false;
  provider_.release(provider_.context);
  provider_ = {};
  errno = saved_errno;
  return true;
}

void ReceiveResult::Reset() noexcept {
  AbortAndRelease();
  payload_rights_.Discard();
  metadata_.Reset();
  guardian_.Reset();
  ReleaseProvider(&provider_);
  authority_ = {};
  carrier_holder_ = {};
  ticket_ = 0;
  payload_count_ = 0;
  claim_count_ = 0;
  claim_taken_.fill(false);
  payload_taken_.fill(false);
  bytes_ = -1;
  flags_ = 0;
  name_length_ = 0;
  valid_ = false;
  private_envelope_ = false;
  admitted_ = false;
  finished_ = false;
  committed_ = false;
  terminal_ = false;
  credentials_ = {};
  credentials_valid_ = false;
}

void ReceiveResult::AbortAndRelease() noexcept {
  if (provider_.context == nullptr) return;
  const int saved_errno = errno;
  if (admitted_ && !finished_) {
    (void)provider_.settle(provider_.context, authority_.data(), ticket_,
                           DARWIN_ART_SCM_ABORTED);
  }
  // Finished can commit at the authority even when its ACK is lost. Every
  // known fresh claim still owned here must be explicitly released on failure;
  // an Abort request alone cannot reclaim an already Finished transfer.
  if (admitted_ && !committed_) {
    for (std::size_t i = 0; i < claim_count_; ++i) {
      if (!claim_taken_[i])
        (void)provider_.release_holder(provider_.context, claims_[i].grant.holder);
    }
  }
  admitted_ = false;
  finished_ = false;
  errno = saved_errno;
}

ReceiveResult::~ReceiveResult() noexcept { Reset(); }

ReceiveResult::ReceiveResult(ReceiveResult &&other) noexcept
    : provider_(other.provider_), authority_(other.authority_),
      carrier_holder_(other.carrier_holder_), ticket_(other.ticket_),
      payload_count_(other.payload_count_), claim_count_(other.claim_count_),
      claims_(other.claims_), claim_taken_(other.claim_taken_),
      payload_taken_(other.payload_taken_),
      payload_rights_(std::move(other.payload_rights_)),
      metadata_(std::move(other.metadata_)), guardian_(std::move(other.guardian_)),
      bytes_(other.bytes_), flags_(other.flags_), name_length_(other.name_length_),
      valid_(other.valid_), private_envelope_(other.private_envelope_),
      admitted_(other.admitted_), finished_(other.finished_),
      committed_(other.committed_), terminal_(other.terminal_) {
  credentials_ = other.credentials_;
  credentials_valid_ = other.credentials_valid_;
  other.provider_ = {};
  other.valid_ = false;
  other.admitted_ = false;
  other.finished_ = false;
  other.committed_ = false;
  other.terminal_ = false;
  other.credentials_valid_ = false;
}

ReceiveResult &ReceiveResult::operator=(ReceiveResult &&other) noexcept {
  if (this == &other) return *this;
  Reset();
  provider_ = other.provider_;
  authority_ = other.authority_;
  carrier_holder_ = other.carrier_holder_;
  ticket_ = other.ticket_;
  payload_count_ = other.payload_count_;
  claim_count_ = other.claim_count_;
  claims_ = other.claims_;
  claim_taken_ = other.claim_taken_;
  payload_taken_ = other.payload_taken_;
  payload_rights_ = std::move(other.payload_rights_);
  metadata_ = std::move(other.metadata_);
  guardian_ = std::move(other.guardian_);
  bytes_ = other.bytes_;
  flags_ = other.flags_;
  name_length_ = other.name_length_;
  valid_ = other.valid_;
  private_envelope_ = other.private_envelope_;
  admitted_ = other.admitted_;
  finished_ = other.finished_;
  committed_ = other.committed_;
  terminal_ = other.terminal_;
  credentials_ = other.credentials_;
  credentials_valid_ = other.credentials_valid_;
  other.provider_ = {};
  other.valid_ = false;
  other.admitted_ = false;
  other.finished_ = false;
  other.committed_ = false;
  other.terminal_ = false;
  other.credentials_valid_ = false;
  return *this;
}

bool ReceiveResult::AllPayloadsHandedOff() const noexcept {
  for (std::size_t i = 0; i < payload_count_; ++i)
    if (!payload_taken_[i]) return false;
  return true;
}

int ReceiveResult::Admit(const uint64_t *publish_ordinals,
                         std::size_t publish_count) noexcept {
  if (!valid_ || !private_envelope_ || admitted_ || terminal_ || finished_ ||
      provider_.context == nullptr || metadata_.get() < 0 ||
      publish_count > DARWIN_ART_SCM_MAX_PAYLOADS ||
      (publish_count != 0 && publish_ordinals == nullptr)) {
    errno = EINVAL;
    return -1;
  }
  for (std::size_t i = 0; i < publish_count; ++i) {
    if (publish_ordinals[i] >= payload_count_) { errno = EINVAL; return -1; }
    for (std::size_t j = 0; j < i; ++j)
      if (publish_ordinals[i] == publish_ordinals[j]) { errno = EINVAL; return -1; }
  }
  DarwinArtScmAdmitRequestV2 request{};
  std::memcpy(request.carrier_holder, carrier_holder_.data(), 16);
  request.metadata_fd = metadata_.get();
  request.payload_count = static_cast<uint32_t>(payload_count_);
  request.publish_ordinals = publish_ordinals;
  request.publish_count = static_cast<uint32_t>(publish_count);
  DarwinArtScmAdmissionV2 admission{};
  const int status = provider_.admit(provider_.context, &request, &admission);
  if (status != 0) { terminal_ = true; errno = status; return -1; }
  if (admission.ticket != 0) {
    ticket_ = admission.ticket;
    admitted_ = true;
  }
  // Validation below intentionally treats provider output as untrusted.
  if (!Nonzero(admission.authority) || std::memcmp(admission.authority, authority_.data(), 16) != 0 ||
      admission.ticket == 0 || (ticket_ != 0 && admission.ticket != ticket_) ||
      admission.claim_count > DARWIN_ART_SCM_MAX_PAYLOADS) {
    terminal_ = true; errno = EPROTO; return -1;
  }
  for (std::size_t i = 0; i < admission.claim_count; ++i) {
    const auto &claim = admission.claims[i];
    if (claim.ordinal >= payload_count_ || !Nonzero(claim.grant.authority) ||
        std::memcmp(claim.grant.authority, authority_.data(), 16) != 0 ||
        claim.grant.carrier == 0 || claim.grant.side > 1 ||
        !Nonzero(claim.grant.holder)) {
      terminal_ = true; errno = EPROTO; return -1;
    }
    for (std::size_t j = 0; j < i; ++j)
      if (admission.claims[j].ordinal == claim.ordinal) { terminal_ = true; errno = EPROTO; return -1; }
  }
  ScmCredentials metadata_credentials{};
  if (!ReadAuthenticatedCredentials(metadata_.get(), &metadata_credentials) ||
      admission.credentials.process_id <= 0) {
    terminal_ = true;
    errno = EPROTO;
    return -1;
  }
  credentials_ = {admission.credentials.process_id, admission.credentials.user_id,
                  admission.credentials.group_id};
  credentials_valid_ = true;
  for (std::size_t i = 0; i < DARWIN_ART_SCM_MAX_PAYLOADS; ++i)
    claims_[i] = admission.claims[i];
  claim_count_ = admission.claim_count;
  claim_taken_.fill(false);
  return 0;
}

int ReceiveResult::Finish() noexcept {
  if (!valid_ || !private_envelope_ || !admitted_ || finished_ || terminal_ ||
      provider_.context == nullptr) { errno = EINVAL; return -1; }
  const int status = provider_.settle(provider_.context, authority_.data(), ticket_,
                                      DARWIN_ART_SCM_FINISHED);
  if (status != 0) { terminal_ = true; errno = status; return -1; }
  finished_ = true;
  metadata_.Reset();
  return 0;
}

bool ReceiveResult::TakePayloadFd(std::size_t index, int *descriptor) noexcept {
  if (!finished_ || terminal_ || descriptor == nullptr || index >= payload_count_ ||
      payload_taken_[index]) { errno = EINVAL; return false; }
  if (!payload_rights_.Take(index + 2, descriptor)) { errno = EINVAL; return false; }
  payload_taken_[index] = true;
  return true;
}

bool ReceiveResult::DiscardPayloadFd(std::size_t index) noexcept {
  int descriptor = -1;
  if (!TakePayloadFd(index, &descriptor)) return false;
  CloseFd(&descriptor);
  return true;
}

bool ReceiveResult::TakeGrant(std::size_t index, GrantLease *lease) noexcept {
  if (!finished_ || terminal_ || lease == nullptr || index >= claim_count_ ||
      claim_taken_[index] || provider_.context == nullptr) { errno = EINVAL; return false; }
  GrantLease replacement;
  void *retained = provider_.retain(provider_.context);
  if (retained == nullptr) { errno = ENOMEM; return false; }
  replacement.provider_ = provider_;
  replacement.provider_.context = retained;
  replacement.grant_ = claims_[index].grant;
  replacement.owned_ = true;
  *lease = std::move(replacement);
  claim_taken_[index] = true;
  return true;
}

bool ReceiveResult::ReadyToCommit() const noexcept {
  if (!finished_ || terminal_ || committed_ || !AllPayloadsHandedOff()) return false;
  for (std::size_t i = 0; i < claim_count_; ++i)
    if (!claim_taken_[i]) return false;
  return true;
}

bool ReceiveResult::Commit() noexcept {
  if (!ReadyToCommit()) { errno = EINVAL; return false; }
  committed_ = true;
  guardian_.Reset();
  return true;
}

SCMChannel::SCMChannel(EndpointLease &endpoint, int native_socket) noexcept
    : endpoint_(&endpoint), socket_(native_socket) {
  if (native_socket < 0 || endpoint.RetainProvider(&provider_) != 0) {
    socket_ = -1;
    endpoint_ = nullptr;
  }
}

SCMChannel::~SCMChannel() noexcept { ReleaseProvider(&provider_); }

int SCMChannel::Send(const NativeMessage &message, const int *payload_fds,
                     std::size_t payload_count, const ManagedPayload *managed,
                     std::size_t managed_count, ssize_t *bytes_sent) noexcept {
  if (bytes_sent == nullptr) { errno = EINVAL; return -1; }
  *bytes_sent = -1;
  if (!valid() || !ValidMessage(message) || payload_count > DARWIN_ART_SCM_MAX_PAYLOADS ||
      managed_count > payload_count || (payload_count != 0 && payload_fds == nullptr) ||
      (managed_count != 0 && managed == nullptr)) { errno = EINVAL; return -1; }
  if (payload_count == 0 && managed_count != 0) { errno = EINVAL; return -1; }
  if (endpoint_->attributes() == nullptr) { errno = EOPNOTSUPP; return -1; }
  std::array<DarwinArtScmManagedPayloadV2, DARWIN_ART_SCM_MAX_PAYLOADS> entries{};
  for (std::size_t i = 0; i < managed_count; ++i) {
    if (managed[i].ordinal >= payload_count || !Nonzero(managed[i].holder.data())) {
      errno = EINVAL; return -1;
    }
    for (std::size_t j = 0; j < i; ++j)
      if (managed[j].ordinal == managed[i].ordinal) {
        errno = EINVAL; return -1;
      }
    entries[i].ordinal = managed[i].ordinal;
    std::memcpy(entries[i].holder, managed[i].holder.data(), 16);
  }

  DarwinArtScmPreparedV2 prepared{};
  prepared.metadata_fd = -1;
  prepared.guardian_fd = -1;
  {
    const auto *attributes = endpoint_->attributes();
    DarwinArtScmPrepareRequestV2 request{};
    std::memcpy(request.carrier_holder, attributes->holder.data(), 16);
    request.payload_fds = payload_fds;
    request.payload_count = static_cast<uint32_t>(payload_count);
    request.managed_count = static_cast<uint32_t>(managed_count);
    request.managed = entries.data();
    const int status = provider_.prepare(provider_.context, &request, &prepared);
    if (status != 0) {
      CloseFd(&prepared.metadata_fd); CloseFd(&prepared.guardian_fd);
      errno = status; return -1;
    }
    if (prepared.metadata_fd < 0 || prepared.guardian_fd < 0 ||
        prepared.payload_count != payload_count || prepared.ticket == 0 ||
        std::memcmp(prepared.authority, attributes->authority.data(), 16) != 0) {
      CloseFd(&prepared.metadata_fd); CloseFd(&prepared.guardian_fd);
      errno = EPROTO; return -1;
    }
  }

  std::array<unsigned char, CMSG_SPACE(kMaxRights * sizeof(int))> control{};
  msghdr native{};
  native.msg_name = const_cast<void *>(message.name);
  native.msg_namelen = message.name_length;
  native.msg_iov = const_cast<iovec *>(message.vectors);
  native.msg_iovlen = static_cast<int>(message.vector_count);
  native.msg_flags = 0;
  std::array<int, kMaxRights> rights{};
  rights[0] = prepared.metadata_fd;
  rights[1] = prepared.guardian_fd;
  for (std::size_t i = 0; i < payload_count; ++i) rights[i + 2] = payload_fds[i];
  native.msg_control = control.data();
  native.msg_controllen = CMSG_SPACE((payload_count + 2) * sizeof(int));
  cmsghdr *header = CMSG_FIRSTHDR(&native);
  if (header == nullptr) { CloseFd(&prepared.metadata_fd); CloseFd(&prepared.guardian_fd); errno = EPROTO; return -1; }
  header->cmsg_level = SOL_SOCKET;
  header->cmsg_type = SCM_RIGHTS;
  header->cmsg_len = CMSG_LEN((payload_count + 2) * sizeof(int));
  std::memcpy(CMSG_DATA(header), rights.data(), (payload_count + 2) * sizeof(int));
  int socket_type = 0;
  socklen_t socket_type_length = sizeof(socket_type);
  sockaddr_storage local_address{};
  socklen_t local_address_length = sizeof(local_address);
  int configured_send_capacity = 0;
  socklen_t capacity_length = sizeof(configured_send_capacity);
  const bool retry_stream_ancillary_pressure =
      message.name_length == 0 &&
      getsockopt(socket_, SOL_SOCKET, SO_TYPE, &socket_type,
                 &socket_type_length) == 0 &&
      socket_type_length == sizeof(socket_type) && socket_type == SOCK_STREAM &&
      getsockname(socket_, reinterpret_cast<sockaddr *>(&local_address),
                  &local_address_length) == 0 &&
      local_address.ss_family == AF_UNIX &&
      getsockopt(socket_, SOL_SOCKET, SO_SNDBUF, &configured_send_capacity,
                 &capacity_length) == 0 &&
      capacity_length == sizeof(configured_send_capacity) &&
      configured_send_capacity > 0 &&
      native.msg_controllen <= static_cast<size_t>(configured_send_capacity);
  ssize_t sent = -1;
  {
    // Every managed send, including a zero-payload record, is a private
    // metadata+guardian envelope. Rights-bearing sendmsg is nonblocking while inside the inheritance
    // boundary.  Readiness waits remain outside that guard, and EAGAIN is the
    // only retryable result; a positive partial send is returned immediately.
    bool nonblocking = false;
    if (!GetNonBlocking(socket_, &nonblocking)) {
      CloseFd(&prepared.metadata_fd); CloseFd(&prepared.guardian_fd); return -1;
    }
    const bool caller_nonblocking = nonblocking || (message.flags & MSG_DONTWAIT) != 0;
    const int send_flags = message.flags | MSG_DONTWAIT;
    timespec deadline_storage{};
    const timespec *deadline = nullptr;
    if (!caller_nonblocking && !SocketDeadline(socket_, SO_SNDTIMEO, &deadline_storage, &deadline)) {
      CloseFd(&prepared.metadata_fd); CloseFd(&prepared.guardian_fd); return -1;
    }
    for (;;) {
      if (!caller_nonblocking && WaitWritable(socket_, deadline) != 0) break;
      SendContext context{socket_, &native, send_flags};
      sent = static_cast<ssize_t>(fd_inheritance::RunFdOperation(&SendOperation, &context));
      if (sent >= 0) break;
      // XNU can return EMSGSIZE when queued stream bytes leave less room than
      // this SCM_RIGHTS envelope, even though the envelope fits SO_SNDBUF.
      // Keep the same prepared ticket and rights across a blocking retry.
      const bool ancillary_pressure =
          errno == EMSGSIZE && retry_stream_ancillary_pressure;
      if (ancillary_pressure) errno = EAGAIN;
      if (caller_nonblocking || errno != EAGAIN) break;
      if (ancillary_pressure) {
        if (deadline != nullptr && PollTimeout(deadline) == 0) break;
        // POLLOUT may remain set when the free space is still smaller than
        // the control message; avoid a busy retry until the peer drains it.
        const timespec pause{0, 1000000};
        (void)nanosleep(&pause, nullptr);
      }
    }
  }
  const int saved_errno = errno;
  CloseFd(&prepared.metadata_fd);
  CloseFd(&prepared.guardian_fd);
  if (sent < 0) { errno = saved_errno; return -1; }
  *bytes_sent = static_cast<ssize_t>(sent);
  errno = saved_errno;
  return 0;
}

int SCMChannel::Receive(const NativeMessage &message, const ReceiveOptions &options,
                        ReceiveResult *result) noexcept {
  if (result == nullptr || !valid() || !ValidMessage(message) ||
      (options.flags & MSG_PEEK) != 0) { errno = (options.flags & MSG_PEEK) ? EOPNOTSUPP : EINVAL; return -1; }
  result->Reset();
  if (options.flags & ~(MSG_OOB | MSG_WAITALL | MSG_DONTWAIT | MSG_NEEDSA)) { errno = EINVAL; return -1; }
  std::vector<iovec> vectors;
  try {
    if (message.vector_count != 0)
      vectors.assign(message.vectors, message.vectors + message.vector_count);
  } catch (...) { errno = ENOMEM; return -1; }
  ReceiveOptions effective = options;
  timespec deadline_storage{};
  if (!SocketDeadline(socket_, SO_RCVTIMEO, &deadline_storage, &effective.deadline)) return -1;
  int socket_type = 0;
  socklen_t socket_type_length = sizeof(socket_type);
  if (getsockopt(socket_, SOL_SOCKET, SO_TYPE, &socket_type, &socket_type_length) != 0)
    return -1;
  // WAITALL never combines independent datagram/seqpacket records.
  const bool waitall = (options.flags & MSG_WAITALL) != 0 && socket_type == SOCK_STREAM;
  const std::size_t requested = RemainingBytes(vectors);
  if (requested == SIZE_MAX) { errno = EOVERFLOW; return -1; }
  const bool needs_readiness = requested != 0 || socket_type != SOCK_STREAM;
  std::size_t total = 0;
  ancillary::OwnedRights final_rights;
  int final_flags = 0;
  socklen_t final_name_length = 0;
  bool got_rights = false;
  for (;;) {
    // Empty stream reads are immediate; empty record receives still wait for
    // and consume one complete record, including its ancillary envelope.
    if (needs_readiness && WaitReadable(socket_, effective) != 0) {
      if (total != 0) break;
      return -1;
    }
    ancillary::RecvRequest request{};
    request.socket = socket_;
    request.vectors = vectors.empty() ? nullptr : vectors.data();
    request.vector_count = vectors.size();
    request.name = const_cast<void *>(message.name);
    request.name_capacity = message.name_length;
    request.flags = options.flags;
    ancillary::RecvResult native;
    const ssize_t received = ancillary::Recvmsg(request, &native);
    if (received < 0) {
      if (errno == EAGAIN && needs_readiness &&
          options.blocking == ReceiveBlocking::kWait &&
          (options.flags & MSG_DONTWAIT) == 0) {
        bool nonblocking = false;
        if (GetNonBlocking(socket_, &nonblocking) && !nonblocking) continue;
      }
      if (total != 0) break;
      return -1;
    }
    final_flags |= native.flags();
    final_name_length = native.name_length();
    if (native.ancillary_count() != 0 && native.rights().size() == 0) { errno = EPROTO; return -1; }
    if (native.rights().size() != 0) {
      if (native.ancillary_count() != 1 || native.ancillary(0).level != SOL_SOCKET ||
          native.ancillary(0).type != SCM_RIGHTS || native.rights().size() < 2 ||
          native.rights().size() > kMaxRights) { errno = EPROTO; return -1; }
      final_rights = std::move(native.rights());
      got_rights = true;
    }
    total += static_cast<std::size_t>(received);
    if (!AdvanceVectors(&vectors, static_cast<std::size_t>(received))) { errno = EPROTO; return -1; }
    if (got_rights || !waitall || received == 0 || total >= requested) break;
  }

  void *retained = provider_.retain(provider_.context);
  if (retained == nullptr) { errno = ENOMEM; return -1; }
  result->provider_ = provider_;
  result->provider_.context = retained;
  const auto *attributes = endpoint_->attributes();
  std::memcpy(result->authority_.data(), attributes->authority.data(), 16);
  std::memcpy(result->carrier_holder_.data(), attributes->holder.data(), 16);
  result->bytes_ = static_cast<ssize_t>(total);
  result->flags_ = final_flags;
  result->name_length_ = final_name_length;
  result->valid_ = true;
  result->private_envelope_ = got_rights;
  if (got_rights) {
    int metadata = -1, guardian = -1;
    if (!final_rights.Take(0, &metadata) || !final_rights.Take(1, &guardian)) {
      CloseFd(&metadata); CloseFd(&guardian); result->Reset(); errno = EPROTO; return -1;
    }
    result->metadata_.Reset(metadata);
    result->guardian_.Reset(guardian);
    result->payload_rights_ = std::move(final_rights);
    result->payload_count_ = result->payload_rights_.size() - 2;
    result->payload_taken_.fill(false);
  }
  return 0;
}

} // namespace darwin_art::bionic::scm
