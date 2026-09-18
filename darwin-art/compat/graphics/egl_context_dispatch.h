#pragma once

#include <cstdint>

namespace darwin_art::graphics {

using EglBoolean = std::uint32_t;
using EglInt = std::int32_t;
using EglDisplay = void*;
using EglConfig = void*;
using EglContext = void*;
using EglSurface = void*;

using CreateContextProc = EglContext (*)(EglDisplay, EglConfig, EglContext,
                                         const EglInt*);
using MakeCurrentProc = EglBoolean (*)(EglDisplay, EglSurface, EglSurface,
                                       EglContext);

EglContext DispatchCreateContext(EglDisplay display, EglConfig config,
                                 EglContext share, const EglInt* attributes,
                                 CreateContextProc create_context,
                                 bool debug_enabled);

EglBoolean DispatchMakeCurrent(EglDisplay display, EglSurface draw,
                               EglSurface read, EglContext context,
                               MakeCurrentProc make_current,
                               bool debug_enabled);

}  // namespace darwin_art::graphics
