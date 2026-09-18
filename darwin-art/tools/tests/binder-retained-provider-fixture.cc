#include "binder-retained-provider-fixture.h"
#include "binder/fd_transport.h"
#include "darwin_art_bionic_socket_broker.h"
#include <cerrno>
#include <chrono>
#include <cstdio>
#include <cstdlib>
#include <fcntl.h>
#include <initializer_list>
#include <poll.h>
#include <sys/socket.h>
#include <thread>
#include <unistd.h>

namespace {
void Check(bool value, const char *message) {
  if (!value) {
    std::fprintf(stderr, "retained Binder provider FAIL: %s\n", message);
    std::abort();
  }
}
void Exchange(int sender, int receiver) {
  const char value = 'x';
  Check(::send(sender, &value, 1, 0) == 1, "native send");
  pollfd ready{receiver, POLLIN, 0};
  Check(::poll(&ready, 1, 1000) == 1, "native payload readiness");
  char received = 0;
  Check(::recv(receiver, &received, 1, 0) == 1 && received == value,
        "positive exchange");
}
}

void TestRetainedBinderProvider() {
  Check(darwin_art_bionic_socket_broker_activate() == 0, "activate");
  int32_t original[2]{-1, -1};
  int32_t replacement[2]{-1, -1};
  Check(darwin_art_bionic_socket_broker_socketpair(1, 1, 0, original) == 0,
        "original pair");
  DarwinArtBinderTransferBinding binding{4, 7, 0, 24};
  DarwinArtBinderRetainedExportedDescriptor output{};
  Check(darwin_art_binder_export_retained_file_descriptor(original[0], &binding,
                                                         &output) == 0,
        "actual retained provider export");
  Check(output.host_fd >= 0 && output.lease != nullptr &&
            output.attributes_length == 0,
        "unmanaged provider output");
  const int peer = darwin_art_bionic_fd_export_for_scm(original[1]);
  Check(peer >= 0, "native peer ownership");
  Check(darwin_art_bionic_socket_broker_socketpair(1, 1, 0, replacement) == 0,
        "replacement pair");
  Check(darwin_art_bionic_socket_broker_dup2(replacement[0], original[0]) == original[0],
        "replace guest slot after retained export");
  for (const int fd : {original[0], original[1], replacement[0], replacement[1]})
    Check(darwin_art_bionic_socket_broker_close(fd) == 0, "close guest alias");
  Check(darwin_art_bionic_socket_broker_live_objects() == 1,
        "exact original Description survives close/dup2");
  Exchange(output.host_fd, peer);
  errno = ERANGE;
  darwin_art_binder_release_export_lease(output.lease);
  output.lease = nullptr;
  Check(errno == ERANGE, "release preserves host errno");
  // Broker close callbacks run on its real deferred-close worker. Observe
  // completion; the deadline only fails this fixture and never frees resources.
  const auto deadline = std::chrono::steady_clock::now() + std::chrono::seconds(1);
  while (darwin_art_bionic_socket_broker_live_objects() != 0) {
    Check(std::chrono::steady_clock::now() < deadline,
          "released original Description did not drain");
    std::this_thread::yield();
  }
  Check(darwin_art_bionic_socket_broker_deactivate() == 0, "quiescent teardown");
  Check(fcntl(output.host_fd, F_GETFD) >= 0, "transport owns exported FD");
  Exchange(peer, output.host_fd);
  Check(close(output.host_fd) == 0 && close(peer) == 0, "close native ownership");
  output.host_fd = -1;
  Check(darwin_art_binder_export_retained_file_descriptor(original[0], &binding,
                                                         &output) == -1 &&
            output.host_fd == -1 && output.lease == nullptr,
        "failed Process acquisition leaves no resources");
}
