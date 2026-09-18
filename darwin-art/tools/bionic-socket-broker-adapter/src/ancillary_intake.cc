#include "ancillary_intake.h"
#include "fd_inheritance.h"

#include <cerrno>
#include <climits>
#include <cstring>
#include <fcntl.h>
#include <unistd.h>

namespace darwin_art::bionic::ancillary {
namespace {

constexpr std::size_t kCmsgAlignment = alignof(struct cmsghdr);

bool AlignCmsg(std::size_t length, std::size_t *aligned) noexcept {
  if (length > SIZE_MAX - (kCmsgAlignment - 1))
    return false;
  *aligned = (length + kCmsgAlignment - 1) & ~(kCmsgAlignment - 1);
  return true;
}

bool ReadHeader(const unsigned char *bytes, std::size_t available,
                std::size_t *length, int *level, int *type,
                std::size_t *aligned) noexcept {
  if (available < sizeof(struct cmsghdr))
    return false;
  socklen_t native_length = 0;
  std::memcpy(&native_length, bytes, sizeof(native_length));
  std::memcpy(level, bytes + sizeof(native_length), sizeof(*level));
  std::memcpy(type, bytes + sizeof(native_length) + sizeof(*level),
              sizeof(*type));
  *length = static_cast<std::size_t>(native_length);
  if (*length < sizeof(struct cmsghdr) || *length > available ||
      !AlignCmsg(*length, aligned) || *aligned > available) {
    return false;
  }
  return true;
}

} // namespace

OwnedRights::~OwnedRights() noexcept { Discard(); }

OwnedRights::OwnedRights(OwnedRights &&other) noexcept
    : descriptors_(other.descriptors_), count_(other.count_) {
  other.count_ = 0;
}

OwnedRights &OwnedRights::operator=(OwnedRights &&other) noexcept {
  if (this == &other)
    return *this;
  Discard();
  descriptors_ = other.descriptors_;
  count_ = other.count_;
  other.count_ = 0;
  return *this;
}

void OwnedRights::Add(int descriptor) noexcept {
  // DecodeAncillary counts all rights before this point.  Keep this guard as
  // a final ownership invariant if the decoder is extended later.
  if (count_ == descriptors_.size()) {
    (void)close(descriptor);
    return;
  }
  descriptors_[count_++] = descriptor;
}

bool OwnedRights::Take(std::size_t index, int *descriptor) noexcept {
  if (descriptor == nullptr || index >= count_ || descriptors_[index] < 0) {
    return false;
  }
  *descriptor = descriptors_[index];
  descriptors_[index] = -1;
  return true;
}

void OwnedRights::Discard() noexcept {
  for (std::size_t index = 0; index < count_; ++index) {
    if (descriptors_[index] >= 0)
      (void)close(descriptors_[index]);
    descriptors_[index] = -1;
  }
  count_ = 0;
}

bool OwnedRights::EnsureCloseOnExec() noexcept {
  for (std::size_t index = 0; index < count_; ++index) {
    const int descriptor = descriptors_[index];
    if (descriptor < 0)
      continue;
    int flags = -1;
    do {
      flags = fcntl(descriptor, F_GETFD);
    } while (flags < 0 && errno == EINTR);
    if (flags < 0)
      return false;
    if ((flags & FD_CLOEXEC) != 0)
      continue;
    int result = -1;
    do {
      result = fcntl(descriptor, F_SETFD, flags | FD_CLOEXEC);
    } while (result < 0 && errno == EINTR);
    if (result < 0)
      return false;
  }
  return true;
}

void RecvResult::Reset() noexcept {
  rights_.Discard();
  bytes_ = -1;
  flags_ = 0;
  name_length_ = 0;
  record_count_ = 0;
}

bool DecodeAncillary(const void *control, std::size_t control_length,
                     AncillaryRecord *records, std::size_t record_capacity,
                     std::size_t *record_count) noexcept {
  if (record_count == nullptr || (control_length != 0 && control == nullptr) ||
      (record_capacity != 0 && records == nullptr)) {
    errno = EINVAL;
    return false;
  }
  *record_count = 0;
  const auto *bytes = static_cast<const unsigned char *>(control);
  std::size_t offset = 0;
  while (offset < control_length) {
    const std::size_t remaining = control_length - offset;
    std::size_t length = 0;
    std::size_t aligned = 0;
    int level = 0;
    int type = 0;
    if (!ReadHeader(bytes + offset, remaining, &length, &level, &type,
                    &aligned)) {
      errno = EPROTO;
      *record_count = 0;
      return false;
    }
    if (*record_count == record_capacity) {
      errno = EOVERFLOW;
      *record_count = 0;
      return false;
    }
    const std::size_t data_offset =
        offset + (sizeof(struct cmsghdr) + kCmsgAlignment - 1) /
                     kCmsgAlignment * kCmsgAlignment;
    records[*record_count] = AncillaryRecord{level, type, bytes + data_offset,
                                             length - sizeof(struct cmsghdr)};
    ++*record_count;
    offset += aligned;
  }
  return true;
}

ssize_t Recvmsg(const RecvRequest &request, RecvResult *result) noexcept {
  if (result == nullptr || request.socket < 0 ||
      request.vector_count > static_cast<std::size_t>(INT_MAX) ||
      (request.vector_count != 0 && request.vectors == nullptr) ||
      (request.name_capacity != 0 && request.name == nullptr)) {
    errno = EINVAL;
    return -1;
  }
  result->Reset();
  if ((request.flags & MSG_PEEK) != 0) {
    errno = EOPNOTSUPP;
    return -1;
  }
  constexpr int kAcceptedInputFlags =
      MSG_OOB | MSG_WAITALL | MSG_DONTWAIT | MSG_NEEDSA;
  // Darwin accepts some unknown bits silently.  Reject them here so a
  // managed caller never mistakes an ignored request flag for native
  // semantics.  MSG_PEEK is handled above with its dedicated error.
  if ((request.flags & ~kAcceptedInputFlags) != 0) {
    errno = EINVAL;
    return -1;
  }

  auto operation = [&]() noexcept -> intptr_t {
    msghdr message{};
    message.msg_name = request.name;
    message.msg_namelen = request.name_capacity;
    message.msg_iov = request.vectors;
    message.msg_iovlen = static_cast<int>(request.vector_count);
    // This pointer is always non-null, even when the caller has no interest in
    // ancillary records.  Managed consumers must never perform zero-control
    // native reads, which silently discard SCM_RIGHTS on Darwin.
    message.msg_control = result->control_.data();
    message.msg_controllen = result->control_.size();
    message.msg_flags = 0;

    const ssize_t received =
        ::recvmsg(request.socket, &message, request.flags | MSG_DONTWAIT);
    const int native_errno = errno;
    if (received < 0) {
      errno = native_errno;
      return -1;
    }
    if (message.msg_controllen > result->control_.size()) {
      result->Reset();
      errno = EPROTO;
      return -1;
    }

    // This is intentionally a streaming decoder. Darwin has already installed
    // SCM_RIGHTS descriptors by the time recvmsg returns; each complete rights
    // record is moved into OwnedRights before looking at the next record.
    // Therefore a malformed later record or MSG_CTRUNC cannot leak an earlier
    // right.
    const auto decode_and_own = [&]() noexcept {
      const auto *bytes = result->control_.data();
      std::size_t offset = 0;
      while (offset < message.msg_controllen) {
        const std::size_t remaining = message.msg_controllen - offset;
        std::size_t length = 0;
        std::size_t aligned = 0;
        int level = 0;
        int type = 0;
        if (!ReadHeader(bytes + offset, remaining, &length, &level, &type,
                        &aligned)) {
          errno = EPROTO;
          return false;
        }
        if (result->record_count_ == result->records_.size()) {
          errno = EOVERFLOW;
          return false;
        }
        const std::size_t data_offset =
            offset + (sizeof(struct cmsghdr) + kCmsgAlignment - 1) /
                         kCmsgAlignment * kCmsgAlignment;
        AncillaryRecord &record = result->records_[result->record_count_++];
        record = AncillaryRecord{level, type, bytes + data_offset,
                                 length - sizeof(struct cmsghdr)};
        if (level == SOL_SOCKET && type == SCM_RIGHTS) {
          if (record.data_length % sizeof(int) != 0 ||
              record.data_length / sizeof(int) > kSupportedRights ||
              result->rights_.size() + record.data_length / sizeof(int) >
                  kSupportedRights) {
            errno = EPROTO;
            return false;
          }
          for (std::size_t data = 0; data < record.data_length;
               data += sizeof(int)) {
            int descriptor = -1;
            std::memcpy(&descriptor, record.data + data, sizeof(descriptor));
            result->rights_.Add(descriptor);
          }
        }
        offset += aligned;
      }
      return true;
    };
    if (!decode_and_own()) {
      const int parse_errno = errno;
      result->Reset();
      errno = parse_errno;
      return -1;
    }

    if ((message.msg_flags & MSG_CTRUNC) != 0) {
      result->Reset();
      errno = EMSGSIZE;
      return -1;
    }
    if (!result->rights_.EnsureCloseOnExec()) {
      const int close_on_exec_errno = errno;
      result->Reset();
      errno = close_on_exec_errno;
      return -1;
    }
    result->bytes_ = received;
    result->flags_ = message.msg_flags;
    result->name_length_ = message.msg_namelen;
    // recvmsg is value-result; preserve its native errno even though the
    // successful decoder does not otherwise consult errno.
    errno = native_errno;
    return received;
  };
  return fd_inheritance::RunFdOperation(
      [](void *context) -> intptr_t {
        return (*static_cast<decltype(operation) *>(context))();
      },
      &operation);
}

} // namespace darwin_art::bionic::ancillary
