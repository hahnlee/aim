#pragma once

#include <android/hardware_buffer.h>
#include <android/native_window.h>

#include <atomic>
#include <mutex>

namespace darwin_art::window {

// Owns one observer-backed ANativeWindow software lock. The producer token and
// mapped hardware buffer remain paired until UnlockAndPost or Cancel.
class NativeWindowSoftwareQueue {
 public:
  explicit NativeWindowSoftwareQueue(void* window) : window_(window) {}
  NativeWindowSoftwareQueue(const NativeWindowSoftwareQueue&) = delete;
  NativeWindowSoftwareQueue& operator=(const NativeWindowSoftwareQueue&) = delete;

  int Lock(ANativeWindow_Buffer* output, ARect* dirty);
  int UnlockAndPost();
  int Cancel();
  bool HasLock() const { return mapped_.load(std::memory_order_acquire); }

 private:
  void* window_ = nullptr;
  mutable std::mutex mutex_;
  AHardwareBuffer* buffer_ = nullptr;
  void* native_buffer_ = nullptr;
  std::atomic<bool> mapped_{false};
  bool locking_ = false;
  bool producer_pinned_ = false;
};

}  // namespace darwin_art::window
