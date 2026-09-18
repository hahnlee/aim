#include "../../compat/input/surface_input_sink.h"
#include "../../probes/fixture_input_queue.h"

#include <atomic>
#include <cassert>
#include <cstdint>
#include <memory>
#include <thread>

namespace {

struct Lifetime {
  std::atomic<int> retains{0};
  std::atomic<int> releases{0};
};

void retain(void* opaque) {
  static_cast<Lifetime*>(opaque)->retains.fetch_add(1);
}
void release(void* opaque) {
  static_cast<Lifetime*>(opaque)->releases.fetch_add(1);
}
DarwinArtSurfaceInputResult pointer_sink(
    void*, const DarwinArtPointerEventV2*) {
  return DARWIN_ART_SURFACE_INPUT_QUEUED;
}
DarwinArtSurfaceInputResult no_target_pointer_sink(
    void*, const DarwinArtPointerEventV2*) {
  return DARWIN_ART_SURFACE_INPUT_NO_TARGET;
}
DarwinArtSurfaceInputResult key_sink(void*, const DarwinArtKeyEventV1*) {
  return DARWIN_ART_SURFACE_INPUT_QUEUED;
}

void test_retained_snapshot_lifetime() {
  Lifetime lifetime;
  const DarwinArtSurfaceInputSink sink{
      .version = 1,
      .size = sizeof(DarwinArtSurfaceInputSink),
      .context = &lifetime,
      .retain_context = &retain,
      .release_context = &release,
      .pointer = &pointer_sink,
      .key = &key_sink,
  };
  auto binding = MakeDarwinArtSurfaceInputSinkBinding(sink);
  assert(lifetime.retains.load() == 1);
  auto snapshot = binding;
  binding.reset();
  assert(lifetime.releases.load() == 0);
  snapshot.reset();
  assert(lifetime.releases.load() == 1);
}

void test_no_target_has_no_fixture_replay() {
  Lifetime lifetime;
  const DarwinArtSurfaceInputSink sink{
      .version = 1,
      .size = sizeof(DarwinArtSurfaceInputSink),
      .context = &lifetime,
      .retain_context = &retain,
      .release_context = &release,
      .pointer = &no_target_pointer_sink,
      .key = &key_sink,
  };
  auto binding = MakeDarwinArtSurfaceInputSinkBinding(sink);
  DarwinArtPointerEventV2 event{};
  // A terminal no-target result is explicit and has no secondary queue path;
  // a later fixture drain therefore cannot replay this packet.
  assert(binding->sink.pointer(binding->sink.context, &event) ==
         DARWIN_ART_SURFACE_INPUT_NO_TARGET);
  darwin_art_graphics_fixture::FixtureInputQueue empty_queue;
  darwin_art_graphics_fixture::FixtureQueuedInput packet;
  assert(!empty_queue.take(&packet));
  assert(empty_queue.acknowledge_if_empty());
}

void test_fixture_queue_order_and_ack() {
  darwin_art_graphics_fixture::FixtureInputQueue queue;
  darwin_art_graphics_fixture::FixtureQueuedInput packet;
  // A no-focus submission is represented by no sink call, so a later focus
  // cannot discover or replay it.
  assert(!queue.take(&packet));
  assert(queue.acknowledge_if_empty());

  DarwinArtPointerEventV2 down{};
  down.action = DARWIN_ART_POINTER_DOWN;
  down.sequence = 10;
  DarwinArtKeyEventV1 key_down{};
  key_down.action = 0;
  key_down.sequence = 20;
  DarwinArtPointerEventV2 cancel{};
  cancel.action = DARWIN_ART_POINTER_CANCEL;
  cancel.sequence = 30;
  DarwinArtKeyEventV1 key_up{};
  key_up.action = 1;
  key_up.sequence = 40;
  assert(queue.enqueue_pointer(down) == DARWIN_ART_SURFACE_INPUT_QUEUED);
  assert(queue.enqueue_key(key_down) == DARWIN_ART_SURFACE_INPUT_QUEUED);
  assert(queue.enqueue_pointer(cancel) == DARWIN_ART_SURFACE_INPUT_QUEUED);
  assert(queue.enqueue_key(key_up) == DARWIN_ART_SURFACE_INPUT_QUEUED);
  assert(queue.pending());

  assert(queue.take(&packet) && packet.kind ==
             darwin_art_graphics_fixture::FixtureQueuedInput::Kind::kPointer &&
         packet.pointer.action == DARWIN_ART_POINTER_DOWN &&
         packet.pointer.sequence == 10);
  assert(queue.take(&packet) && packet.kind ==
             darwin_art_graphics_fixture::FixtureQueuedInput::Kind::kKey &&
         packet.key.sequence == 20);
  assert(queue.take(&packet) && packet.kind ==
             darwin_art_graphics_fixture::FixtureQueuedInput::Kind::kPointer &&
         packet.pointer.action == DARWIN_ART_POINTER_CANCEL &&
         packet.pointer.sequence == 30);
  assert(queue.take(&packet) && packet.kind ==
             darwin_art_graphics_fixture::FixtureQueuedInput::Kind::kKey &&
         packet.key.sequence == 40);
  assert(!queue.take(&packet));
  assert(queue.acknowledge_if_empty());
  assert(!queue.pending());
}

void test_concurrent_wake_ack_does_not_drop_packets() {
  darwin_art_graphics_fixture::FixtureInputQueue queue;
  std::atomic<bool> producer_done{false};
  std::atomic<int> consumed{0};
  std::thread producer([&] {
    for (uint64_t sequence = 1; sequence <= 1000; ++sequence) {
      DarwinArtKeyEventV1 event{};
      event.sequence = sequence;
      while (queue.enqueue_key(event) == DARWIN_ART_SURFACE_INPUT_BACKPRESSURED) {
        std::this_thread::yield();
      }
    }
    producer_done.store(true, std::memory_order_release);
  });
  std::thread consumer([&] {
    darwin_art_graphics_fixture::FixtureQueuedInput packet;
    for (;;) {
      if (queue.take(&packet)) {
        ++consumed;
        continue;
      }
      if (producer_done.load(std::memory_order_acquire)) break;
      std::this_thread::yield();
    }
    while (queue.take(&packet)) ++consumed;
    assert(queue.acknowledge_if_empty());
  });
  producer.join();
  consumer.join();
  assert(consumed.load() == 1000);
  assert(!queue.pending());
}

}  // namespace

int main() {
  test_retained_snapshot_lifetime();
  test_no_target_has_no_fixture_replay();
  test_fixture_queue_order_and_ack();
  test_concurrent_wake_ack_does_not_drop_packets();
  return 0;
}
