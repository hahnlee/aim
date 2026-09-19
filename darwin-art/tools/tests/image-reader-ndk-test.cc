#include "compat/media/image_reader_ndk.h"
#include "compat/darwin_angle_egl.h"

#include <android/hardware_buffer.h>
#include <media/NdkImage.h>
#include <media/NdkImageReader.h>

#include <cassert>
#include <chrono>
#include <condition_variable>
#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <fcntl.h>
#include <mutex>
#include <poll.h>
#include <thread>
#include <unistd.h>
#include <vector>

namespace {

using namespace std::chrono_literals;

struct MockBuffer {
  std::mutex mutex;
  int references = 1;
  AHardwareBuffer_Desc description{};
};

struct MockWindow {
  std::mutex mutex;
  int references = 1;
  void (*callback)(void*, AHardwareBuffer*, int32_t, uint64_t, uint64_t, int,
                   int32_t) = nullptr;
  void* callback_context = nullptr;
  void (*release_context)(void*) = nullptr;
  bool deferred_release = false;
  void (*pending_release_context)(void*) = nullptr;
  void* pending_release_opaque = nullptr;
  struct Returned {
    int32_t slot;
    int fence;
  };
  std::vector<Returned> returned;
};

std::mutex g_mock_mutex;
std::condition_variable g_mock_cv;
std::vector<MockBuffer*> g_buffers;

MockBuffer* NewBuffer(uint32_t width, uint32_t height, int32_t format,
                      uint64_t usage, uint32_t id) {
  auto* buffer = new MockBuffer;
  buffer->description.width = width;
  buffer->description.height = height;
  buffer->description.layers = 1;
  buffer->description.format = format;
  buffer->description.usage = usage;
  buffer->description.stride = width;
  (void)id;
  {
    std::lock_guard<std::mutex> lock(g_mock_mutex);
    g_buffers.push_back(buffer);
  }
  return buffer;
}

void Emit(MockWindow* window, MockBuffer* buffer, int32_t slot, int fence,
          int32_t dataspace = 0) {
  void (*callback)(void*, AHardwareBuffer*, int32_t, uint64_t, uint64_t, int,
                   int32_t) = nullptr;
  void* context = nullptr;
  {
    std::lock_guard<std::mutex> lock(window->mutex);
    callback = window->callback;
    context = window->callback_context;
  }
  assert(callback != nullptr);
  callback(context, reinterpret_cast<AHardwareBuffer*>(buffer), slot, 1, 1,
           fence, dataspace);
}

bool Wait(std::condition_variable& cv, std::mutex& mutex,
          const auto& predicate, std::chrono::milliseconds timeout = 1500ms) {
  std::unique_lock<std::mutex> lock(mutex);
  return cv.wait_for(lock, timeout, predicate);
}

template <typename Function>
Function Lookup(const char* name) {
  void* address = darwin_art::media::ImageReaderSymbol(name);
  assert(address != nullptr && name != nullptr);
  return reinterpret_cast<Function>(address);
}

using ReaderNew = media_status_t (*)(int32_t, int32_t, int32_t, uint64_t,
                                      int32_t, AImageReader**);
using ReaderDelete = void (*)(AImageReader*);
using ReaderWindow = media_status_t (*)(AImageReader*, ANativeWindow**);
using ReaderListener = media_status_t (*)(AImageReader*,
                                           AImageReader_ImageListener*);
using ReaderAcquire = media_status_t (*)(AImageReader*, AImage**, int*);
using ReaderAcquireSync = media_status_t (*)(AImageReader*, AImage**);
using ImageDelete = void (*)(AImage*);
using ImageDeleteAsync = void (*)(AImage*, int);
using ImageWidth = media_status_t (*)(const AImage*, int32_t*);
using ImageBuffer = media_status_t (*)(const AImage*, AHardwareBuffer**);

ReaderNew NewReader() { return Lookup<ReaderNew>("AImageReader_newWithUsage"); }
ReaderDelete DeleteReader() {
  return Lookup<ReaderDelete>("AImageReader_delete");
}
ReaderWindow GetWindow() {
  return Lookup<ReaderWindow>("AImageReader_getWindow");
}
ReaderListener SetListener() {
  return Lookup<ReaderListener>("AImageReader_setImageListener");
}
ReaderAcquire AcquireNextAsync() {
  return Lookup<ReaderAcquire>("AImageReader_acquireNextImageAsync");
}
ReaderAcquire AcquireLatestAsync() {
  return Lookup<ReaderAcquire>("AImageReader_acquireLatestImageAsync");
}
ReaderAcquireSync AcquireNextSync() {
  return Lookup<ReaderAcquireSync>("AImageReader_acquireNextImage");
}
ImageDelete DeleteImage() { return Lookup<ImageDelete>("AImage_delete"); }
ImageDeleteAsync DeleteImageAsync() {
  return Lookup<ImageDeleteAsync>("AImage_deleteAsync");
}
ImageWidth GetWidth() { return Lookup<ImageWidth>("AImage_getWidth"); }
ImageBuffer GetBuffer() {
  return Lookup<ImageBuffer>("AImage_getHardwareBuffer");
}

void CheckStatus(media_status_t actual, media_status_t expected) {
  if (actual != expected) {
    std::fprintf(stderr, "unexpected media status %d (want %d)\n",
                 static_cast<int>(actual), static_cast<int>(expected));
    std::abort();
  }
}

struct ListenerState {
  std::mutex mutex;
  std::condition_variable cv;
  int calls = 0;
  std::thread::id callback_thread;
  AImage* acquired = nullptr;
  AImageReader* reader = nullptr;
  bool delete_reader = false;
};

void OnImageAvailable(void* context, AImageReader* reader) {
  auto* state = static_cast<ListenerState*>(context);
  AImage* image = nullptr;
  int fence = -1;
  if (!state->delete_reader) {
    (void)AcquireNextAsync()(reader, &image, &fence);
    if (fence >= 0) close(fence);
  }
  {
    std::lock_guard<std::mutex> lock(state->mutex);
    state->calls++;
    state->callback_thread = std::this_thread::get_id();
    state->acquired = image;
  }
  if (state->delete_reader) DeleteReader()(reader);
  state->cv.notify_all();
}

MockWindow* MakeReader(int32_t max_images, AImageReader** out) {
  CheckStatus(NewReader()(16, 12, AIMAGE_FORMAT_RGBA_8888,
                          AHARDWAREBUFFER_USAGE_GPU_SAMPLED_IMAGE, max_images,
                          out), AMEDIA_OK);
  ANativeWindow* native_window = nullptr;
  CheckStatus(GetWindow()(*out, &native_window), AMEDIA_OK);
  assert(native_window != nullptr);
  return reinterpret_cast<MockWindow*>(native_window);
}

void TestFifoAndLatestMargin() {
  AImageReader* reader = nullptr;
  MockWindow* window = MakeReader(3, &reader);
  MockBuffer* first = NewBuffer(16, 12, AHARDWAREBUFFER_FORMAT_R8G8B8A8_UNORM,
                                0, 1);
  MockBuffer* second = NewBuffer(16, 12, AHARDWAREBUFFER_FORMAT_R8G8B8A8_UNORM,
                                 0, 2);
  MockBuffer* third = NewBuffer(16, 12, AHARDWAREBUFFER_FORMAT_R8G8B8A8_UNORM,
                                0, 3);
  Emit(window, first, 1, -1);
  Emit(window, second, 2, -1);
  Emit(window, third, 3, -1);
  AImage* image = nullptr;
  int fence = -1;
  CheckStatus(AcquireLatestAsync()(reader, &image, &fence), AMEDIA_OK);
  assert(image != nullptr && fence == -1);
  AHardwareBuffer* returned = nullptr;
  CheckStatus(GetBuffer()(image, &returned), AMEDIA_OK);
  assert(returned == reinterpret_cast<AHardwareBuffer*>(third));
  DeleteImage()(image);
  DeleteReader()(reader);

  reader = nullptr;
  window = MakeReader(2, &reader);
  Emit(window, first, 11, -1);
  CheckStatus(AcquireNextAsync()(reader, &image, &fence), AMEDIA_OK);
  assert(image != nullptr);
  AImage* held = image;
  Emit(window, second, 12, -1);
  Emit(window, third, 13, -1);
  CheckStatus(AcquireLatestAsync()(reader, &image, &fence), AMEDIA_OK);
  CheckStatus(GetBuffer()(image, &returned), AMEDIA_OK);
  assert(returned == reinterpret_cast<AHardwareBuffer*>(second));
  DeleteImage()(image);
  CheckStatus(AcquireNextAsync()(reader, &image, &fence), AMEDIA_OK);
  CheckStatus(GetBuffer()(image, &returned), AMEDIA_OK);
  assert(returned == reinterpret_cast<AHardwareBuffer*>(third));
  DeleteImage()(image);
  DeleteImage()(held);
  DeleteReader()(reader);
}

void TestListenerAndDeletion() {
  AImageReader* reader = nullptr;
  MockWindow* window = MakeReader(2, &reader);
  ListenerState state;
  state.reader = reader;
  AImageReader_ImageListener listener{};
  listener.context = &state;
  listener.onImageAvailable = &OnImageAvailable;
  CheckStatus(SetListener()(reader, &listener), AMEDIA_OK);
  MockBuffer* buffer = NewBuffer(16, 12, AHARDWAREBUFFER_FORMAT_R8G8B8A8_UNORM,
                                 0, 20);
  const auto caller_thread = std::this_thread::get_id();
  Emit(window, buffer, 20, -1);
  assert(Wait(state.cv, state.mutex, [&] { return state.calls == 1; }));
  assert(state.callback_thread != caller_thread);
  assert(state.acquired != nullptr);
  DeleteImage()(state.acquired);
  DeleteReader()(reader);

  reader = nullptr;
  window = MakeReader(1, &reader);
  ListenerState delete_state;
  delete_state.delete_reader = true;
  AImageReader_ImageListener delete_listener{};
  delete_listener.context = &delete_state;
  delete_listener.onImageAvailable = &OnImageAvailable;
  CheckStatus(SetListener()(reader, &delete_listener), AMEDIA_OK);
  Emit(window, buffer, 21, -1);
  assert(Wait(delete_state.cv, delete_state.mutex,
              [&] { return delete_state.calls == 1; }));
}

void TestReaderDeleteInvalidatesImage() {
  AImageReader* reader = nullptr;
  MockWindow* window = MakeReader(1, &reader);
  MockBuffer* buffer = NewBuffer(16, 12, AHARDWAREBUFFER_FORMAT_R8G8B8A8_UNORM,
                                 0, 30);
  Emit(window, buffer, 30, -1);
  AImage* image = nullptr;
  int fence = -1;
  CheckStatus(AcquireNextAsync()(reader, &image, &fence), AMEDIA_OK);
  DeleteReader()(reader);
  int32_t width = 0;
  CheckStatus(GetWidth()(image, &width), AMEDIA_ERROR_INVALID_OBJECT);
  DeleteImage()(image);
}

void TestDeferredObserverClose() {
  AImageReader* reader = nullptr;
  MockWindow* window = MakeReader(1, &reader);
  // Keep the borrowed window alive while a transport emulates an in-flight
  // observer callback. The reader must complete close without waiting on the
  // producer's deferred context destructor.
  darwin_art_android_ANativeWindow_acquire(window);
  {
    std::lock_guard<std::mutex> lock(window->mutex);
    window->deferred_release = true;
  }
  void (*captured_callback)(void*, AHardwareBuffer*, int32_t, uint64_t,
                            uint64_t, int, int32_t) = nullptr;
  void* captured_context = nullptr;
  {
    std::lock_guard<std::mutex> lock(window->mutex);
    captured_callback = window->callback;
    captured_context = window->callback_context;
  }
  assert(captured_callback != nullptr && captured_context != nullptr);
  int fence_pipe[2];
  assert(pipe(fence_pipe) == 0);
  MockBuffer* buffer = NewBuffer(16, 12, AHARDWAREBUFFER_FORMAT_R8G8B8A8_UNORM,
                                 0, 35);
  DeleteReader()(reader);
  // A producer may already have captured the observer callback when close
  // unregisters it. The retained observer must return that exact slot/fence,
  // rather than merely closing the descriptor after the reader is gone.
  captured_callback(captured_context, reinterpret_cast<AHardwareBuffer*>(buffer),
                    35, 1, 1, fence_pipe[0], 0);
  {
    std::lock_guard<std::mutex> lock(window->mutex);
    assert(window->pending_release_context != nullptr);
    assert(window->returned.size() == 1);
    assert(window->returned[0].slot == 35);
    assert(window->returned[0].fence == fence_pipe[0]);
  }
  void (*release_context)(void*) = nullptr;
  void* opaque = nullptr;
  {
    std::lock_guard<std::mutex> lock(window->mutex);
    release_context = window->pending_release_context;
    opaque = window->pending_release_opaque;
    window->pending_release_context = nullptr;
    window->pending_release_opaque = nullptr;
  }
  release_context(opaque);
  darwin_art_android_ANativeWindow_release(window);
  close(fence_pipe[1]);
}

void TestAsyncAndSyncFences() {
  AImageReader* reader = nullptr;
  MockWindow* window = MakeReader(2, &reader);
  int acquire_pipe[2];
  assert(pipe(acquire_pipe) == 0);
  MockBuffer* buffer = NewBuffer(16, 12, AHARDWAREBUFFER_FORMAT_R8G8B8A8_UNORM,
                                 0, 40);
  Emit(window, buffer, 40, acquire_pipe[0]);
  AImage* image = nullptr;
  int fence = -1;
  CheckStatus(AcquireNextAsync()(reader, &image, nullptr),
              AMEDIA_ERROR_INVALID_PARAMETER);
  assert(image == nullptr);
  // The failed call must not consume the queued frame or its fence.
  CheckStatus(AcquireNextAsync()(reader, &image, &fence), AMEDIA_OK);
  // The async API transfers the owned acquire descriptor, rather than
  // manufacturing a second readiness object.
  assert(fence == acquire_pipe[0]);
  struct pollfd pfd{fence, POLLIN, 0};
  assert(poll(&pfd, 1, 0) == 0);
  assert(write(acquire_pipe[1], "x", 1) == 1);
  assert(poll(&pfd, 1, 1000) == 1);
  close(fence);
  close(acquire_pipe[1]);
  int release_pipe[2];
  assert(pipe(release_pipe) == 0);
  DeleteImageAsync()(image, release_pipe[0]);
  close(release_pipe[1]);

  int sync_pipe[2];
  assert(pipe(sync_pipe) == 0);
  Emit(window, buffer, 41, sync_pipe[0]);
  std::mutex mutex;
  std::condition_variable cv;
  AImage* sync_image = nullptr;
  bool finished = false;
  std::thread waiter([&] {
    int status = AcquireNextSync()(reader, &sync_image);
    assert(status == AMEDIA_OK);
    {
      std::lock_guard<std::mutex> lock(mutex);
      finished = true;
    }
    cv.notify_one();
  });
  {
    std::lock_guard<std::mutex> lock(mutex);
    assert(!finished);
  }
  assert(write(sync_pipe[1], "y", 1) == 1);
  assert(Wait(cv, mutex, [&] { return finished; }));
  waiter.join();
  close(sync_pipe[1]);
  DeleteImage()(sync_image);
  DeleteReader()(reader);
}

void TestPendingCloseAndFencePreservation() {
  AImageReader* reader = nullptr;
  MockWindow* window = MakeReader(3, &reader);
  int old_pipe[2];
  assert(pipe(old_pipe) == 0);
  MockBuffer* old = NewBuffer(16, 12, AHARDWAREBUFFER_FORMAT_R8G8B8A8_UNORM,
                              0, 50);
  MockBuffer* newest = NewBuffer(16, 12, AHARDWAREBUFFER_FORMAT_R8G8B8A8_UNORM,
                                 0, 51);
  Emit(window, old, 50, old_pipe[0]);
  Emit(window, newest, 51, -1);
  AImage* image = nullptr;
  int fence = -1;
  CheckStatus(AcquireLatestAsync()(reader, &image, &fence), AMEDIA_OK);
  assert(fence == -1);
  AHardwareBuffer* returned = nullptr;
  CheckStatus(GetBuffer()(image, &returned), AMEDIA_OK);
  assert(returned == reinterpret_cast<AHardwareBuffer*>(newest));
  DeleteImage()(image);
  close(old_pipe[1]);
  darwin_art_android_ANativeWindow_acquire(window);
  DeleteReader()(reader);
  {
    std::lock_guard<std::mutex> lock(window->mutex);
    assert(window->returned.size() == 2);
    assert(window->returned[0].slot == 50);
    assert(window->returned[0].fence == old_pipe[0]);
  }
  darwin_art_android_ANativeWindow_release(window);

  reader = nullptr;
  window = MakeReader(2, &reader);
  darwin_art_android_ANativeWindow_acquire(window);
  MockBuffer* pending = NewBuffer(16, 12, AHARDWAREBUFFER_FORMAT_R8G8B8A8_UNORM,
                                  0, 60);
  Emit(window, pending, 60, -1);
  DeleteReader()(reader);
  {
    std::lock_guard<std::mutex> lock(window->mutex);
    assert(window->returned.size() == 1);
  }
  darwin_art_android_ANativeWindow_release(window);
}

void DestroyBuffers() {
  std::vector<MockBuffer*> buffers;
  {
    std::lock_guard<std::mutex> lock(g_mock_mutex);
    buffers.swap(g_buffers);
  }
  for (MockBuffer* buffer : buffers) {
    std::lock_guard<std::mutex> lock(buffer->mutex);
    assert(buffer->references == 1);
    delete buffer;
  }
}

}  // namespace

