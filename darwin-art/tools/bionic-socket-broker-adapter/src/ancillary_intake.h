#ifndef DARWIN_ART_BIONIC_ANCILLARY_INTAKE_H_
#define DARWIN_ART_BIONIC_ANCILLARY_INTAKE_H_

// Managed-only native Darwin recvmsg primitive.  This is deliberately not a
// replacement for the process' socket ABI: its caller must own the managed
// carrier and must handle (or reject) every ancillary record.

#include <sys/socket.h>

#include <array>
#include <cstddef>
#include <cstdint>
#include <sys/types.h>

namespace darwin_art::bionic::ancillary {

// XNU's AF_UNIX SCM_RIGHTS batch is bounded by the kernel.  On the supported
// Darwin 25.6 kernel a 254-right send succeeds and 255 returns EINVAL.  Keep
// this bound explicit instead of pretending that an arbitrary 64 KiB control
// allocation is complete.  A future host must rerun the native probe before
// changing this value.
inline constexpr std::size_t kSupportedRights = 254;
inline constexpr std::size_t kControlBytes =
    CMSG_SPACE(kSupportedRights * sizeof(int));
inline constexpr std::size_t kRecordCapacity =
    kControlBytes / sizeof(struct cmsghdr) + 1;

// The bound covers one native rights batch (the managed carrier's supported
// shape).  Callers must reject a protocol envelope whose rights/groups and
// other ancillary records cannot fit before enqueueing it.  MSG_CTRUNC is
// fatal: visible rights are first acquired and then discarded, while any
// rights hidden by the kernel's truncation are unrecoverable and therefore
// cannot be reported as a successful managed receive.

static_assert(kControlBytes >= CMSG_LEN(kSupportedRights * sizeof(int)));

struct RecvRequest {
  int socket = -1;

  // Both the iovec array and each pointed-to payload are borrowed for the
  // duration of Recvmsg.  No payload or peer-name copy is made.
  struct iovec *vectors = nullptr;
  std::size_t vector_count = 0;
  void *name = nullptr;
  socklen_t name_capacity = 0;
  int flags = 0;
};

struct AncillaryRecord {
  int level = 0;
  int type = 0;
  const unsigned char *data = nullptr; // borrowed from RecvResult
  std::size_t data_length = 0;
};

class RecvResult;

// Every descriptor returned by native recvmsg is owned by this object as soon
// as it is externalized by the kernel.  A caller must explicitly Take a right
// it publishes; destruction closes all rights not taken, exactly once.
class OwnedRights final {
public:
  OwnedRights() noexcept = default;
  ~OwnedRights() noexcept;

  OwnedRights(const OwnedRights &) = delete;
  OwnedRights &operator=(const OwnedRights &) = delete;
  OwnedRights(OwnedRights &&other) noexcept;
  OwnedRights &operator=(OwnedRights &&other) noexcept;

  std::size_t size() const noexcept { return count_; }

  // On success, writes the descriptor to *descriptor and transfers it to the
  // caller.  A repeated take of the same slot fails with EINVAL semantics
  // (without touching any other slot).  The caller owns the returned fd.
  bool Take(std::size_t index, int *descriptor) noexcept;

  // Close every descriptor still owned.  Safe to call repeatedly.
  void Discard() noexcept;

private:
  friend class RecvResult;
  friend ssize_t Recvmsg(const RecvRequest &, RecvResult *) noexcept;
  void Add(int descriptor) noexcept;
  bool EnsureCloseOnExec() noexcept;
  std::array<int, kSupportedRights> descriptors_{};
  std::size_t count_ = 0;
};

class RecvResult final {
public:
  RecvResult() noexcept = default;
  ~RecvResult() = default;

  RecvResult(const RecvResult &) = delete;
  RecvResult &operator=(const RecvResult &) = delete;
  RecvResult(RecvResult &&) = delete;
  RecvResult &operator=(RecvResult &&) = delete;

  ssize_t bytes() const noexcept { return bytes_; }
  int flags() const noexcept { return flags_; }
  socklen_t name_length() const noexcept { return name_length_; }
  const OwnedRights &rights() const noexcept { return rights_; }
  OwnedRights &rights() noexcept { return rights_; }

  std::size_t ancillary_count() const noexcept { return record_count_; }
  const AncillaryRecord &ancillary(std::size_t index) const noexcept {
    return records_[index];
  }

private:
  friend ssize_t Recvmsg(const RecvRequest &, RecvResult *) noexcept;
  void Reset() noexcept;

  ssize_t bytes_ = -1;
  int flags_ = 0;
  socklen_t name_length_ = 0;
  OwnedRights rights_;
  std::array<unsigned char, kControlBytes> control_{};
  std::array<AncillaryRecord, kRecordCapacity> records_{};
  std::size_t record_count_ = 0;
};

// Decode an already-received native control buffer without dereferencing an
// unaligned cmsghdr.  This is public for unit tests and for the eventual
// managed translation owner; records borrow |control| and remain valid only
// until that storage is reused.  It does not take ownership of integer fds.
bool DecodeAncillary(const void *control, std::size_t control_length,
                     AncillaryRecord *records, std::size_t record_capacity,
                     std::size_t *record_count) noexcept;

// Consume one native message with a complete private control buffer.
// The operation always uses nonblocking native intake under the installed
// Rust inheritance boundary through ownership/CLOEXEC. Its carrier owner must
// perform readiness/deadline waits and would-block retries OUTSIDE that guard.
// MSG_PEEK is rejected with EOPNOTSUPP before recvmsg, because native Darwin
// peek does not return SCM_RIGHTS and a cache would violate the managed carrier
// contract. On syscall failure, the native return/error are preserved
// (-1/errno).  A native MSG_CTRUNC or malformed control sequence is fatal to
// this managed operation; all externalized rights are closed and errno is set
// to EMSGSIZE or EPROTO respectively.
ssize_t Recvmsg(const RecvRequest &request, RecvResult *result) noexcept;

} // namespace darwin_art::bionic::ancillary

#endif // DARWIN_ART_BIONIC_ANCILLARY_INTAKE_H_
