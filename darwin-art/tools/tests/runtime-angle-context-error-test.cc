// This wrapper includes the production implementation so the test invokes the
// actual anonymous-namespace EglCreateContextHost/EglGetError functions.
#include <unistd.h>
#include "../../compat/darwin_angle_egl.cc"

#include <cassert>
#include <cstdint>
#include <cstdlib>
#include <iostream>

extern "C" void* TestCreateContext(const int32_t* attributes) {
  return darwin_art::EglCreateContextHost(nullptr, nullptr, nullptr, attributes);
}

extern "C" uint32_t TestGuestGetError() {
  return static_cast<uint32_t>(EglGetError(nullptr, nullptr));
}

int main(int argc, char** argv) {
  assert(argc == 2);
  const auto expected = static_cast<uint32_t>(std::strtoul(argv[1], nullptr, 0));
  const int32_t attributes[] = {0x3038};  // EGL_NONE
  assert(TestCreateContext(attributes) == nullptr);  // EGL_NO_CONTEXT
  assert(TestGuestGetError() == expected);
  assert(TestGuestGetError() == 0x3000u);  // EGL_SUCCESS; error is consumed once.
  std::cout << "actual EglCreateContextHost dispatch + guest eglGetError latch PASS"
            << " error=0x" << std::hex << expected << std::dec << '\n';
}
