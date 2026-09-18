#include "compat/surfaceflinger/client_receipt.h"
#include "compat/surfaceflinger/composition_protocol.h"
#include "compat/surfaceflinger/socket_transport.h"
#include <cassert>
#include <cerrno>
#include <cstdio>
#include <cstring>
#include <fcntl.h>
#include <sys/socket.h>
#include <unistd.h>
#include <stdexcept>

using namespace darwin_art::surfaceflinger;
static int Adopt(int fd) { return fd; }
static bool g_reuse_import_fd = false;
static int g_reused_fd = -1;
static int FailImport(int fd) {
  close(fd);
  if (g_reuse_import_fd) g_reused_fd = open("/dev/null", O_RDONLY);
  return -1;
}
static int ThrowImport(int fd) {
  close(fd);
  throw std::runtime_error("provider failure");
}
static ResponseHeader Header(CommitDisposition disposition, int error, unsigned fence) {
  ResponseHeader header{};
  std::memcpy(header.magic, kResponseMagic.data(), kResponseMagic.size());
  header.version = kProtocolVersion;
  header.commit = disposition;
  header.status = error;
  header.has_completion_fence = fence;
  return header;
}
static DarwinArtSurfaceFlingerReceipt Read(
    const ResponseHeader* header, bool send_right, bool fail_import,
    bool throw_import = false) {
  int pair[2], pipe_fds[2];
  assert(socketpair(AF_UNIX, SOCK_STREAM, 0, pair) == 0);
  assert(pipe(pipe_fds) == 0);
  if (header) assert(WriteAll(pair[0], header, sizeof(*header)));
  if (send_right) assert(SendDescriptor(pair[0], pipe_fds[0]));
  assert(shutdown(pair[0], SHUT_WR) == 0);
  const auto result = ReadClientReceipt(pair[1], throw_import ? ThrowImport :
                                                   fail_import ? FailImport : Adopt);
  close(pair[0]); close(pair[1]); close(pipe_fds[0]); close(pipe_fds[1]);
  return result;
}
static int OpenDescriptors() {
  int count = 0;
  for (int fd = 0; fd < 4096; ++fd) if (fcntl(fd, F_GETFD) >= 0) ++count;
  return count;
}
int main() {
  const int before = OpenDescriptors();
  auto header = Header(CommitDisposition::RejectedBeforeCommit, EBUSY, 0);
  auto result = Read(&header, false, false);
  assert(result.disposition == DARWIN_ART_SF_COMMIT_REJECTED && result.error == EBUSY);
  assert(result.completion_fd == -1);
  result = Read(nullptr, false, false);
  assert(result.disposition == DARWIN_ART_SF_COMMIT_UNKNOWN);
  header = Header(CommitDisposition::Committed, 0, 1);
  result = Read(&header, false, false);
  assert(result.disposition == DARWIN_ART_SF_COMMIT_COMMITTED && result.error != 0);
  assert(result.completion_fd == -1);
  result = Read(&header, true, true);
  assert(result.disposition == DARWIN_ART_SF_COMMIT_COMMITTED && result.error == EIO);
  assert(result.completion_fd == -1 && OpenDescriptors() == before);
  g_reuse_import_fd = true;
  result = Read(&header, true, true);
  assert(result.disposition == DARWIN_ART_SF_COMMIT_COMMITTED && result.error == EIO);
  assert(g_reused_fd >= 0 && fcntl(g_reused_fd, F_GETFD) >= 0);
  close(g_reused_fd);
  g_reuse_import_fd = false;
  assert(OpenDescriptors() == before);
  result = Read(&header, true, false, true);
  assert(result.disposition == DARWIN_ART_SF_COMMIT_COMMITTED && result.error == EIO);
  assert(result.completion_fd == -1 && OpenDescriptors() == before);
  result = Read(&header, true, false);
  assert(result.disposition == DARWIN_ART_SF_COMMIT_COMMITTED && result.error == 0);
  assert(result.completion_fd >= 0);
  close(result.completion_fd);
  header.has_completion_fence = 0;
  result = Read(&header, false, false);
  assert(result.disposition == DARWIN_ART_SF_COMMIT_COMMITTED && result.error != 0);
  header.version = 0;
  result = Read(&header, false, false);
  assert(result.disposition == DARWIN_ART_SF_COMMIT_UNKNOWN);
  header = Header(CommitDisposition::RejectedBeforeCommit, 0, 0);
  result = Read(&header, false, false);
  assert(result.disposition == DARWIN_ART_SF_COMMIT_UNKNOWN);
  assert(OpenDescriptors() == before);
  std::puts("SurfaceFlinger client receipt: rejected/unknown/committed missing-FD/import-failure/ownership PASS");
}
