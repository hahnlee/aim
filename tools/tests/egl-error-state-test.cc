#include "compat/graphics/egl_error_state.h"

#include <cassert>
#include <cstdint>
#include <thread>

using aim::graphics::ConsumeEglError;
using aim::graphics::PeekEglError;
using aim::graphics::SetEglError;

int main() {
  assert(ConsumeEglError() == aim::graphics::kEglSuccess);
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
  assert(child_before == aim::graphics::kEglSuccess);
  assert(child_after == 0x3009);
  assert(PeekEglError() == 0x3006);
  assert(ConsumeEglError() == 0x3006);
  assert(ConsumeEglError() == aim::graphics::kEglSuccess);
  return 0;
}
