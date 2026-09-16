#include "../../compat/media/consumer_buffer.h"

#include <cassert>
#include <cerrno>
#include <cstdio>
#include <fcntl.h>
#include <initializer_list>
#include <type_traits>
#include <utility>
#include <poll.h>
#include <unistd.h>

struct AHardwareBuffer {};
namespace {
int returned_fd = -1;
int returns = 0;
int releases = 0;
int closes = 0;
int producer;
int producer_refs = 0;
AHardwareBuffer buffer;
}

extern "C" void darwin_art_android_ANativeWindow_release_consumer_slot(
    void* window, int32_t slot, int fence) {
  assert(window == &producer && slot == 2);
  assert(fence == -1 || fcntl(fence, F_GETFD) >= 0);
  returned_fd = fence;
  ++returns;
}
extern "C" int darwin_art_bionic_socket_broker_close(int fd) {
  ++closes;
  return close(fd);
}
extern "C" void AHardwareBuffer_release(AHardwareBuffer* value) {
  assert(value == &buffer);
  ++releases;
}
extern "C" void darwin_art_android_ANativeWindow_acquire(void* value) {
  assert(value == &producer);
  ++producer_refs;
}
extern "C" void darwin_art_android_ANativeWindow_release(void* value) {
  assert(value == &producer && producer_refs > 0);
  --producer_refs;
}

int main() {
  int signal[2];
  assert(pipe(signal) == 0);
  const int acquire = signal[0];
  darwin_art::media::ReturnConsumerBuffer(&producer, 2, acquire, &buffer);
  assert(returns == 1 && releases == 1 && closes == 0);
  assert(returned_fd == acquire);
  pollfd pending{acquire, POLLIN, 0};
  assert(poll(&pending, 1, 0) == 0);  // Producer work is still outstanding.
  assert(write(signal[1], "x", 1) == 1);
  assert(poll(&pending, 1, 0) == 1 && (pending.revents & POLLIN));
  assert(close(signal[1]) == 0);
  assert(darwin_art_bionic_socket_broker_close(returned_fd) == 0);

  darwin_art::media::ReturnConsumerBuffer(&producer, 2, -1, &buffer);
  assert(returns == 2 && returned_fd == -1 && releases == 2 && closes == 1);

  for (bool missing_producer : {false, true}) {
    int fd = open("/dev/null", O_RDONLY);
    assert(fd >= 0);
    darwin_art::media::ReturnConsumerBuffer(
        missing_producer ? nullptr : &producer,
        missing_producer ? 2 : -1, fd, &buffer);
    assert(fcntl(fd, F_GETFD) == -1 && errno == EBADF);
  }
  assert(returns == 2 && releases == 4 && closes == 3);
  darwin_art::media::ReturnConsumerBuffer(nullptr, -1, -1, nullptr);
  assert(returns == 2 && releases == 4 && closes == 3);
  using darwin_art::media::OwnedConsumerBuffer;
  static_assert(!std::is_copy_constructible_v<OwnedConsumerBuffer>);
  static_assert(std::is_nothrow_move_constructible_v<OwnedConsumerBuffer>);
  {
    OwnedConsumerBuffer first(&producer, 2, -1, &buffer);
    assert(producer_refs == 1);
    OwnedConsumerBuffer second(std::move(first));
    assert(first.buffer() == nullptr && first.fence() == -1);
    assert(second.buffer() == &buffer && producer_refs == 1);
    OwnedConsumerBuffer third;
    third = std::move(second);
    third.reset();
    third.reset();
    assert(returns == 3 && releases == 5 && producer_refs == 0);
  }
  try {
    OwnedConsumerBuffer unwind(&producer, 2, -1, &buffer);
    throw 1;
  } catch (int) {}
  assert(returns == 4 && releases == 6 && producer_refs == 0);
  {
    OwnedConsumerBuffer target(&producer, 2, -1, &buffer);
    OwnedConsumerBuffer source(&producer, 2, -1, &buffer);
    target = std::move(source);  // Releases target's previous ownership once.
    assert(returns == 5 && releases == 7 && producer_refs == 1);
  }
  assert(returns == 6 && releases == 8 && producer_refs == 0);
  {
    int input[2], completion[2];
    assert(pipe(input) == 0 && pipe(completion) == 0);
    OwnedConsumerBuffer image(&producer, 2, input[0], &buffer);
    assert(!image.adoptFence(completion[0]));
    assert(fcntl(input[0], F_GETFD) >= 0 && fcntl(completion[0], F_GETFD) >= 0);
    const int acquired = image.takeFence();
    assert(acquired == input[0] && image.takeFence() == -1);
    assert(write(input[1], "x", 1) == 1);
    pollfd ready{acquired, POLLIN, 0};
    assert(poll(&ready, 1, 0) == 1);
    close(acquired); close(input[1]);
    assert(!image.adoptFence(-2));
    assert(image.adoptFence(completion[0]));
    image.reset();
    assert(returned_fd == completion[0]);
    pollfd pending_completion{returned_fd, POLLIN, 0};
    assert(poll(&pending_completion, 1, 0) == 0);
    assert(write(completion[1], "x", 1) == 1);
    assert(poll(&pending_completion, 1, 0) == 1);
    close(returned_fd); close(completion[1]);
  }
  assert(returns == 7 && releases == 9 && producer_refs == 0);
  std::puts("consumer-buffer: PASS pending-fence transfer, signal, orphan cleanup");
}
