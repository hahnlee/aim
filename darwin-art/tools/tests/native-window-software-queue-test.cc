#include "compat/window/native_window_software_queue.h"

#include <cassert>
#include <cerrno>
#include <condition_variable>
#include <cstdint>
#include <cstdio>
#include <cstring>
#include <mutex>
#include <thread>
#include <vector>

struct AHardwareBuffer {
  AHardwareBuffer_Desc description{};
  std::vector<uint8_t> pixels;
  bool mapped = false;
  bool cpu_rgba = false;
};

namespace {

struct ClientBuffer {
  AHardwareBuffer* buffer = nullptr;
};

struct FakeWindow {
  std::mutex mutex;
  std::condition_variable dequeue_cv;
  int references = 1;
  bool managed = true;
  bool has_queue_callback = true;
  bool block_dequeue = false;
  bool dequeue_entered = false;
  std::vector<ClientBuffer> pending;
  size_t next_pending = 0;
  int dequeue_result = 0;
  int dequeue_fence = -1;
  int wait_result = 0;
  int map_result = 0;
  int unlock_result = 0;
  int unlock_fence = 41;
  int acquire_calls = 0;
  int release_calls = 0;
  int dequeue_calls = 0;
  int queue_calls = 0;
  int cancel_calls = 0;
  int cancel_fence = -2;
  int published_fence = -2;
  void* published_native = nullptr;
  void* canceled_native = nullptr;
  int close_calls = 0;
  int last_closed_fence = -1;
  int wait_calls = 0;
  darwin_art::window::NativeWindowSoftwareQueue* callback_queue = nullptr;
  bool invoke_reentrant_callback = false;
  bool drop_owner_in_callback = false;
  int callback_lock_result = 999;
};

FakeWindow* g_wait_window = nullptr;

AHardwareBuffer* NewBuffer(uint32_t width, uint32_t height,
                           uint32_t stride = 0, uint32_t format = 1) {
  auto* buffer = new AHardwareBuffer();
  buffer->description.width = width;
  buffer->description.height = height;
  buffer->description.stride = stride == 0 ? width : stride;
  buffer->description.format = format;
  buffer->pixels.resize(static_cast<size_t>(buffer->description.stride) *
                        height * 4u);
  return buffer;
}

void DeleteBuffers(FakeWindow* window) {
  for (const ClientBuffer& client : window->pending) delete client.buffer;
  window->pending.clear();
}

void AddBuffer(FakeWindow* window, AHardwareBuffer* buffer) {
  std::lock_guard<std::mutex> lock(window->mutex);
  window->pending.push_back(ClientBuffer{buffer});
}

extern "C" bool darwin_art_android_ANativeWindow_has_queue_callback(
    void* opaque) {
  auto* window = static_cast<FakeWindow*>(opaque);
  std::lock_guard<std::mutex> lock(window->mutex);
  return window->managed && window->has_queue_callback;
}

extern "C" void darwin_art_android_ANativeWindow_acquire(void* opaque) {
  auto* window = static_cast<FakeWindow*>(opaque);
  std::lock_guard<std::mutex> lock(window->mutex);
  assert(window->managed);
  ++window->references;
  ++window->acquire_calls;
}

extern "C" void darwin_art_android_ANativeWindow_release(void* opaque) {
  auto* window = static_cast<FakeWindow*>(opaque);
  std::lock_guard<std::mutex> lock(window->mutex);
  assert(window->references > 0);
  --window->references;
  ++window->release_calls;
  if (window->references == 0) window->managed = false;
}

extern "C" int darwin_art_android_ANativeWindow_dequeue_hardware_buffer(
    void* opaque, AHardwareBuffer** output_buffer, void** output_native,
    int* output_fence) {
  auto* window = static_cast<FakeWindow*>(opaque);
  std::unique_lock<std::mutex> lock(window->mutex);
  ++window->dequeue_calls;
  if (window->block_dequeue) {
    window->dequeue_entered = true;
    window->dequeue_cv.notify_all();
    window->dequeue_cv.wait(lock, [&] { return !window->block_dequeue; });
  }
  if (window->dequeue_result != 0) return window->dequeue_result;
  if (window->next_pending >= window->pending.size()) return -EAGAIN;
  ClientBuffer& client = window->pending[window->next_pending++];
  *output_buffer = client.buffer;
  *output_native = &client;
  *output_fence = window->dequeue_fence;
  return 0;
}

extern "C" int darwin_art_android_ANativeWindow_cancel_hardware_buffer(
    void* opaque, void* native_buffer, int fence) {
  auto* window = static_cast<FakeWindow*>(opaque);
  std::lock_guard<std::mutex> lock(window->mutex);
  ++window->cancel_calls;
  window->cancel_fence = fence;
  window->canceled_native = native_buffer;
  return 0;
}

extern "C" int darwin_art_android_ANativeWindow_queue_hardware_buffer(
    void* opaque, void* native_buffer, int fence) {
  auto* window = static_cast<FakeWindow*>(opaque);
  {
    std::lock_guard<std::mutex> lock(window->mutex);
    ++window->queue_calls;
    window->published_native = native_buffer;
    window->published_fence = fence;
  }
  if (window->invoke_reentrant_callback && window->callback_queue != nullptr) {
    ANativeWindow_Buffer output{};
    window->callback_lock_result = window->callback_queue->Lock(&output, nullptr);
    if (window->drop_owner_in_callback)
      darwin_art_android_ANativeWindow_release(window);
  }
  return 0;
}

extern "C" int sync_wait(int fd, int) {
  (void)fd;
  assert(g_wait_window != nullptr);
  ++g_wait_window->wait_calls;
  return g_wait_window->wait_result;
}

extern "C" int darwin_art_bionic_socket_broker_close(int fd) {
  assert(g_wait_window != nullptr);
  ++g_wait_window->close_calls;
  g_wait_window->last_closed_fence = fd;
  return 0;
}

extern "C" void AHardwareBuffer_describe(const AHardwareBuffer* buffer,
                                           AHardwareBuffer_Desc* output) {
  *output = buffer->description;
}

extern "C" int AHardwareBuffer_lock(AHardwareBuffer* buffer, uint64_t, int32_t,
                                     const ARect*, void** output_pixels) {
  assert(g_wait_window != nullptr);
  if (g_wait_window->map_result != 0) return g_wait_window->map_result;
  buffer->mapped = true;
  *output_pixels = buffer->pixels.data();
  return 0;
}

extern "C" int AHardwareBuffer_unlock(AHardwareBuffer* buffer,
                                       int32_t* output_fence) {
  assert(g_wait_window != nullptr);
  buffer->mapped = false;
  *output_fence = g_wait_window->unlock_fence;
  return g_wait_window->unlock_result;
}

extern "C" void darwin_art_android_hardware_buffer_mark_cpu_rgba(
    AHardwareBuffer* buffer) {
  buffer->cpu_rgba = true;
}

void Install(FakeWindow* window) { g_wait_window = window; }

void TestPostPublishesMappedRgba() {
  FakeWindow window;
  auto* buffer = NewBuffer(4, 3, 8);
  AddBuffer(&window, buffer);
  Install(&window);
  darwin_art::window::NativeWindowSoftwareQueue queue(&window);
  ANativeWindow_Buffer output{};
  ARect dirty{99, 99, 99, 99};
  assert(queue.Lock(&output, &dirty) == 0);
  assert(queue.HasLock() && output.width == 4 && output.height == 3 &&
         output.stride == 8 && output.format == 1 && output.bits == buffer->pixels.data());
  assert(dirty.left == 0 && dirty.top == 0 && dirty.right == 4 && dirty.bottom == 3);
  std::memset(output.bits, 0x5A, buffer->pixels.size());
  assert(queue.UnlockAndPost() == 0);
  assert(!queue.HasLock() && window.queue_calls == 1 && window.cancel_calls == 0 &&
         window.published_fence == window.unlock_fence && buffer->cpu_rgba &&
         window.references == 1 && window.acquire_calls == 1 &&
         window.release_calls == 1);
  assert(buffer->pixels[0] == 0x5A);
  DeleteBuffers(&window);
}

void TestCancelTransfersUnlockFence() {
  FakeWindow window;
  auto* buffer = NewBuffer(2, 2);
  AddBuffer(&window, buffer);
  Install(&window);
  darwin_art::window::NativeWindowSoftwareQueue queue(&window);
  ANativeWindow_Buffer output{};
  assert(queue.Lock(&output, nullptr) == 0);
  assert(queue.Cancel() == 0);
  assert(!queue.HasLock() && window.queue_calls == 0 && window.cancel_calls == 1 &&
         window.cancel_fence == window.unlock_fence && window.references == 1);
  DeleteBuffers(&window);
}

void TestFailureDoesNotPublishOrLeakPin() {
  {
    FakeWindow window;
    window.dequeue_result = -EIO;
    Install(&window);
    darwin_art::window::NativeWindowSoftwareQueue queue(&window);
    ANativeWindow_Buffer output{};
    assert(queue.Lock(&output, nullptr) == -EIO);
    assert(!queue.HasLock() && window.queue_calls == 0 && window.references == 1 &&
           window.acquire_calls == 1 && window.release_calls == 1);
  }
  {
    FakeWindow window;
    window.dequeue_fence = 9;
    window.wait_result = -EIO;
    auto* buffer = NewBuffer(2, 2);
    AddBuffer(&window, buffer);
    Install(&window);
    darwin_art::window::NativeWindowSoftwareQueue queue(&window);
    ANativeWindow_Buffer output{};
    assert(queue.Lock(&output, nullptr) == -EIO);
    assert(!queue.HasLock() && window.wait_calls == 1 && window.close_calls == 1 &&
           window.last_closed_fence == 9 && window.cancel_calls == 1 &&
           window.cancel_fence == -1 && window.references == 1);
    DeleteBuffers(&window);
  }
  {
    FakeWindow window;
    window.map_result = -EIO;
    auto* buffer = NewBuffer(2, 2);
    AddBuffer(&window, buffer);
    Install(&window);
    darwin_art::window::NativeWindowSoftwareQueue queue(&window);
    ANativeWindow_Buffer output{};
    assert(queue.Lock(&output, nullptr) == -EIO);
    assert(!queue.HasLock() && window.cancel_calls == 1 && window.queue_calls == 0 &&
           window.references == 1);
    DeleteBuffers(&window);
  }
  {
    FakeWindow window;
    auto* buffer = NewBuffer(0, 2);
    AddBuffer(&window, buffer);
    Install(&window);
    darwin_art::window::NativeWindowSoftwareQueue queue(&window);
    ANativeWindow_Buffer output{};
    assert(queue.Lock(&output, nullptr) == -EINVAL);
    assert(!queue.HasLock() && window.cancel_calls == 1 && window.references == 1);
    DeleteBuffers(&window);
  }
}

void TestConcurrentLockRejectsDuringDequeue() {
  FakeWindow window;
  auto* buffer = NewBuffer(2, 2);
  AddBuffer(&window, buffer);
  window.block_dequeue = true;
  Install(&window);
  darwin_art::window::NativeWindowSoftwareQueue queue(&window);
  int first_result = 999;
  std::thread first([&] {
    ANativeWindow_Buffer output{};
    first_result = queue.Lock(&output, nullptr);
  });
  {
    std::unique_lock<std::mutex> lock(window.mutex);
    window.dequeue_cv.wait(lock, [&] { return window.dequeue_entered; });
  }
  ANativeWindow_Buffer second_output{};
  assert(queue.Lock(&second_output, nullptr) == -EBUSY);
  {
    std::lock_guard<std::mutex> lock(window.mutex);
    window.block_dequeue = false;
  }
  window.dequeue_cv.notify_all();
  first.join();
  assert(first_result == 0);
  assert(queue.Cancel() == 0 && window.references == 1);
  DeleteBuffers(&window);
}

void TestReentrantCallbackAndOwnerDrop() {
  FakeWindow window;
  auto* first_buffer = NewBuffer(2, 2);
  auto* second_buffer = NewBuffer(3, 1);
  AddBuffer(&window, first_buffer);
  AddBuffer(&window, second_buffer);
  Install(&window);
  darwin_art::window::NativeWindowSoftwareQueue queue(&window);
  window.callback_queue = &queue;
  window.invoke_reentrant_callback = true;
  window.drop_owner_in_callback = true;
  ANativeWindow_Buffer output{};
  assert(queue.Lock(&output, nullptr) == 0);
  assert(queue.UnlockAndPost() == 0);
  assert(window.callback_lock_result == 0 && queue.HasLock() &&
         window.references == 1 && window.managed && window.queue_calls == 1);
  assert(queue.Cancel() == 0);
  assert(!queue.HasLock() && window.references == 0 && !window.managed);
  DeleteBuffers(&window);
}

}  // namespace

// NativeWindowSoftwareQueue uses the real production translation unit.  The
// doubles above stop at its ANativeWindow/AHardwareBuffer/fence boundaries.
int main() {
  TestPostPublishesMappedRgba();
  TestCancelTransfersUnlockFence();
  TestFailureDoesNotPublishOrLeakPin();
  TestConcurrentLockRejectsDuringDequeue();
  TestReentrantCallbackAndOwnerDrop();
  std::puts("native-window-software-queue: PASS lock/post/cancel/failure/reentry");
}
