#include "../../compat/media/image_consumer_queue.h"

#include <atomic>
#include <cassert>
#include <csignal>
#include <cstdio>
#include <thread>
#include <unistd.h>

struct AHardwareBuffer {};
using namespace darwin_art::media;
namespace {
int producer;
AHardwareBuffer buffer;
std::atomic<int> refs{0}, released{0}, returned{0};
ImageConsumerQueue *reenter = nullptr;
bool probe_acquire = false;
std::atomic<int> reentrant_status{-1};
ConsumerFrame Frame(int sequence) {
  return {{&producer, sequence, -1, &buffer}, sequence, sequence * 100};
}
} // namespace
extern "C" void darwin_art_android_ANativeWindow_acquire(void *value) {
  assert(value == &producer);
  ++refs;
}
extern "C" void darwin_art_android_ANativeWindow_release(void *value) {
  assert(value == &producer && refs.fetch_sub(1) > 0);
}
extern "C" void darwin_art_android_ANativeWindow_release_consumer_slot(
    void *value, int32_t slot, int fence) {
  assert(value == &producer && slot >= 0 && fence == -1);
  if (reenter) {
    if (probe_acquire) {
      reentrant_status = static_cast<int>(reenter->acquireNext().status);
    } else {
      assert(!reenter->releaseAcquired());
    }
  }
  ++returned;
}
extern "C" int darwin_art_bionic_socket_broker_close(int fd) {
  return close(fd);
}
extern "C" void AHardwareBuffer_release(AHardwareBuffer *value) {
  assert(value == &buffer);
  ++released;
}
int main() {
  std::signal(SIGALRM, [](int) { _exit(124); });
  alarm(5);
  {
    ImageConsumerQueue queue(2);
    assert(queue.acquireNext().status == ImageAcquireStatus::NoBuffer);
    assert(queue.enqueue(Frame(1)) && queue.enqueue(Frame(2)));
    auto first = queue.acquireNext();
    assert(first.status == ImageAcquireStatus::Ok &&
           first.frame.dataspace == 1);
    assert(first.frame.timestamp_ns == 100);
    assert(queue.enqueue(Frame(3)));
    auto second = queue.acquireNext();
    assert(second.status == ImageAcquireStatus::Ok &&
           second.frame.dataspace == 2);
    assert(queue.acquireNext().status == ImageAcquireStatus::MaxImages);
    first.frame.lease.reset();
    assert(queue.releaseAcquired());
    auto third = queue.acquireNext();
    assert(third.status == ImageAcquireStatus::Ok &&
           third.frame.dataspace == 3);
    queue.close();
    assert(queue.acquireNext().status == ImageAcquireStatus::NoBuffer);
    assert(second.frame.lease.buffer() == &buffer &&
           third.frame.lease.buffer() == &buffer);
    assert(refs == 2);
    second.frame.lease.reset();
    third.frame.lease.reset();
    assert(queue.releaseAcquired() && queue.releaseAcquired());
    assert(!queue.releaseAcquired());
    assert(!queue.enqueue(Frame(4)));
    queue.close();
  }
  assert(refs == 0 && returned == 4 && released == 4);
  {
    ImageConsumerQueue queue(1);
    reenter = &queue;
    assert(queue.enqueue(Frame(5)) && queue.enqueue(Frame(6)));
    assert(returned == 4); // maxImages=1 does not discard queued frames.
    reenter = nullptr;
    auto oldest = queue.acquireNext();
    assert(oldest.status == ImageAcquireStatus::Ok &&
           oldest.frame.dataspace == 5);
    assert(queue.acquireNext().status == ImageAcquireStatus::MaxImages);
    oldest.frame.lease.reset();
    assert(queue.releaseAcquired());
    reenter = &queue;
    queue.close();
    assert(returned == 6);
    reenter = nullptr;
  }
  {
    ImageConsumerQueue queue(3);
    std::atomic<bool> start{false};
    std::thread writer([&] {
      while (!start.load()) {
      }
      for (int i = 0; i < 1000; ++i)
        queue.enqueue(Frame(i));
    });
    std::thread reader([&] {
      while (!start.load()) {
      }
      for (int i = 0; i < 1000; ++i) {
        auto item = queue.acquireNext();
        if (item.status == ImageAcquireStatus::Ok) {
          item.frame.lease.reset();
          assert(queue.releaseAcquired());
        }
      }
    });
    start = true;
    queue.close();
    writer.join();
    reader.join();
  }
  assert(refs == 0 && returned == 1006 && released == 1006);
  {
    ImageConsumerQueue queue(3);
    assert(queue.enqueue(Frame(7)));
    auto held = queue.acquireNext();
    assert(held.status == ImageAcquireStatus::Ok && held.frame.dataspace == 7);

    assert(queue.enqueue(Frame(10)) && queue.enqueue(Frame(11)) &&
           queue.enqueue(Frame(12)));
    reentrant_status = -1;
    reenter = &queue;
    probe_acquire = true;
    auto latest = queue.acquireLatest();
    probe_acquire = false;
    reenter = nullptr;
    assert(latest.status == ImageAcquireStatus::Ok &&
           latest.frame.dataspace == 12);
    // Two older pending frames were released outside the queue mutex. The
    // callback's reentrant acquire would deadlock if destruction happened
    // while that mutex was held.
    assert(reentrant_status == static_cast<int>(ImageAcquireStatus::NoBuffer));

    // Two images are acquired, so there is only one free slot. AOSP's
    // acquireLatest must retain FIFO behavior when its two-image margin is
    // unavailable rather than discarding pending frames.
    assert(queue.enqueue(Frame(13)) && queue.enqueue(Frame(14)));
    auto fifo = queue.acquireLatest();
    assert(fifo.status == ImageAcquireStatus::Ok && fifo.frame.dataspace == 13);
    assert(queue.acquireLatest().status == ImageAcquireStatus::MaxImages);

    held.frame.lease.reset();
    latest.frame.lease.reset();
    fifo.frame.lease.reset();
    assert(queue.releaseAcquired() && queue.releaseAcquired() &&
           queue.releaseAcquired());
    auto remaining = queue.acquireNext();
    assert(remaining.status == ImageAcquireStatus::Ok &&
           remaining.frame.dataspace == 14);
    remaining.frame.lease.reset();
    assert(queue.releaseAcquired());
    queue.close();
  }
  alarm(0);
  std::puts("image-consumer-queue: PASS FIFO latest-margin limits close "
            "retained-images reentry concurrent-close");
}
