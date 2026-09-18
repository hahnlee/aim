#include "egl_context_dispatch.h"

#include <pthread.h>
#include <unistd.h>

#include <cstddef>
#include <iostream>
#include <sstream>

namespace darwin_art::graphics {
namespace {

std::uint64_t NativeTid() {
  std::uint64_t tid = 0;
  return pthread_threadid_np(nullptr, &tid) == 0 ? tid : 0;
}

void LogCreateContext(EglDisplay display, EglConfig config, EglContext share,
                      const EglInt* attributes, EglContext result) {
  std::ostringstream line;
  line << "ART Android EGL: eglCreateContext pid=" << getpid()
       << " native_tid=" << NativeTid() << " display=" << display
       << " config=" << config << " share=" << share << " result=" << result
       << " attributes=";
  if (attributes == nullptr) {
    line << "<null>";
  } else {
    line << "[";
    bool first = true;
    bool terminated = false;
    for (std::size_t index = 0; index < 64; index += 2) {
      const EglInt name = attributes[index];
      if (!first) line << ",";
      first = false;
      line << "0x" << std::hex << name << std::dec;
      if (name == 0x3038) {
        terminated = true;
        break;
      }
      line << "=" << attributes[index + 1];
    }
    if (!terminated) line << ",...";
    line << "]";
  }
  std::cerr << line.str() << '\n';
}

void LogMakeCurrent(EglDisplay display, EglSurface draw, EglSurface read,
                    EglContext context, EglBoolean result) {
  std::ostringstream line;
  line << "ART Android EGL: eglMakeCurrent pid=" << getpid()
       << " native_tid=" << NativeTid() << " display=" << display
       << " draw=" << draw << " read=" << read << " context=" << context
       << " result=" << result;
  std::cerr << line.str() << '\n';
}

}  // namespace

EglContext DispatchCreateContext(EglDisplay display, EglConfig config,
                                 EglContext share, const EglInt* attributes,
                                 CreateContextProc create_context,
                                 bool debug_enabled) {
  if (create_context == nullptr) return nullptr;
  const EglContext result =
      create_context(display, config, share, attributes);
  if (debug_enabled || result == nullptr) {
    LogCreateContext(display, config, share, attributes, result);
  }
  return result;
}

EglBoolean DispatchMakeCurrent(EglDisplay display, EglSurface draw,
                               EglSurface read, EglContext context,
                               MakeCurrentProc make_current,
                               bool debug_enabled) {
  if (make_current == nullptr) return 0;
  const EglBoolean result = make_current(display, draw, read, context);
  if (debug_enabled) {
    LogMakeCurrent(display, draw, read, context, result);
  }
  return result;
}

}  // namespace darwin_art::graphics
