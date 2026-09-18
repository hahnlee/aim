#pragma once

#include <cstdint>
#include <mutex>
#include <vector>

struct AHardwareBuffer;

namespace darwin_art::window {

struct NativeWindowFrameToken {
  int32_t slot = -1;
  uint64_t generation = 0;
  uint64_t frame = 0;
};

struct NativeWindowDequeuedBuffer {
  AHardwareBuffer* buffer = nullptr;  // borrowed until queue/cancel
  void* native_buffer = nullptr;
  int acquire_fence = -1;  // transferred to caller
};

// An independent AHB lease survives facade observer/transaction calls. The
// producer fence stays with the caller of Queue on every return path.
struct NativeWindowQueuedBuffer {
  NativeWindowFrameToken token;
  AHardwareBuffer* buffer = nullptr;
  NativeWindowQueuedBuffer() = default;
  ~NativeWindowQueuedBuffer();
  NativeWindowQueuedBuffer(const NativeWindowQueuedBuffer&) = delete;
  NativeWindowQueuedBuffer& operator=(const NativeWindowQueuedBuffer&) = delete;
  NativeWindowQueuedBuffer(NativeWindowQueuedBuffer&&) noexcept;
  NativeWindowQueuedBuffer& operator=(NativeWindowQueuedBuffer&&) noexcept;
};

// BufferQueue slot and fence authority only. No JNI, SurfaceControl, observer,
// containing-window references or display/presentation callbacks cross here.
// Facade -> queue is the only lock order; resource APIs must not call policy.
class NativeWindowBufferQueue final {
 public:
  NativeWindowBufferQueue(int32_t width, int32_t height);
  ~NativeWindowBufferQueue();
  NativeWindowBufferQueue(const NativeWindowBufferQueue&) = delete;
  NativeWindowBufferQueue& operator=(const NativeWindowBufferQueue&) = delete;

  int Dequeue(NativeWindowDequeuedBuffer* output);
  int Queue(void* native_buffer, NativeWindowQueuedBuffer* output);
  int Cancel(void* native_buffer, int owned_fence);
  int PrepareGeometry(int32_t width, int32_t height);
  // Imported metadata publication preserves lazy initial allocation. Once a
  // pool exists, the normal failure-atomic resize contract applies.
  int UpdateGeometry(int32_t width, int32_t height);
  // Consumes the fence even for stale/duplicate tokens. Quarantine cannot be
  // used to re-hold a slot which was already returned by another completion.
  void Return(NativeWindowFrameToken token, int owned_fence, bool quarantine);
  // Existing slot-only consumer ABI: valid only for the current held slot.
  // Frame-aware consumers must use Return with the complete immutable token.
  void ReturnCurrentSlot(int32_t slot, int owned_fence);
  uint64_t NextFrame() const;
  uint64_t Generation() const;

 private:
  struct Slot {
    AHardwareBuffer* buffer = nullptr;
    void* native_buffer = nullptr;
    bool dequeued = false;
    bool held = false;
    int release_fence = -1;
    uint64_t generation = 0;
    uint64_t frame = 0;
    bool retired = false;
  };
  bool Active(const Slot&) const;
  void ReclaimRetiredLocked();
  int PrepareLocked(int32_t width, int32_t height, bool lazy_initial_pool = false);
  mutable std::mutex mutex_;
  std::vector<Slot> slots_;
  int32_t width_;
  int32_t height_;
  size_t next_slot_ = 0;
  uint64_t generation_ = 1;
  uint64_t frame_ = 0;
};
}  // namespace darwin_art::window
