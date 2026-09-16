#pragma once

#include <cstdint>

struct AHardwareBuffer;

namespace darwin_art::media {
// Consumes one retained buffer reference and the acquire-fence descriptor.
// The caller retains the producer through this call. With no additional
// consumer work submitted, the original producer fence becomes the release
// fence: returning a slot must not discard its unfinished producer work.
void ReturnConsumerBuffer(void* producer, int32_t slot, int acquire_fence,
                          AHardwareBuffer* buffer);

// Move-only ownership of a queued slot, retained hardware-buffer reference and
// acquire FD. Construction adopts buffer/FD and retains the producer. Views
// returned by accessors are borrowed; moving invalidates the source's views.
class OwnedConsumerBuffer {
 public:
  OwnedConsumerBuffer() = default;
  OwnedConsumerBuffer(void* producer, int32_t slot, int acquire_fence,
                      AHardwareBuffer* buffer) noexcept;
  ~OwnedConsumerBuffer();
  OwnedConsumerBuffer(const OwnedConsumerBuffer&) = delete;
  OwnedConsumerBuffer& operator=(const OwnedConsumerBuffer&) = delete;
  OwnedConsumerBuffer(OwnedConsumerBuffer&& other) noexcept;
  OwnedConsumerBuffer& operator=(OwnedConsumerBuffer&& other) noexcept;
  void reset() noexcept;
  // Transfers the owned acquire FD to an async consumer. After transfer the
  // consumer must wait on it and return its completion FD before slot reuse.
  int takeFence() noexcept;
  // Adopts a completion FD only when this lease has no remaining acquire FD.
  // On rejection the caller retains ownership; no dependency is overwritten.
  bool adoptFence(int owned_fence) noexcept;
  AHardwareBuffer* buffer() const { return buffer_; }
  int fence() const { return fence_; }

 private:
  void* producer_ = nullptr;
  int32_t slot_ = -1;
  int fence_ = -1;
  AHardwareBuffer* buffer_ = nullptr;
};
}
