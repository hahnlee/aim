#include "native_window_software_queue.h"

#include "../darwin_angle_egl.h"
#include "../graphics/hardware_buffer_owner.h"

#include <algorithm>
#include <cerrno>
#include <cstdint>

extern "C" int sync_wait(int fd, int timeout_ms);
extern "C" int darwin_art_bionic_socket_broker_close(int fd);

namespace darwin_art::window {
namespace {
void CloseFence(int fence) {
  if (fence >= 0) (void)darwin_art_bionic_socket_broker_close(fence);
}
}  // namespace

int NativeWindowSoftwareQueue::Lock(ANativeWindow_Buffer* output,
                                    ARect* dirty) {
  if (output == nullptr || window_ == nullptr ||
      !darwin_art_android_ANativeWindow_has_queue_callback(window_))
    return -ENOTSUP;
  {
    std::lock_guard<std::mutex> lock(mutex_);
    if (mapped_.load(std::memory_order_relaxed) || locking_) return -EBUSY;
    locking_ = true;
    darwin_art_android_ANativeWindow_acquire(window_);
    producer_pinned_ = true;
  }

  AHardwareBuffer* buffer = nullptr;
  void* native_buffer = nullptr;
  int acquire_fence = -1;
  const int dequeue = darwin_art_android_ANativeWindow_dequeue_hardware_buffer(
      window_, &buffer, &native_buffer, &acquire_fence);
  if (dequeue != 0) {
    {
      std::lock_guard<std::mutex> lock(mutex_);
      locking_ = false;
      producer_pinned_ = false;
    }
    darwin_art_android_ANativeWindow_release(window_);
    return dequeue;
  }
  if (acquire_fence >= 0) {
    const int wait = sync_wait(acquire_fence, -1);
    CloseFence(acquire_fence);
    acquire_fence = -1;
    if (wait != 0) {
      (void)darwin_art_android_ANativeWindow_cancel_hardware_buffer(
          window_, native_buffer, -1);
      {
        std::lock_guard<std::mutex> lock(mutex_);
        locking_ = false;
        producer_pinned_ = false;
      }
      darwin_art_android_ANativeWindow_release(window_);
      return -EIO;
    }
  }

  AHardwareBuffer_Desc description{};
  AHardwareBuffer_describe(buffer, &description);
  if (description.width == 0 || description.height == 0) {
    (void)darwin_art_android_ANativeWindow_cancel_hardware_buffer(
        window_, native_buffer, -1);
    {
      std::lock_guard<std::mutex> lock(mutex_);
      locking_ = false;
      producer_pinned_ = false;
    }
    darwin_art_android_ANativeWindow_release(window_);
    return -EINVAL;
  }
  void* pixels = nullptr;
  const int map = AHardwareBuffer_lock(
      buffer, AHARDWAREBUFFER_USAGE_CPU_READ_OFTEN |
                  AHARDWAREBUFFER_USAGE_CPU_WRITE_OFTEN,
      -1, nullptr, &pixels);
  if (map != 0 || pixels == nullptr) {
    if (map == 0) (void)AHardwareBuffer_unlock(buffer, nullptr);
    (void)darwin_art_android_ANativeWindow_cancel_hardware_buffer(
        window_, native_buffer, -1);
    {
      std::lock_guard<std::mutex> lock(mutex_);
      locking_ = false;
      producer_pinned_ = false;
    }
    darwin_art_android_ANativeWindow_release(window_);
    return map == 0 ? -EIO : map;
  }

  const uint32_t stride = description.stride != 0 ? description.stride
                                                   : description.width;
  *output = ANativeWindow_Buffer{
      .width = static_cast<int32_t>(description.width),
      .height = static_cast<int32_t>(description.height),
      .stride = static_cast<int32_t>(stride),
      .format = static_cast<int32_t>(description.format),
      .bits = pixels,
      .reserved = {},
  };
  if (dirty != nullptr) {
    // The queue does not promise preserved contents across a generation
    // transition. Let Canvas redraw the complete real buffer extent.
    *dirty = ARect{0, 0, output->width, output->height};
  }
  {
    std::lock_guard<std::mutex> lock(mutex_);
    buffer_ = buffer;
    native_buffer_ = native_buffer;
    mapped_.store(true, std::memory_order_release);
    locking_ = false;
  }
  return 0;
}

int NativeWindowSoftwareQueue::UnlockAndPost() {
  AHardwareBuffer* buffer = nullptr;
  void* native_buffer = nullptr;
  void* window = nullptr;
  {
    std::lock_guard<std::mutex> lock(mutex_);
    if (window_ == nullptr || buffer_ == nullptr || native_buffer_ == nullptr ||
        !mapped_.load(std::memory_order_relaxed)) return -EINVAL;
    buffer = buffer_;
    native_buffer = native_buffer_;
    window = window_;
    buffer_ = nullptr;
    native_buffer_ = nullptr;
    mapped_.store(false, std::memory_order_release);
    producer_pinned_ = false;
  }
  int release_fence = -1;
  const int unlock = AHardwareBuffer_unlock(buffer, &release_fence);
  if (unlock != 0) {
    (void)darwin_art_android_ANativeWindow_cancel_hardware_buffer(
        window, native_buffer, release_fence);
    darwin_art_android_ANativeWindow_release(window);
    return unlock;
  }
  darwin_art_android_hardware_buffer_mark_cpu_rgba(buffer);
  const int queued = darwin_art_android_ANativeWindow_queue_hardware_buffer(
      window, native_buffer, release_fence);
  if (queued != 0) {
    (void)darwin_art_android_ANativeWindow_cancel_hardware_buffer(
        window, native_buffer, -1);
  }
  darwin_art_android_ANativeWindow_release(window);
  return queued;
}

int NativeWindowSoftwareQueue::Cancel() {
  AHardwareBuffer* buffer = nullptr;
  void* native_buffer = nullptr;
  void* window = nullptr;
  {
    std::lock_guard<std::mutex> lock(mutex_);
    if (window_ == nullptr || buffer_ == nullptr || native_buffer_ == nullptr ||
        !mapped_.load(std::memory_order_relaxed)) return -EINVAL;
    buffer = buffer_;
    native_buffer = native_buffer_;
    window = window_;
    buffer_ = nullptr;
    native_buffer_ = nullptr;
    mapped_.store(false, std::memory_order_release);
    producer_pinned_ = false;
  }
  int release_fence = -1;
  const int unlock = AHardwareBuffer_unlock(buffer, &release_fence);
  const int canceled = darwin_art_android_ANativeWindow_cancel_hardware_buffer(
      window, native_buffer, release_fence);
  darwin_art_android_ANativeWindow_release(window);
  return unlock != 0 ? unlock : canceled;
}

}  // namespace darwin_art::window
