#include "compat/window/native_window_buffer_queue.h"
#include <android/hardware_buffer.h>
#include <cassert>
#include <cerrno>
#include <cstdio>

struct AHardwareBuffer { int references = 1; uint32_t width = 0, height = 0; };
static int live_buffers = 0;
static int allocation_failure = -1;
static int closed_fences = 0;
static bool signaled = false;
extern "C" int AHardwareBuffer_allocate(const AHardwareBuffer_Desc* descriptor,
                                          AHardwareBuffer** output) {
  if (allocation_failure == 0) { *output = nullptr; return -ENOMEM; }
  if (allocation_failure > 0) --allocation_failure;
  *output = new AHardwareBuffer;
  (*output)->width = descriptor->width;
  (*output)->height = descriptor->height;
  ++live_buffers;
  return 0;
}
extern "C" void AHardwareBuffer_acquire(AHardwareBuffer* buffer) {
  assert(buffer && buffer->references > 0);
  ++buffer->references;
}
extern "C" void AHardwareBuffer_release(AHardwareBuffer* buffer) {
  assert(buffer && buffer->references > 0);
  if (--buffer->references == 0) { delete buffer; --live_buffers; }
}
extern "C" void* darwin_art_android_hardware_buffer_native_window_buffer(void* buffer) {
  return buffer;
}
extern "C" int darwin_art_bionic_socket_broker_close(int fd) {
  assert(fd >= 0); ++closed_fences; return 0;
}
extern "C" int sync_wait(int, int) { return signaled ? 0 : -1; }

using namespace darwin_art::window;
int main() {
  {
    NativeWindowBufferQueue queue(16, 16);
    assert(queue.UpdateGeometry(72, 128) == 0);
    assert(queue.Generation() == 1 && live_buffers == 0);
    NativeWindowDequeuedBuffer output;
    assert(queue.Dequeue(&output) == 0);
    assert(output.buffer->width == 72 && output.buffer->height == 128);
    assert(queue.Generation() == 1);
    assert(queue.UpdateGeometry(80, 160) == -EBUSY);
    assert(queue.Cancel(output.native_buffer, -1) == 0);
    allocation_failure = 1;
    assert(queue.UpdateGeometry(80, 160) == -ENOMEM);
    assert(queue.Generation() == 1);
    allocation_failure = -1;
    assert(queue.Dequeue(&output) == 0);
    assert(output.buffer->width == 72 && output.buffer->height == 128);
    assert(queue.Cancel(output.native_buffer, -1) == 0);
    assert(queue.UpdateGeometry(80, 160) == 0);
    assert(queue.Generation() == 2);
    assert(queue.Dequeue(&output) == 0);
    assert(output.buffer->width == 80 && output.buffer->height == 160);
    assert(queue.Cancel(output.native_buffer, -1) == 0);
  }
  assert(live_buffers == 0);
  {
    NativeWindowBufferQueue queue(16, 16);
    assert(queue.Generation() == 1);
    assert(queue.PrepareGeometry(16, 16) == 0);
    assert(queue.Generation() == 2);
    NativeWindowDequeuedBuffer output;
    assert(queue.Dequeue(&output) == 0 && queue.Generation() == 2);
    assert(queue.Cancel(output.native_buffer, -1) == 0);
  }
  assert(live_buffers == 0);
  {
    NativeWindowBufferQueue queue(16, 16);
    NativeWindowDequeuedBuffer a;
    allocation_failure = 1;
    assert(queue.Dequeue(&a) == -ENOMEM);
    assert(live_buffers == 0 && !a.buffer && !a.native_buffer);
    allocation_failure = -1;
    assert(queue.Dequeue(&a) == 0);
    assert(queue.Generation() == 1);
    assert(queue.PrepareGeometry(32, 32) == -EBUSY); // producer still dequeued
    NativeWindowQueuedBuffer first;
    assert(queue.Queue(a.native_buffer, &first) == 0);
    assert(first.buffer == a.buffer && first.token.frame == 1);
    assert(queue.NextFrame() == 2);
    assert(queue.PrepareGeometry(32, 32) == 0);
    assert(queue.Generation() != first.token.generation);
    NativeWindowDequeuedBuffer b;
    assert(queue.Dequeue(&b) == 0 && b.buffer != a.buffer);
    NativeWindowQueuedBuffer second;
    assert(queue.Queue(b.native_buffer, &second) == 0);
    assert(queue.PrepareGeometry(64, 64) == -EBUSY); // two held generations
    queue.ReturnCurrentSlot(first.token.slot, 7);
    assert(queue.PrepareGeometry(64, 64) == -EBUSY); // old GPU not finished
    signaled = true;
    assert(queue.PrepareGeometry(64, 64) == 0);
    queue.Return(second.token, -1, false);
    int closed = closed_fences;
    queue.Return(second.token, 81, true); // cannot quarantine a returned slot
    assert(closed_fences == ++closed);
    queue.ReturnCurrentSlot(second.token.slot, 85);
    assert(closed_fences == ++closed);
    queue.Return(first.token, 82, false); // reclaimed generation, stale
    assert(closed_fences == ++closed);
    NativeWindowDequeuedBuffer c;
    assert(queue.Dequeue(&c) == 0);
    NativeWindowQueuedBuffer third;
    assert(queue.Queue(c.native_buffer, &third) == 0);
    queue.Return(third.token, 83, true);
    assert(closed_fences == ++closed);
    for (int index = 0; index < 4; ++index) {
      NativeWindowDequeuedBuffer other;
      assert(queue.Dequeue(&other) == 0 && other.native_buffer != c.native_buffer);
      assert(queue.Cancel(other.native_buffer, -1) == 0);
    }
    queue.Return(third.token, -1, false);
    assert(queue.Cancel(nullptr, 84) == -EINVAL);
    assert(closed_fences == ++closed);
  }
  assert(live_buffers == 0);
  std::puts("native-window buffer queue: partial allocation/resize/exact return/fences/leases PASS");
}
