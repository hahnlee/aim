#pragma once

#include <cstdint>

#include "egl_window_surface_owner.h"

namespace darwin_art::graphics {

enum class EglWindowCreationError {
  kNone,
  kUninitializedDisplay,
  kInvalidNativeWindow,
  kResourceAllocation,
  kProviderFailure,
};

struct EglWindowCreationResult {
  void* surface = nullptr;
  EglWindowCreationError error = EglWindowCreationError::kNone;
};

// This table is a copied, immutable view of the real ANGLE entry points.  It
// deliberately contains no loader handles, JNI state, or symbol resolver.
struct EglWindowBackendTable {
  using Bool = std::uint32_t;
  using Display = void*;
  using Config = void*;
  using Surface = void*;

  Bool (*initialize)(Display, std::int32_t*, std::int32_t*) = nullptr;
  Bool (*terminate)(Display) = nullptr;
  Surface (*create_pbuffer_surface)(Display, Config, const std::int32_t*) = nullptr;
  Surface (*create_pbuffer_from_client_buffer)(
      Display, std::uint32_t, void*, Config, const std::int32_t*) = nullptr;
  Bool (*destroy_surface)(Display, Surface) = nullptr;
  Bool (*swap_buffers)(Display, Surface) = nullptr;
  Bool (*bind_tex_image)(Display, Surface, std::int32_t) = nullptr;
  Bool (*release_tex_image)(Display, Surface, std::int32_t) = nullptr;
  Bool (*wait_gl)() = nullptr;
  Bool (*release_thread)() = nullptr;
  Bool (*get_config_attrib)(Display, Config, std::int32_t, std::int32_t*) = nullptr;
  std::int32_t (*get_error)() = nullptr;

  void (*gl_get_integer_v)(std::uint32_t, std::int32_t*) = nullptr;
  std::uint8_t (*gl_is_enabled)(std::uint32_t) = nullptr;
  void (*gl_disable)(std::uint32_t) = nullptr;
  void (*gl_enable)(std::uint32_t) = nullptr;
  void (*gl_scissor)(std::int32_t, std::int32_t, std::int32_t,
                     std::int32_t) = nullptr;
  void (*gl_bind_texture)(std::uint32_t, std::uint32_t) = nullptr;
  void (*gl_active_texture)(std::uint32_t) = nullptr;
  void (*gl_tex_parameter_i)(std::uint32_t, std::uint32_t, std::int32_t) = nullptr;
  void (*gl_gen_textures)(std::int32_t, std::uint32_t*) = nullptr;
  void (*gl_delete_textures)(std::int32_t, const std::uint32_t*) = nullptr;
  void (*gl_gen_framebuffers)(std::int32_t, std::uint32_t*) = nullptr;
  void (*gl_bind_framebuffer)(std::uint32_t, std::uint32_t) = nullptr;
  void (*gl_framebuffer_texture_2d)(std::uint32_t, std::uint32_t,
                                    std::uint32_t, std::uint32_t,
                                    std::int32_t) = nullptr;
  std::uint32_t (*gl_check_framebuffer_status)(std::uint32_t) = nullptr;
  void (*gl_delete_framebuffers)(std::int32_t, const std::uint32_t*) = nullptr;
  void (*gl_blit_framebuffer)(std::int32_t, std::int32_t, std::int32_t,
                              std::int32_t, std::int32_t, std::int32_t,
                              std::int32_t, std::int32_t, std::uint32_t,
                              std::uint32_t) = nullptr;
  std::uint32_t (*gl_get_error)() = nullptr;
};

// Darwin resource ownership is expressed only through these narrow callbacks.
// The callback context must be process-lifetime (or otherwise outlive every
// deferred owner cleanup and lease), never a per-call stack object.
struct EglWindowBackendResources {
  void* context = nullptr;
  void* (*current_host)(void*) = nullptr;
  bool (*acquire_iosurface)(void*, void*, void**, std::uint32_t*,
                            std::uint32_t*) = nullptr;
  bool (*lookup_iosurface)(void*, std::uint32_t, void**, std::uint32_t*,
                           std::uint32_t*) = nullptr;
  void (*release_iosurface)(void*, void*) = nullptr;
  void (*embedded_geometry)(void*, void*, std::int32_t*, std::int32_t*,
                            std::uint32_t*, std::uint32_t*) = nullptr;
  void (*set_embedded_extent)(void*, void*, std::uint32_t, std::uint32_t) = nullptr;
  void (*publish_embedded)(void*, void*) = nullptr;
  void (*native_acquire)(void*, void*) = nullptr;
  void (*native_release)(void*, void*) = nullptr;
  int (*native_dequeue)(void*, void*, void**, void**, int*) = nullptr;
  int (*native_queue)(void*, void*, void*, int) = nullptr;
  int (*native_cancel)(void*, void*, void*, int) = nullptr;
  void (*describe_buffer)(void*, void*, std::uint32_t*, std::uint32_t*) = nullptr;
  void* (*buffer_iosurface)(void*, void*) = nullptr;
  bool (*reset_composition)(void*, void*) = nullptr;
  int (*wait_fence)(void*, int, int) = nullptr;
  int (*close_fence)(void*, int) = nullptr;
};

class EglWindowBackend final {
 public:
  EglWindowBackend(EglWindowBackendTable table,
                   EglWindowBackendResources resources);
  EglWindowBackend(const EglWindowBackend&) = delete;
  EglWindowBackend& operator=(const EglWindowBackend&) = delete;

  bool Initialize(void* display, std::int32_t* major, std::int32_t* minor);
  EglWindowCreationResult CreateWindowWithError(void* display, void* config,
                                                void* native_window);
  void* CreateWindow(void* display, void* config, void* native_window);
  bool Destroy(void* display, void* surface);
  bool Terminate(void* display);
  bool Swap(void* display, void* surface);
  bool ReleaseThread();

 private:
  static void Cleanup(const EglWindowSurfaceCleanup&, void*);
  bool CreateWindowResources(void* display, void* config, void* native_window,
                             void** surface, EglWindowCreationError* error);
  bool SwapWindowResources(void* display, void* surface,
                          EglWindowSurfaceOwner::Lease* lease,
                          bool* transferred);

  const EglWindowBackendTable table_;
  const EglWindowBackendResources resources_;
};

}  // namespace darwin_art::graphics
