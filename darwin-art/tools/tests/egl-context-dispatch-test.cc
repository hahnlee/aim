#include "compat/graphics/egl_context_dispatch.h"

#include <cassert>
#include <cstdint>
#include <thread>

namespace {

using namespace darwin_art::graphics;

EglDisplay const kDisplay = reinterpret_cast<EglDisplay>(0x101);
EglConfig const kConfig = reinterpret_cast<EglConfig>(0x202);
EglContext const kShare = reinterpret_cast<EglContext>(0x303);
EglSurface const kDraw = reinterpret_cast<EglSurface>(0x404);
EglSurface const kRead = reinterpret_cast<EglSurface>(0x505);

int g_create_calls = 0;
int g_make_current_calls = 0;
bool g_create_args_match = false;
bool g_make_current_args_match = false;
std::thread::id g_make_current_thread;
thread_local int g_backend_error = 0;

EglContext RejectCreate(EglDisplay, EglConfig, EglContext, const EglInt*) {
  ++g_create_calls;
  g_backend_error = 0x3009;
  return nullptr;
}

EglBoolean RejectCurrent(EglDisplay, EglSurface, EglSurface, EglContext) {
  ++g_make_current_calls;
  g_backend_error = 0x3002;
  return 0;
}

EglContext FakeCreateContext(EglDisplay display, EglConfig config,
                             EglContext share, const EglInt* attributes) {
  ++g_create_calls;
  g_create_args_match = display == kDisplay && config == kConfig &&
                        share == kShare && attributes != nullptr &&
                        attributes[0] == 0x3038;
  return reinterpret_cast<EglContext>(0xC0DE);
}

EglBoolean FakeMakeCurrent(EglDisplay display, EglSurface draw,
                           EglSurface read, EglContext context) {
  ++g_make_current_calls;
  g_make_current_args_match = display == kDisplay && draw == kDraw &&
                              read == kRead && context == kShare;
  g_make_current_thread = std::this_thread::get_id();
  return 1;
}

}  // namespace

int main(int argc, char** argv) {
  assert(argc == 2);
  const bool debug = argv[1][0] == '1';
  const EglInt attributes[] = {0x3038};
  if (argv[1][0] == '2') {
    assert(DispatchCreateContext(kDisplay, kConfig, kShare, attributes,
                                 RejectCreate, false) == nullptr);
    assert(g_create_calls == 1 && g_backend_error == 0x3009);
    std::thread worker([&] {
      assert(g_backend_error == 0);
      assert(DispatchMakeCurrent(kDisplay, kDraw, kRead, kShare,
                                  RejectCurrent, false) == 0);
      assert(g_backend_error == 0x3002);
    });
    worker.join();
    assert(g_make_current_calls == 1 && g_backend_error == 0x3009);
    return 0;
  }
  const EglContext context = DispatchCreateContext(
      kDisplay, kConfig, kShare, attributes, FakeCreateContext, debug);
  assert(context == reinterpret_cast<EglContext>(0xC0DE));
  assert(g_create_calls == 1 && g_create_args_match);

  const auto main_thread = std::this_thread::get_id();
  EglBoolean make_result = 0;
  std::thread worker([&] {
    make_result = DispatchMakeCurrent(kDisplay, kDraw, kRead, kShare,
                                      FakeMakeCurrent, debug);
  });
  worker.join();
  assert(make_result == 1);
  assert(g_make_current_calls == 1 && g_make_current_args_match);
  assert(g_make_current_thread != main_thread);

  assert(DispatchCreateContext(kDisplay, kConfig, kShare, attributes, nullptr,
                                debug) == nullptr);
  assert(DispatchMakeCurrent(kDisplay, kDraw, kRead, kShare, nullptr, debug) ==
         0);
  assert(g_create_calls == 1 && g_make_current_calls == 1);
}
