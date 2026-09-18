#include "../../compat/surfaceflinger/service_ingress.h"
#include "../../compat/surfaceflinger/composition_protocol.h"
#include "../../compat/surfaceflinger/socket_deadline.h"

#include <assert.h>
#include <errno.h>
#include <fcntl.h>
#include <sys/socket.h>
#include <unistd.h>

#include <array>
#include <chrono>
#include <cstdint>
#include <cstdio>
#include <cstring>

namespace {

using darwin_art::surfaceflinger::OutputRequest;
using darwin_art::surfaceflinger::OutputResponse;

struct CallbackState {
  int apply_calls = 0;
  int drop_calls = 0;
  int compose_calls = 0;
  bool drop_saw_open_fd = false;
  OutputRequest last_request{};
  std::array<char, 8> compose_magic{};
};

CallbackState* callbacks = nullptr;

darwin_art::surfaceflinger::OutputResponse ApplyOutput(
    int descriptor, const OutputRequest& request) {
  assert(callbacks != nullptr);
  assert(fcntl(descriptor, F_GETFD) >= 0);
  ++callbacks->apply_calls;
  callbacks->last_request = request;
  OutputResponse response{};
  response.status = 0;
  response.token.server_instance = 0xCAFE;
  response.token.serial = 0xBEEF;
  response.generation = 1;
  return response;
}

void DropOutput(int descriptor) noexcept {
  if (callbacks == nullptr) return;
  ++callbacks->drop_calls;
  callbacks->drop_saw_open_fd = fcntl(descriptor, F_GETFD) >= 0;
}

void Compose(int descriptor, const std::array<char, 8>& magic) {
  assert(callbacks != nullptr);
  assert(fcntl(descriptor, F_GETFD) >= 0);
  ++callbacks->compose_calls;
  callbacks->compose_magic = magic;
}

void SendFragmented(int descriptor, const void* data, size_t size) {
  const auto* bytes = static_cast<const unsigned char*>(data);
  while (size != 0) {
    assert(send(descriptor, bytes, 1, 0) == 1);
    ++bytes;
    --size;
  }
}

bool ReadFragmented(int descriptor, void* data, size_t size,
                    darwin_art::surfaceflinger::SocketDeadline deadline) {
  auto* bytes = static_cast<unsigned char*>(data);
  for (size_t index = 0; index < size; ++index) {
    if (!darwin_art::surfaceflinger::ReadAllUntil(
            descriptor, bytes + index, 1, deadline)) {
      return false;
    }
  }
  return true;
}

void ClosePair(int (&pair)[2]) {
  for (int& descriptor : pair) {
    if (descriptor >= 0) {
      close(descriptor);
      descriptor = -1;
    }
  }
}

void TestFragmentedRegistrationResponseAndDrop() {
  int pair[2] = {-1, -1};
  assert(socketpair(AF_UNIX, SOCK_STREAM, 0, pair) == 0);
  CallbackState state;
  callbacks = &state;
  {
    darwin_art::surfaceflinger::ServiceIngress ingress(
        &ApplyOutput, &DropOutput, &Compose);
    const int server = pair[0];
    assert(ingress.Add(server));
    pair[0] = -1;
    OutputRequest request{};
    request.iosurface_id = 17;
    request.physical_width = 640;
    request.physical_height = 480;
    request.logical_width = 320;
    request.logical_height = 240;
    const auto* request_bytes =
        reinterpret_cast<const unsigned char*>(&request);
    for (size_t index = 0; index < sizeof(request); ++index) {
      // No later bytes are queued: exercise real partial ingress reads rather
      // than relying on separate sends to remain separate stream packets.
      assert(send(pair[1], request_bytes + index, 1, 0) == 1);
      ingress.Ready(server, POLLIN);
      if (index + 1 < sizeof(request)) assert(state.apply_calls == 0);
    }
    assert(state.apply_calls == 1);
    // Drive the real fixed-size response path. ReadAllUntil below intentionally
    // consumes the response in one-byte chunks from a fragmented receive view.
    ingress.Ready(server, POLLOUT);
    OutputResponse response{};
    const auto deadline = std::chrono::steady_clock::now() +
                          std::chrono::milliseconds(500);
    assert(ReadFragmented(pair[1], &response, sizeof(response), deadline));
    assert(response.status == 0 && response.token.serial == 0xBEEF &&
           response.generation == 1);
    assert(state.last_request.iosurface_id == 17);

    // EOF is observed by ingress before it closes the descriptor. A second
    // Ready and the destructor must not duplicate DropOutput.
    assert(shutdown(pair[1], SHUT_WR) == 0);
    ingress.Ready(server, POLLIN | POLLHUP);
    assert(state.drop_calls == 1 && state.drop_saw_open_fd);
    ingress.Ready(server, POLLHUP);
    assert(state.drop_calls == 1);
  }
  assert(state.drop_calls == 1);
  ClosePair(pair);
  callbacks = nullptr;
}

void TestCompositionPrefixRouting() {
  int pair[2] = {-1, -1};
  assert(socketpair(AF_UNIX, SOCK_STREAM, 0, pair) == 0);
  CallbackState state;
  callbacks = &state;
  {
    darwin_art::surfaceflinger::ServiceIngress ingress(
        &ApplyOutput, &DropOutput, &Compose);
    const int server = pair[0];
    assert(ingress.Add(server));
    pair[0] = -1;
    SendFragmented(pair[1], darwin_art::surfaceflinger::kRequestMagic.data(),
                   darwin_art::surfaceflinger::kRequestMagic.size());
    for (size_t index = 0;
         index < darwin_art::surfaceflinger::kRequestMagic.size(); ++index) {
      ingress.Ready(server, POLLIN);
    }
    assert(state.compose_calls == 1 && state.apply_calls == 0);
    assert(state.compose_magic ==
           darwin_art::surfaceflinger::kRequestMagic);
    assert(state.drop_calls == 1 && state.drop_saw_open_fd);
  }
  assert(state.drop_calls == 1);
  ClosePair(pair);
  callbacks = nullptr;
}

void TestSocketDeadlineTimeout() {
  int pair[2] = {-1, -1};
  assert(socketpair(AF_UNIX, SOCK_STREAM, 0, pair) == 0);
  uint8_t byte = 0;
  errno = 0;
  const auto deadline = std::chrono::steady_clock::now() +
                        std::chrono::milliseconds(25);
  assert(!darwin_art::surfaceflinger::ReadAllUntil(
      pair[0], &byte, sizeof(byte), deadline));
  assert(errno == ETIMEDOUT);
  ClosePair(pair);
}

}  // namespace

int main() {
  TestFragmentedRegistrationResponseAndDrop();
  TestCompositionPrefixRouting();
  TestSocketDeadlineTimeout();
  std::puts("service ingress: fragmented registration/response, EOF drop ordering, "
            "composition prefix routing, deadline timeout, no duplicate drop PASS");
}
