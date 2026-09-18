#include "client_receipt.h"
#include "composition_protocol.h"
#include "socket_transport.h"
#include <cerrno>
#include <cstddef>
#include <cstring>
#include <sys/stat.h>
#include <unistd.h>

namespace darwin_art::surfaceflinger {
static_assert(sizeof(DarwinArtSurfaceFlingerReceipt) == 12);
static_assert(offsetof(DarwinArtSurfaceFlingerReceipt, error) == 4);
static_assert(offsetof(DarwinArtSurfaceFlingerReceipt, completion_fd) == 8);
static_assert(static_cast<uint32_t>(CommitDisposition::Unknown) == DARWIN_ART_SF_COMMIT_UNKNOWN);
static_assert(static_cast<uint32_t>(CommitDisposition::RejectedBeforeCommit) == DARWIN_ART_SF_COMMIT_REJECTED);
static_assert(static_cast<uint32_t>(CommitDisposition::Committed) == DARWIN_ART_SF_COMMIT_COMMITTED);
DarwinArtSurfaceFlingerReceipt ReadClientReceipt(
    int socket_fd, int (*import_descriptor)(int)) {
  DarwinArtSurfaceFlingerReceipt receipt{DARWIN_ART_SF_COMMIT_UNKNOWN, EPROTO, -1};
  ResponseHeader response{};
  if (!ReadAll(socket_fd, &response, sizeof(response)) ||
      std::memcmp(response.magic, kResponseMagic.data(), kResponseMagic.size()) != 0 ||
      response.version != kProtocolVersion) return receipt;
  if (response.commit == CommitDisposition::RejectedBeforeCommit &&
      response.status != 0 && response.has_completion_fence == 0) {
    return {DARWIN_ART_SF_COMMIT_REJECTED, response.status, -1};
  }
  if (response.commit != CommitDisposition::Committed) return receipt;
  receipt.disposition = DARWIN_ART_SF_COMMIT_COMMITTED;
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