extern "C" {

void* darwin_art_android_ANativeWindow_create(int32_t, int32_t, int32_t) {
  return new MockWindow;
}
void darwin_art_android_ANativeWindow_acquire(void* window) {
  auto* native_window = static_cast<MockWindow*>(window);
  std::lock_guard<std::mutex> lock(native_window->mutex);
  ++native_window->references;
}
void darwin_art_android_ANativeWindow_release(void* window) {
  auto* native_window = static_cast<MockWindow*>(window);
  void (*release_context)(void*) = nullptr;
  void* context = nullptr;
  {
    std::lock_guard<std::mutex> lock(native_window->mutex);
    if (--native_window->references != 0) return;
    release_context = native_window->release_context;
    context = native_window->callback_context;
  }
  if (release_context != nullptr) release_context(context);
  delete native_window;
}
bool darwin_art_android_ANativeWindow_set_owned_queue_callback(
    void* window, DarwinArtAndroidNativeWindowQueueCallback callback,
    void* context, void (*release_context)(void*)) {
  auto* native_window = static_cast<MockWindow*>(window);
  void (*old_release)(void*) = nullptr;
  void* old_context = nullptr;
  bool defer_release = false;
  {
    std::lock_guard<std::mutex> lock(native_window->mutex);
    old_release = native_window->release_context;
    old_context = native_window->callback_context;
    defer_release = native_window->deferred_release;
    native_window->callback = callback;
    native_window->callback_context = context;
    native_window->release_context = release_context;
  }
  if (old_release != nullptr && defer_release) {
    std::lock_guard<std::mutex> lock(native_window->mutex);
    native_window->pending_release_context = old_release;
    native_window->pending_release_opaque = old_context;
  } else if (old_release != nullptr) {
    old_release(old_context);
  }
  return true;
}
void darwin_art_android_ANativeWindow_release_consumer_slot(void* window,
                                                             int32_t slot,
                                                             int release_fence) {
  auto* native_window = static_cast<MockWindow*>(window);
  {
    std::lock_guard<std::mutex> lock(native_window->mutex);
    native_window->returned.push_back({slot, release_fence});
  }
  if (release_fence >= 0) close(release_fence);
}
int darwin_art_bionic_socket_broker_close(int fd) { return close(fd); }
int sync_wait(int fd, int timeout_ms) {
  struct pollfd pfd{fd, POLLIN, 0};
  const int result = poll(&pfd, 1, timeout_ms);
  return result == 1 && (pfd.revents & (POLLIN | POLLHUP)) != 0 ? 0 : -1;
}

void AHardwareBuffer_acquire(AHardwareBuffer* buffer) {
  auto* mock = reinterpret_cast<MockBuffer*>(buffer);
  std::lock_guard<std::mutex> lock(mock->mutex);
  ++mock->references;
}
void AHardwareBuffer_release(AHardwareBuffer* buffer) {
  auto* mock = reinterpret_cast<MockBuffer*>(buffer);
  std::lock_guard<std::mutex> lock(mock->mutex);
  assert(mock->references > 0);
  --mock->references;
}
void AHardwareBuffer_describe(const AHardwareBuffer* buffer,
                              AHardwareBuffer_Desc* out) {
  assert(buffer != nullptr && out != nullptr);
  *out = reinterpret_cast<const MockBuffer*>(buffer)->description;
}

}  // extern "C"

int main() {
  TestFifoAndLatestMargin();
  TestListenerAndDeletion();
  TestReaderDeleteInvalidatesImage();
  TestDeferredObserverClose();
  TestAsyncAndSyncFences();
  TestPendingCloseAndFencePreservation();
  DestroyBuffers();
  std::puts("image-reader-ndk: PASS fifo/latest listener invalidation fences");
  return 0;
}
