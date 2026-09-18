#include "compat/surfaceflinger/service_darwin.h"
#include "compat/surfaceflinger/composition_protocol.h"
#include "compat/surfaceflinger/socket_transport.h"
#include "compat/surfaceflinger/transaction_reply.h"
#include <atomic>
#include <cassert>
#include <cerrno>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <fcntl.h>
#include <poll.h>
#include <sys/socket.h>
#include <sys/un.h>
#include <thread>
#include <unistd.h>

using namespace darwin_art::surfaceflinger;
static std::atomic<bool> g_fail_import{false};
extern "C" int darwin_art_bionic_socket_broker_close(int fd) { return close(fd); }
extern "C" int darwin_art_bionic_fd_export_for_scm(int fd) { return dup(fd); }
extern "C" int darwin_art_bionic_fd_import_from_scm(int fd) {
  if (g_fail_import.load()) { close(fd); return -1; }
  return fd;
}
extern "C" int darwin_art_android_metal_shared_event_fence_fd(void*, uint64_t) {
  assert(false && "fixture submits no producer event");
  return -1;
}

enum class Reply { Commit, Reject, Lost, MissingFd };
enum class Port { Commit, Submit, Present, LegacyCommit };
static int OpenDescriptors() {
  int count = 0;
  for (int fd = 0; fd < 4096; ++fd) if (fcntl(fd, F_GETFD) >= 0) ++count;
  return count;
}

int main(int argc, char** argv) {
  assert(argc == 2);
  const int initial = OpenDescriptors();
  const int listener = socket(AF_UNIX, SOCK_STREAM, 0);
  assert(listener >= 0);
  sockaddr_un address{};
  address.sun_family = AF_UNIX;
  assert(std::strlen(argv[1]) < sizeof(address.sun_path));
  std::strcpy(address.sun_path, argv[1]);
  assert(bind(listener, reinterpret_cast<sockaddr*>(&address), sizeof(address)) == 0);
  assert(listen(listener, 4) == 0);
  assert(setenv("DARWIN_ART_SURFACEFLINGER_SOCKET", argv[1], 1) == 0);
  DarwinArtMetalComposerLayer layer{};
  layer.owner_process_id = static_cast<uint32_t>(getpid());
  layer.layer_id = 99;
  layer.what = 0;
  layer.transparent_region_count = 1;
  layer.transparent_region[0] = {1, 2, 8, 9};
  unsigned requests = 0;
  auto exchange = [&](Port port, Reply reply, bool fail_import = false) {
    const unsigned before = requests;
    int pipe_fds[2];
    assert(pipe(pipe_fds) == 0);
    g_fail_import.store(fail_import);
    std::thread server([&] {
      const int peer = accept(listener, nullptr, nullptr);
      assert(peer >= 0);
      RequestHeader request{};
      assert(ReadAll(peer, &request, sizeof(request)));
      assert(std::memcmp(request.magic, kRequestMagic.data(), 8) == 0);
      assert(request.version == kProtocolVersion && request.layer_count == 1);
      assert(request.transaction_id == 123 &&
             request.process_id == static_cast<uint32_t>(getpid()));
      const auto expected = port == Port::Present ? RequestKind::kDisplayPresent :
                            port == Port::Submit ? RequestKind::kLayerTransaction :
                                                   RequestKind::kStructuralCommit;
      assert(request.kind == static_cast<uint32_t>(expected));
      assert(request.has_producer_fence == 0);
      if (port == Port::Present)
        assert(request.target_iosurface_id == 7 && request.target_width == 360 &&
               request.target_height == 640);
      else
        assert(request.target_iosurface_id == 0 && request.target_width == 0 &&
               request.target_height == 0);
      WireLayer wire{};
      assert(ReadAll(peer, &wire, sizeof(wire)) && wire.layer_id == 99);
      assert(wire.transparent_region_count == 1 &&
             wire.transparent_region[0].left == 1 && wire.transparent_region[0].bottom == 9);
      int producer = -1;
      assert(ReceiveDescriptor(peer, false, &producer) && producer == -1);
      ++requests;
      if (reply == Reply::Commit || reply == Reply::Reject) {
        auto response = TransactionReply::Create(peer, pipe_fds[0]);
        assert(response);
        if (reply == Reply::Commit) assert(response->Committed());
        else assert(response->Rejected(EBUSY));
      } else if (reply == Reply::MissingFd) {
        ResponseHeader response{};
        std::memcpy(response.magic, kResponseMagic.data(), 8);
        response.version = kProtocolVersion;
        response.commit = CommitDisposition::Committed;
        response.has_completion_fence = 1;
        assert(WriteAll(peer, &response, sizeof(response)));
      }
      close(peer);
    });
    DarwinArtSurfaceFlingerReceipt result{DARWIN_ART_SF_COMMIT_UNKNOWN, EIO, -1};
    if (port == Port::Commit)
      result = darwin_art_surfaceflinger_service_commit_receipt(123, &layer, 1);
    else if (port == Port::Submit)
      result = darwin_art_surfaceflinger_service_submit_receipt(123, &layer, 1);
    else if (port == Port::Present)
      result = darwin_art_surfaceflinger_service_present_receipt(7, 360, 640, 123,
                                                               &layer, 1, nullptr, 0);
    else {
      result.completion_fd = darwin_art_surfaceflinger_service_commit(123, &layer, 1);
      assert(result.completion_fd >= 0);
      result.disposition = DARWIN_ART_SF_COMMIT_COMMITTED;
      result.error = 0;
    }
    server.join();
    assert(requests == before + 1);
    close(pipe_fds[0]); close(pipe_fds[1]);
    return result;
  };
  for (Port port : {Port::Commit, Port::Submit, Port::Present, Port::LegacyCommit}) {
    const auto result = exchange(port, Reply::Commit);
    assert(result.disposition == DARWIN_ART_SF_COMMIT_COMMITTED && result.error == 0);
    assert(result.completion_fd >= 0);
    close(result.completion_fd);
  }
  auto result = exchange(Port::Commit, Reply::Reject);
  assert(result.disposition == DARWIN_ART_SF_COMMIT_REJECTED && result.error == EBUSY);
  result = exchange(Port::Submit, Reply::Lost);
  assert(result.disposition == DARWIN_ART_SF_COMMIT_UNKNOWN && result.completion_fd == -1);
  result = exchange(Port::Present, Reply::MissingFd);
  assert(result.disposition == DARWIN_ART_SF_COMMIT_COMMITTED && result.error != 0);
  result = exchange(Port::Commit, Reply::Commit, true);
  assert(result.disposition == DARWIN_ART_SF_COMMIT_COMMITTED && result.error == EIO);
  layer.layer_id = 0;
  const unsigned before = requests;
  result = darwin_art_surfaceflinger_service_commit_receipt(123, &layer, 1);
  assert(result.disposition == DARWIN_ART_SF_COMMIT_REJECTED && requests == before);
  pollfd pending{listener, POLLIN, 0};
  assert(poll(&pending, 1, 0) == 0);
  close(listener); unlink(argv[1]);
  assert(OpenDescriptors() == initial);
  std::puts("SurfaceFlinger actual client transport + reply: ports/one request/classification/FD balance PASS");
}
