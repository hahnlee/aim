#include "compat/graphics/egl_error_state.h"

#include <cassert>
#include <cstdint>
#include <thread>

using darwin_art::graphics::ConsumeEglError;
using darwin_art::graphics::PeekEglError;
using darwin_art::graphics::SetEglError;

int main() {
  assert(ConsumeEglError() == darwin_art::graphics::kEglSuccess);
  SetEglError(0x3006);
  assert(PeekEglError() == 0x3006);
  assert(PeekEglError() == 0x3006);

  std::int32_t child_before = 0;
  std::int32_t child_after = 0;
  std::thread child([&] {
    child_before = PeekEglError();
    SetEglError(0x3009);
    child_after = ConsumeEglError();
  });
  child.join();
  assert(child_before == darwin_art::graphics::kEglSuccess);
  assert(child_after == 0x3009);
  assert(PeekEglError() == 0x3006);
  assert(ConsumeEglError() == 0x3006);
  assert(ConsumeEglError() == darwin_art::graphics::kEglSuccess);
  return 0;
}
