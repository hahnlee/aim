#include "consumer_buffer.h"

#include "../darwin_angle_egl.h"
#include <android/hardware_buffer.h>
#include <utility>

extern "C" int darwin_art_bionic_socket_broker_close(int);

namespace darwin_art::media {
void ReturnConsumerBuffer(void* producer, int32_t slot, int acquire_fence,
                          AHardwareBuffer* buffer) {
  if (producer != nullptr && slot >= 0) {
    // release_consumer_slot consumes the descriptor even if the slot is stale.
    // No wait/readback or duplicate descriptor is needed on this path.
    darwin_art_android_ANativeWindow_release_consumer_slot(
        producer, slot, acquire_fence);
  } else if (acquire_fence >= 0) {
    (void)darwin_art_bionic_socket_broker_close(acquire_fence);
  }
  if (buffer != nullptr) AHardwareBuffer_release(buffer);
}

OwnedConsumerBuffer::OwnedConsumerBuffer(void* producer, int32_t slot,
    int acquire_fence, AHardwareBuffer* buffer) noexcept
    : producer_(producer), slot_(slot), fence_(acquire_fence), buffer_(buffer) {
  if (producer_) darwin_art_android_ANativeWindow_acquire(producer_);
}
OwnedConsumerBuffer::~OwnedConsumerBuffer() { reset(); }
OwnedConsumerBuffer::OwnedConsumerBuffer(OwnedConsumerBuffer&& other) noexcept {
  *this = std::move(other);
}
OwnedConsumerBuffer& OwnedConsumerBuffer::operator=(OwnedConsumerBuffer&& other) noexcept {
  if (this == &other) return *this;
  reset();
  producer_ = std::exchange(other.producer_, nullptr);
  slot_ = std::exchange(other.slot_, -1);
  fence_ = std::exchange(other.fence_, -1);
  buffer_ = std::exchange(other.buffer_, nullptr);
  return *this;
}
void OwnedConsumerBuffer::reset() noexcept {
  void* producer = std::exchange(producer_, nullptr);
  const int32_t slot = std::exchange(slot_, -1);
  const int fence = std::exchange(fence_, -1);
  auto* buffer = std::exchange(buffer_, nullptr);
  ReturnConsumerBuffer(producer, slot, fence, buffer);
  if (producer) darwin_art_android_ANativeWindow_release(producer);
}
int OwnedConsumerBuffer::takeFence() noexcept {
  return std::exchange(fence_, -1);
}
bool OwnedConsumerBuffer::adoptFence(int owned_fence) noexcept {
  if (fence_ >= 0 || owned_fence < -1) return false;
  fence_ = owned_fence;
  return true;
}
}
