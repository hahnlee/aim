#include "native_window_buffer_queue.h"

#include <android/hardware_buffer.h>
#include <algorithm>
#include <array>
#include <cerrno>
#include <new>
#include <stdexcept>
#include <utility>

extern "C" int sync_wait(int, int);
extern "C" int darwin_art_bionic_socket_broker_close(int);
extern "C" void* darwin_art_android_hardware_buffer_native_window_buffer(void*);

namespace darwin_art::window {
namespace {
void Close(int fence) {
  if (fence >= 0) (void)darwin_art_bionic_socket_broker_close(fence);
}
struct PreparedBuffers {
  std::array<AHardwareBuffer*, 3> buffers{};
  ~PreparedBuffers() {
    for (auto* buffer : buffers)
      if (buffer != nullptr) AHardwareBuffer_release(buffer);
  }
  bool Allocate(int32_t width, int32_t height) {
    AHardwareBuffer_Desc description{};
    description.width = static_cast<uint32_t>(width);
    description.height = static_cast<uint32_t>(height);
    description.layers = 1;
    description.format = AHARDWAREBUFFER_FORMAT_R8G8B8A8_UNORM;
    description.usage = AHARDWAREBUFFER_USAGE_GPU_SAMPLED_IMAGE |
                        AHARDWAREBUFFER_USAGE_GPU_FRAMEBUFFER;
    for (auto& buffer : buffers)
      if (AHardwareBuffer_allocate(&description, &buffer) != 0 || !buffer)
        return false;
    return true;
  }
};
}  // namespace

NativeWindowQueuedBuffer::~NativeWindowQueuedBuffer() {
  if (buffer != nullptr) AHardwareBuffer_release(buffer);
}
NativeWindowQueuedBuffer::NativeWindowQueuedBuffer(
    NativeWindowQueuedBuffer&& other) noexcept
    : token(other.token), buffer(std::exchange(other.buffer, nullptr)) {}
NativeWindowQueuedBuffer& NativeWindowQueuedBuffer::operator=(
    NativeWindowQueuedBuffer&& other) noexcept {
  if (this != &other) {
    if (buffer != nullptr) AHardwareBuffer_release(buffer);
    token = other.token;
    buffer = std::exchange(other.buffer, nullptr);
  }
  return *this;
}
NativeWindowBufferQueue::NativeWindowBufferQueue(int32_t width, int32_t height)
    : width_(width), height_(height) {}
NativeWindowBufferQueue::~NativeWindowBufferQueue() {
  for (auto& slot : slots_) {
    Close(slot.release_fence);
    if (slot.buffer != nullptr) AHardwareBuffer_release(slot.buffer);
  }
}
bool NativeWindowBufferQueue::Active(const Slot& slot) const {
  return slot.buffer && !slot.retired && slot.generation == generation_;
}
void NativeWindowBufferQueue::ReclaimRetiredLocked() {
  for (auto& slot : slots_) {
    if (!slot.retired || !slot.buffer || slot.dequeued || slot.held) continue;
    if (slot.release_fence >= 0 && sync_wait(slot.release_fence, 0) != 0)
      continue;
    Close(slot.release_fence);
    AHardwareBuffer_release(slot.buffer);
    slot = {};
  }
}
int NativeWindowBufferQueue::PrepareLocked(int32_t width, int32_t height,
                                          bool lazy_initial_pool) {
  if (width <= 0 || height <= 0) return -EINVAL;
  ReclaimRetiredLocked();
  size_t active = 0;
  bool active_held = false;
  for (const auto& slot : slots_) {
    if (!Active(slot)) continue;
    ++active;
    if (slot.dequeued) return -EBUSY;
    if (slot.release_fence >= 0 && sync_wait(slot.release_fence, 0) != 0)
      return -EBUSY;
    active_held |= slot.held;
  }
  if (active != 0 && active != 3) return -EBUSY;
  if (active_held && std::any_of(slots_.begin(), slots_.end(),
      [](const Slot& slot) { return slot.retired && slot.buffer; }))
    return -EBUSY;
  if (!lazy_initial_pool && generation_ == UINT64_MAX) return -EBUSY;
  PreparedBuffers prepared;
  if (!prepared.Allocate(width, height)) return -ENOMEM;
  size_t reusable = 0;
  for (const auto& slot : slots_)
    if ((!slot.buffer && !slot.dequeued && !slot.held && slot.release_fence < 0)
        || (Active(slot) && !slot.held)) ++reusable;
  try {
    slots_.reserve(slots_.size() + (reusable >= 3 ? 0 : 3 - reusable));
  } catch (const std::bad_alloc&) { return -ENOMEM; }
    catch (const std::length_error&) { return -ENOMEM; }
  for (auto& slot : slots_) {
    if (!Active(slot)) continue;
    Close(slot.release_fence);
    slot.release_fence = -1;
    if (slot.held) slot.retired = true;
    else { AHardwareBuffer_release(slot.buffer); slot = {}; }
  }
  // Lazy allocation preserves generation1; explicit geometry preparation
  // advances identity even before the first dequeue, matching the old ABI.
  if (!lazy_initial_pool) ++generation_;
  for (auto& buffer : prepared.buffers) {
    auto empty = std::find_if(slots_.begin(), slots_.end(), [](const Slot& slot) {
      return !slot.buffer && !slot.dequeued && !slot.held && slot.release_fence < 0;
    });
    Slot incoming{.buffer = buffer,
        .native_buffer = darwin_art_android_hardware_buffer_native_window_buffer(buffer),
        .generation = generation_};
    if (empty == slots_.end()) slots_.push_back(incoming);
    else *empty = incoming;
    buffer = nullptr;
  }
  width_ = width;
  height_ = height;
  next_slot_ = 0;
  return 0;
}
int NativeWindowBufferQueue::PrepareGeometry(int32_t width, int32_t height) {
  std::lock_guard lock(mutex_);
  return PrepareLocked(width, height);
}

int NativeWindowBufferQueue::UpdateGeometry(int32_t width, int32_t height) {
  std::lock_guard<std::mutex> lock(mutex_);
  if (width <= 0 || height <= 0) return -EINVAL;
  if (width == width_ && height == height_) return 0;
  if (slots_.empty()) {
    width_ = width;
    height_ = height;
    return 0;
  }
  return PrepareLocked(width, height);
}
int NativeWindowBufferQueue::Dequeue(NativeWindowDequeuedBuffer* output) {
  if (!output) return -EINVAL;
  std::lock_guard lock(mutex_);
  ReclaimRetiredLocked();
  if (std::none_of(slots_.begin(), slots_.end(),
                   [this](const Slot& slot) { return Active(slot); })) {
    const int status = PrepareLocked(width_, height_, true);
    if (status != 0) return status;
  }
  for (size_t offset = 0; offset < slots_.size(); ++offset) {
    const size_t index = (next_slot_ + offset) % slots_.size();
    auto& slot = slots_[index];
    if (!Active(slot) || slot.dequeued || slot.held) continue;
    if (slot.release_fence >= 0 && sync_wait(slot.release_fence, 0) != 0)
      continue;
    Close(slot.release_fence);
    slot.release_fence = -1;
    slot.dequeued = true;
    next_slot_ = (index + 1) % slots_.size();
    *output = {.buffer = slot.buffer, .native_buffer = slot.native_buffer};
    return 0;
  }
  return -EBUSY;
}
int NativeWindowBufferQueue::Queue(void* native,
                                   NativeWindowQueuedBuffer* output) {
  if (!native || !output) return -EINVAL;
  // Release an old output resource before entering the policy mutex.
  *output = NativeWindowQueuedBuffer{};
  std::lock_guard lock(mutex_);
  auto found = std::find_if(slots_.begin(), slots_.end(),
      [native](const Slot& slot) { return slot.native_buffer == native; });
  if (found == slots_.end() || !found->dequeued) return -EINVAL;
  if (frame_ == UINT64_MAX) return -EBUSY;
  found->dequeued = false;
  found->held = true;
  found->frame = ++frame_;
  output->token = {static_cast<int32_t>(found - slots_.begin()),
                   found->generation, found->frame};
  AHardwareBuffer_acquire(found->buffer);
  output->buffer = found->buffer;
  return 0;
}
int NativeWindowBufferQueue::Cancel(void* native, int fence) {
  Close(fence);
  if (!native) return -EINVAL;
  std::lock_guard lock(mutex_);
  auto found = std::find_if(slots_.begin(), slots_.end(),
      [native](const Slot& slot) { return slot.native_buffer == native; });
  if (found == slots_.end() || !found->dequeued) return -EINVAL;
  found->dequeued = false;
  return 0;
}
void NativeWindowBufferQueue::Return(NativeWindowFrameToken token,
                                     int fence, bool quarantine) {
  std::lock_guard lock(mutex_);
  if (token.slot < 0 || static_cast<size_t>(token.slot) >= slots_.size()) {
    Close(fence); return;
  }
  auto& slot = slots_[static_cast<size_t>(token.slot)];
  if (!slot.held || slot.generation != token.generation || slot.frame != token.frame) {
    Close(fence); return;
  }
  Close(slot.release_fence);
  slot.release_fence = -1;
  if (quarantine) { Close(fence); return; }
  slot.release_fence = fence;
  slot.held = false;
}
uint64_t NativeWindowBufferQueue::NextFrame() const {
  std::lock_guard lock(mutex_);
  return frame_ == UINT64_MAX ? 0 : frame_ + 1;
}
void NativeWindowBufferQueue::ReturnCurrentSlot(int32_t index, int fence) {
  std::lock_guard lock(mutex_);
  if (index < 0 || static_cast<size_t>(index) >= slots_.size() ||
      !slots_[static_cast<size_t>(index)].held) {
    Close(fence);
    return;
  }
  auto& slot = slots_[static_cast<size_t>(index)];
  Close(slot.release_fence);
  slot.release_fence = fence;
  slot.held = false;
}
uint64_t NativeWindowBufferQueue::Generation() const {
  std::lock_guard lock(mutex_);
  return generation_;
}
}  // namespace darwin_art::window
