#include "client_receipt.h"
#include "composition_protocol.h"
#include "socket_transport.h"
#include <cerrno>
#include <cstddef>
#include <cstring>
#include <sys/stat.h>
#include <unistd.h>

namespace aim::surfaceflinger {
static_assert(sizeof(AimSurfaceFlingerReceipt) == 12);
static_assert(offsetof(AimSurfaceFlingerReceipt, error) == 4);
static_assert(offsetof(AimSurfaceFlingerReceipt, completion_fd) == 8);
static_assert(static_cast<uint32_t>(CommitDisposition::Unknown) == AIM_SF_COMMIT_UNKNOWN);
static_assert(static_cast<uint32_t>(CommitDisposition::RejectedBeforeCommit) == AIM_SF_COMMIT_REJECTED);
static_assert(static_cast<uint32_t>(CommitDisposition::Committed) == AIM_SF_COMMIT_COMMITTED);
AimSurfaceFlingerReceipt ReadClientReceipt(
    int socket_fd, int (*import_descriptor)(int)) {
  AimSurfaceFlingerReceipt receipt{AIM_SF_COMMIT_UNKNOWN, EPROTO, -1};
  ResponseHeader response{};
  if (!ReadAll(socket_fd, &response, sizeof(response)) ||
      std::memcmp(response.magic, kResponseMagic.data(), kResponseMagic.size()) != 0 ||
      response.version != kProtocolVersion) return receipt;
  if (response.commit == CommitDisposition::RejectedBeforeCommit &&
      response.status != 0 && response.has_completion_fence == 0) {
    return {AIM_SF_COMMIT_REJECTED, response.status, -1};
  }
  if (response.commit != CommitDisposition::Committed) return receipt;
  receipt.disposition = AIM_SF_COMMIT_COMMITTED;
  if (response.status != 0 || response.has_completion_fence != 1 ||
      import_descriptor == nullptr) return receipt;
  int native_fd = -1;
  if (!ReceiveDescriptor(socket_fd, true, &native_fd)) return receipt;
  struct stat descriptor_state{};
  if (fstat(native_fd, &descriptor_state) != 0 ||
      !S_ISFIFO(descriptor_state.st_mode)) {
    close(native_fd);
    return receipt;
  }
  int guest_fd = -1;
  try {
    guest_fd = import_descriptor(native_fd);
  } catch (...) {
    // Keep the established commit classification even across a C++ provider
    // failure; descriptor ownership transferred to the consuming provider.
    receipt.error = EIO;
    return receipt;
  }
  if (guest_fd < 0) {
    receipt.error = EIO;
    return receipt;
  }
  receipt.error = 0;
  receipt.completion_fd = guest_fd;
  return receipt;
}
}
