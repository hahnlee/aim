#include "compat/graphics/composition_buffer_lease.h"

#include <cassert>
#include <cstdint>
#include <cstdio>
#include <utility>

// Only the callback table is mocked here. CompositionBufferLease itself is the
// actual production TU; no product AHardwareBuffer implementation is linked.
namespace {

AHardwareBuffer* const kBuffer =
    reinterpret_cast<AHardwareBuffer*>(static_cast<uintptr_t>(0x101));
int g_retain_count = 0;
int g_release_count = 0;
int g_describe_count = 0;
int g_iosurface_count = 0;
bool g_valid_dimensions = true;
bool g_valid_iosurface = true;

void FakeRetain(AHardwareBuffer* buffer) {
  assert(buffer == kBuffer);
  ++g_retain_count;
}

void FakeRelease(AHardwareBuffer* buffer) {
  assert(buffer == kBuffer);
  ++g_release_count;
}

void FakeDescribe(const AHardwareBuffer* buffer, AHardwareBuffer_Desc* desc) {
  assert(buffer == kBuffer && desc != nullptr);
  ++g_describe_count;
  desc->width = g_valid_dimensions ? 16 : 0;
  desc->height = g_valid_dimensions ? 8 : 0;
  desc->layers = 1;
  desc->format = AHARDWAREBUFFER_FORMAT_R8G8B8A8_UNORM;
}

void* FakeIosurface(AHardwareBuffer* buffer) {
  assert(buffer == kBuffer);
  ++g_iosurface_count;
  return g_valid_iosurface ? reinterpret_cast<void*>(0xabc) : nullptr;
}

darwin_art::graphics::CompositionBufferLeaseOps FakeOps() {
  return {
      .retain = FakeRetain,
      .release = FakeRelease,
      .describe = FakeDescribe,
      .iosurface = FakeIosurface,
  };
}

void ResetCounters() {
  g_retain_count = 0;
  g_release_count = 0;
  g_describe_count = 0;
  g_iosurface_count = 0;
  g_valid_dimensions = true;
  g_valid_iosurface = true;
}

}  // namespace

int main() {
  using darwin_art::graphics::CompositionBufferLease;

  auto ops = FakeOps();
  ResetCounters();
  assert(!CompositionBufferLease::Acquire(nullptr, ops));
  assert(g_retain_count == 0 && g_release_count == 0);

  g_valid_dimensions = false;
  assert(!CompositionBufferLease::Acquire(kBuffer, ops));
  assert(g_retain_count == 1 && g_release_count == 1);
  assert(g_describe_count == 1 && g_iosurface_count == 1);

  ResetCounters();
  {
    auto acquired = CompositionBufferLease::Acquire(kBuffer, ops);
    assert(acquired && acquired->buffer() == kBuffer);
    assert(acquired->description().width == 16 &&
           acquired->description().height == 8);
    assert(acquired->iosurface() == reinterpret_cast<void*>(0xabc));
    CompositionBufferLease moved = std::move(*acquired);
    assert(acquired->buffer() == nullptr && acquired->iosurface() == nullptr);
    assert(moved.buffer() == kBuffer);
  }
  assert(g_retain_count == 1 && g_release_count == 1);

  ResetCounters();
  {
    auto first = CompositionBufferLease::Acquire(kBuffer, ops);
    auto second = CompositionBufferLease::Acquire(kBuffer, ops);
    assert(first && second);
    *first = std::move(*second);
    assert(first->buffer() == kBuffer && second->buffer() == nullptr);
    assert(g_retain_count == 2 && g_release_count == 1);
  }
  assert(g_release_count == g_retain_count);

  ResetCounters();
  g_valid_iosurface = false;
  assert(!CompositionBufferLease::Acquire(kBuffer, ops));
  assert(g_retain_count == 1 && g_release_count == 1);

  std::puts("composition buffer lease actual TU/null-invalid/move/exact release PASS");
}
