#include "egl_window_backend.h"

#include "egl_error_state.h"

#include <algorithm>
#include <cstdlib>
#include <iostream>
#include <limits>
#include <new>
#include <utility>

namespace darwin_art::graphics {
namespace {

constexpr std::int32_t kEglNone = 0x3038;
constexpr std::uint32_t kEglIosurfaceAngle = 0x3454;
constexpr std::int32_t kEglIosurfacePlaneAngle = 0x345A;
constexpr std::int32_t kEglTextureTarget = 0x3081;
constexpr std::int32_t kEglTextureInternalFormatAngle = 0x345D;
constexpr std::int32_t kEglTextureFormat = 0x3080;
constexpr std::int32_t kEglTextureTypeAngle = 0x345C;
constexpr std::int32_t kEglBackBuffer = 0x3084;
constexpr std::int32_t kEglBindToTextureTargetAngle = 0x348D;
constexpr std::int32_t kEglWidth = 0x3057;
constexpr std::int32_t kEglHeight = 0x3056;
constexpr std::int32_t kEglTextureRectangleAngle = 0x345B;
constexpr std::int32_t kEglTexture2d = 0x305F;
constexpr std::uint32_t kGlTexture2d = 0x0DE1;
constexpr std::uint32_t kGlTextureRectangleAngle = 0x84F5;
constexpr std::uint32_t kGlBgraExt = 0x80E1;
constexpr std::uint32_t kGlUnsignedByte = 0x1401;
constexpr std::uint32_t kGlFramebuffer = 0x8D40;
constexpr std::uint32_t kGlDrawFramebuffer = 0x8CA9;
constexpr std::uint32_t kGlColorAttachment0 = 0x8CE0;
constexpr std::uint32_t kGlFramebufferComplete = 0x8CD5;

void LatchCreationError(EglWindowCreationError kind,
                        const EglWindowBackendTable& table) {
  switch (kind) {
    case EglWindowCreationError::kUninitializedDisplay:
      SetEglError(kEglNotInitialized);
      return;
    case EglWindowCreationError::kInvalidNativeWindow:
      SetEglError(kEglBadNativeWindow);
      return;
    case EglWindowCreationError::kResourceAllocation:
      SetEglError(kEglBadAlloc);
      return;
    case EglWindowCreationError::kProviderFailure: {
      // Read the provider latch while its failure is still current. Cleanup
      // may issue destroy/cancel calls that replace ANGLE's raw error.
      const std::int32_t provider_error =
          table.get_error == nullptr ? kEglBadMatch : table.get_error();
      SetEglError(provider_error == kEglSuccess ? kEglBadMatch
                                                : provider_error);
      return;
    }
    case EglWindowCreationError::kNone:
      return;
  }
}

bool SameHostSurface(const EglWindowSurfaceSnapshot& value, void* iosurface,
                     std::uint32_t width, std::uint32_t height) {
  return value.iosurface == iosurface && value.width == width &&
         value.height == height;
}

struct GuestGlState {
  bool captured = false;
  bool scissor_enabled = false;
  std::int32_t active_texture = 0;
  std::int32_t texture = 0;
  std::int32_t read_framebuffer = 0;
  std::int32_t draw_framebuffer = 0;
  std::int32_t scissor[4] = {};
};

GuestGlState CaptureGuestGlState(const EglWindowBackendTable& table,
                                 std::uint32_t texture_target) {
  GuestGlState state;
  if (table.gl_get_integer_v == nullptr) return state;
  state.captured = true;
  state.scissor_enabled =
      table.gl_is_enabled != nullptr && table.gl_is_enabled(0x0C11) != 0;
  table.gl_get_integer_v(0x84E0, &state.active_texture);
  table.gl_get_integer_v(texture_target == kGlTexture2d ? 0x8069 : 0x84F6,
                         &state.texture);
  table.gl_get_integer_v(0x8CAA, &state.read_framebuffer);
  table.gl_get_integer_v(0x8CA6, &state.draw_framebuffer);
  table.gl_get_integer_v(0x0C10, state.scissor);
  return state;
}

void RestoreGuestGlState(const EglWindowBackendTable& table,
                         std::uint32_t texture_target,
                         const GuestGlState& state) {
  if (!state.captured) return;
  if (table.gl_bind_framebuffer != nullptr) {
    table.gl_bind_framebuffer(0x8CA8,
                              static_cast<std::uint32_t>(state.read_framebuffer));
    table.gl_bind_framebuffer(kGlDrawFramebuffer,
                              static_cast<std::uint32_t>(state.draw_framebuffer));
  }
  if (table.gl_scissor != nullptr)
    table.gl_scissor(state.scissor[0], state.scissor[1], state.scissor[2],
                     state.scissor[3]);
  if (state.scissor_enabled) {
    if (table.gl_enable != nullptr) table.gl_enable(0x0C11);
  } else if (table.gl_disable != nullptr) {
    table.gl_disable(0x0C11);
  }
  if (table.gl_bind_texture != nullptr)
    table.gl_bind_texture(texture_target, static_cast<std::uint32_t>(state.texture));
  if (table.gl_active_texture != nullptr)
    table.gl_active_texture(static_cast<std::uint32_t>(state.active_texture));
}

}  // namespace

EglWindowBackend::EglWindowBackend(EglWindowBackendTable table,
                                   EglWindowBackendResources resources)
    : table_(table), resources_(resources) {}

bool EglWindowBackend::Initialize(void* display, std::int32_t* major,
                                  std::int32_t* minor) {
  if (display == nullptr || table_.initialize == nullptr) return false;
  auto lease = EglWindowSurfaceOwner::Instance().ReserveDisplayInitialization(
      display);
  if (!lease) return false;
  std::int32_t initialized_major = 0;
  std::int32_t initialized_minor = 0;
  const bool ok = table_.initialize(display, &initialized_major,
                                    &initialized_minor) != 0;
  if (!lease.Complete(ok)) return false;
  if (major != nullptr) *major = initialized_major;
  if (minor != nullptr) *minor = initialized_minor;
  return true;
}

EglWindowCreationResult EglWindowBackend::CreateWindowWithError(
    void* display, void* config, void* native_window) {
  void* surface = nullptr;
  EglWindowCreationError error = EglWindowCreationError::kNone;
  try {
    if (CreateWindowResources(display, config, native_window, &surface,
                              &error))
      return {surface, EglWindowCreationError::kNone};
  } catch (const std::bad_alloc&) {
    error = EglWindowCreationError::kResourceAllocation;
    LatchCreationError(error, table_);
  }
  if (error == EglWindowCreationError::kNone) {
    error = EglWindowCreationError::kProviderFailure;
    LatchCreationError(error, table_);
  }
  // A window surface must retain its BufferQueue/IOSurface publication owner.
  // An ordinary pbuffer cannot substitute for a failed window: swaps on it
  // would report success without submitting any frame to composition.
  return {nullptr, error};
}

void* EglWindowBackend::CreateWindow(void* display, void* config,
                                     void* native_window) {
  return CreateWindowWithError(display, config, native_window).surface;
}

bool EglWindowBackend::CreateWindowResources(void* display, void* config,
                                             void* native_window,
                                             void** surface_out,
                                             EglWindowCreationError* error) {
  if (error != nullptr) *error = EglWindowCreationError::kNone;
  if (surface_out == nullptr) return false;
  *surface_out = nullptr;
  auto fail = [&](EglWindowCreationError kind, bool provider_failure = false) {
    if (error != nullptr) *error = kind;
    if (provider_failure) {
      LatchCreationError(kind, table_);
    } else if (kind == EglWindowCreationError::kProviderFailure) {
      // No provider call failed on this path, so reading raw eglGetError
      // would consume an unrelated caller error.
      SetEglError(kEglBadParameter);
    } else {
      LatchCreationError(kind, table_);
    }
    return false;
  };
  if (display == nullptr ||
      !EglWindowSurfaceOwner::Instance().IsDisplayInitialized(display))
    return fail(EglWindowCreationError::kUninitializedDisplay);
  if (config == nullptr || table_.get_config_attrib == nullptr ||
      table_.create_pbuffer_from_client_buffer == nullptr ||
      table_.create_pbuffer_surface == nullptr)
    return fail(EglWindowCreationError::kProviderFailure);
  if (native_window != nullptr && resources_.native_dequeue == nullptr)
    return fail(EglWindowCreationError::kInvalidNativeWindow);
  void* host = resources_.current_host == nullptr
                   ? nullptr
                   : resources_.current_host(resources_.context);
  void* iosurface = nullptr;
  std::uint32_t width = 0;
  std::uint32_t height = 0;
  void* native_buffer = nullptr;
  bool owns_iosurface = false;
  int acquire_fence = -1;
  void* hardware_buffer = nullptr;
  if (native_window != nullptr && resources_.native_dequeue != nullptr &&
      resources_.native_dequeue(resources_.context, native_window,
                                 &hardware_buffer, &native_buffer,
                                 &acquire_fence) == 0) {
    if (acquire_fence >= 0) {
      if (resources_.wait_fence != nullptr)
        (void)resources_.wait_fence(resources_.context, acquire_fence, -1);
      if (resources_.close_fence != nullptr)
        (void)resources_.close_fence(resources_.context, acquire_fence);
      acquire_fence = -1;
    }
    if (resources_.describe_buffer != nullptr &&
        resources_.buffer_iosurface != nullptr) {
      resources_.describe_buffer(resources_.context, hardware_buffer, &width,
                                 &height);
      iosurface = resources_.buffer_iosurface(resources_.context,
                                              hardware_buffer);
    }
    if (iosurface == nullptr || width == 0 || height == 0) {
      if (resources_.native_cancel != nullptr)
        (void)resources_.native_cancel(resources_.context, native_window,
                                       native_buffer, -1);
      native_buffer = nullptr;
      iosurface = nullptr;
    }
  }
  // Native window geometry and buffers belong exclusively to its producer.
  // Never replace a failed dequeue/import with the unrelated host scanout.
  if (native_window != nullptr &&
      (native_buffer == nullptr || iosurface == nullptr || width == 0 ||
       height == 0))
    return fail(EglWindowCreationError::kInvalidNativeWindow);
  if (native_window == nullptr && iosurface == nullptr &&
      resources_.acquire_iosurface != nullptr)
    owns_iosurface = resources_.acquire_iosurface(
        resources_.context, host, &iosurface, &width, &height);
  if (native_window == nullptr && iosurface == nullptr &&
      resources_.lookup_iosurface != nullptr) {
    const char* inherited = std::getenv("DARWIN_ART_HOST_IOSURFACE_ID");
    if (inherited != nullptr) {
      char* end = nullptr;
      const unsigned long parsed = std::strtoul(inherited, &end, 10);
      if (end != inherited && *end == '\0' && parsed <= UINT32_MAX)
        owns_iosurface = resources_.lookup_iosurface(
            resources_.context, static_cast<std::uint32_t>(parsed), &iosurface,
            &width, &height);
    }
  }
  if (iosurface == nullptr || width == 0 || height == 0)
    return fail(native_window != nullptr
                    ? EglWindowCreationError::kInvalidNativeWindow
                    : EglWindowCreationError::kResourceAllocation);

  std::uint32_t requested_width = width;
  std::uint32_t requested_height = height;
  if (native_window == nullptr && resources_.embedded_geometry != nullptr) {
    std::int32_t embedded_x = 0;
    std::int32_t embedded_y = 0;
    std::uint32_t embedded_width = width;
    std::uint32_t embedded_height = height;
    resources_.embedded_geometry(resources_.context, host, &embedded_x,
                                 &embedded_y, &embedded_width,
                                 &embedded_height);
    (void)embedded_x;
    (void)embedded_y;
    requested_width = std::min(width, std::max<std::uint32_t>(1, embedded_width));
    requested_height =
        std::min(height, std::max<std::uint32_t>(1, embedded_height));
  }

  std::int32_t bind_target = 0;
  if (!table_.get_config_attrib(display, config,
                                kEglBindToTextureTargetAngle,
                                &bind_target)) {
    LatchCreationError(EglWindowCreationError::kProviderFailure, table_);
    if (owns_iosurface && resources_.release_iosurface != nullptr)
      resources_.release_iosurface(resources_.context, iosurface);
    if (native_buffer != nullptr && resources_.native_cancel != nullptr)
      (void)resources_.native_cancel(resources_.context, native_window,
                                     native_buffer, -1);
    if (error != nullptr) *error = EglWindowCreationError::kProviderFailure;
    return false;
  }
  const std::int32_t attributes[] = {
      kEglWidth, static_cast<std::int32_t>(requested_width),
      kEglHeight, static_cast<std::int32_t>(requested_height),
      kEglIosurfacePlaneAngle, 0,
      kEglTextureTarget, bind_target,
      kEglTextureInternalFormatAngle, static_cast<std::int32_t>(kGlBgraExt),
      kEglTextureFormat, 0x305E,
      kEglTextureTypeAngle, static_cast<std::int32_t>(kGlUnsignedByte),
      kEglNone};
  void* target = table_.create_pbuffer_from_client_buffer(
      display, kEglIosurfaceAngle, iosurface, config, attributes);
  if (target == nullptr) {
    if (error != nullptr) *error = EglWindowCreationError::kProviderFailure;
    LatchCreationError(EglWindowCreationError::kProviderFailure, table_);
  }
  const std::int32_t render_attributes[] = {kEglWidth,
                                            static_cast<std::int32_t>(requested_width),
                                            kEglHeight,
                                            static_cast<std::int32_t>(requested_height),
                                            kEglNone};
  void* surface = target == nullptr
                      ? nullptr
                      : table_.create_pbuffer_surface(display, config,
                                                      render_attributes);
  if (target != nullptr && surface == nullptr) {
    if (error != nullptr) *error = EglWindowCreationError::kProviderFailure;
    LatchCreationError(EglWindowCreationError::kProviderFailure, table_);
  }
  const std::uint32_t texture_target =
      bind_target == kEglTextureRectangleAngle
          ? kGlTextureRectangleAngle
          : bind_target == kEglTexture2d ? kGlTexture2d : 0;
  bool native_owned = false;
  if (surface != nullptr && target != nullptr && texture_target != 0) {
    if (native_window != nullptr && resources_.native_acquire != nullptr) {
      resources_.native_acquire(resources_.context, native_window);
      native_owned = true;
    }
    EglWindowSurfaceCreateInfo info;
    info.host = host;
    info.native_window = native_window;
    info.native_buffer = native_buffer;
    info.config = config;
    info.bind_target = bind_target;
    info.iosurface = iosurface;
    info.iosurface_target = target;
    info.texture_target = texture_target;
    info.render_width = requested_width;
    info.render_height = requested_height;
    info.width = requested_width;
    info.height = requested_height;
    info.owns_iosurface_ref = owns_iosurface;
    bool cleaned = false;
    if (EglWindowSurfaceOwner::Instance().Create(
            display, surface, std::move(info), &Cleanup, this, &cleaned)) {
      if (native_owned) native_owned = false;
      *surface_out = surface;
      return true;
    }
    if (cleaned) {
      // Owner::Create has already run Cleanup.  Clear every local ownership
      // marker and leave immediately; repeating the fallback path would
      // destroy the same EGL objects and release the same native references.
      surface = nullptr;
      target = nullptr;
      native_buffer = nullptr;
      iosurface = nullptr;
      owns_iosurface = false;
      native_owned = false;
      if (error != nullptr) *error = EglWindowCreationError::kResourceAllocation;
      LatchCreationError(EglWindowCreationError::kResourceAllocation, table_);
      return false;
    }
  }
  if (error != nullptr && *error == EglWindowCreationError::kNone)
    *error = texture_target == 0
                 ? EglWindowCreationError::kProviderFailure
                 : EglWindowCreationError::kResourceAllocation;
  if (error != nullptr && *error == EglWindowCreationError::kResourceAllocation)
    LatchCreationError(EglWindowCreationError::kResourceAllocation, table_);
  if (texture_target == 0 && target != nullptr && surface != nullptr)
    LatchCreationError(EglWindowCreationError::kProviderFailure, table_);
  if (native_owned && resources_.native_release != nullptr)
    resources_.native_release(resources_.context, native_window);
  if (surface != nullptr && table_.destroy_surface != nullptr)
    (void)table_.destroy_surface(display, surface);
  if (target != nullptr && table_.destroy_surface != nullptr)
    (void)table_.destroy_surface(display, target);
  if (owns_iosurface && resources_.release_iosurface != nullptr)
    resources_.release_iosurface(resources_.context, iosurface);
  if (native_buffer != nullptr && resources_.native_cancel != nullptr)
    (void)resources_.native_cancel(resources_.context, native_window,
                                   native_buffer, -1);
  return false;
}

void EglWindowBackend::Cleanup(const EglWindowSurfaceCleanup& cleanup,
                               void* context) {
  auto* backend = static_cast<EglWindowBackend*>(context);
  if (backend == nullptr) return;
  const auto& value = cleanup.resources;
  if (value.tex_image_bound && value.iosurface_target != nullptr &&
      backend->table_.release_tex_image != nullptr)
    (void)backend->table_.release_tex_image(cleanup.display,
                                             value.iosurface_target,
                                             kEglBackBuffer);
  if (value.framebuffer != 0 &&
      backend->table_.gl_delete_framebuffers != nullptr)
    backend->table_.gl_delete_framebuffers(1, &value.framebuffer);
  if (value.texture != 0 && backend->table_.gl_delete_textures != nullptr)
    backend->table_.gl_delete_textures(1, &value.texture);
  if (value.iosurface_target != nullptr &&
      backend->table_.destroy_surface != nullptr)
    (void)backend->table_.destroy_surface(cleanup.display,
                                           value.iosurface_target);
  if (value.owns_iosurface_ref && value.iosurface != nullptr &&
      backend->resources_.release_iosurface != nullptr)
    backend->resources_.release_iosurface(backend->resources_.context,
                                          value.iosurface);
  if (value.native_buffer != nullptr && value.native_window != nullptr &&
      backend->resources_.native_cancel != nullptr)
    (void)backend->resources_.native_cancel(backend->resources_.context,
                                            value.native_window,
                                            value.native_buffer, -1);
  if (value.native_window != nullptr &&
      backend->resources_.native_release != nullptr)
    backend->resources_.native_release(backend->resources_.context,
                                       value.native_window);
  if (cleanup.surface != nullptr && backend->table_.destroy_surface != nullptr)
    (void)backend->table_.destroy_surface(cleanup.display, cleanup.surface);
}

bool EglWindowBackend::Destroy(void* display, void* surface) {
  if (display == nullptr || surface == nullptr) return false;
  EglWindowSurfaceAdmission admission;
  if (EglWindowSurfaceOwner::Instance().Destroy(display, surface,
                                                 &admission))
    return true;
  if (admission != EglWindowSurfaceAdmission::kUnknown ||
      table_.destroy_surface == nullptr)
    return false;
  return table_.destroy_surface(display, surface) != 0;
}

bool EglWindowBackend::Terminate(void* display) {
  if (display == nullptr || table_.terminate == nullptr) return false;
  const auto callback = [](void* callback_display, void* opaque) -> bool {
    auto* backend = static_cast<EglWindowBackend*>(opaque);
    if (backend == nullptr) return false;
    if (backend->resources_.reset_composition != nullptr &&
        !backend->resources_.reset_composition(backend->resources_.context,
                                               callback_display))
      return false;
    return backend->table_.terminate(callback_display) != 0;
  };
  if (!EglWindowSurfaceOwner::Instance().RetireDisplay(display, callback,
                                                       this))
    return false;
  return EglWindowSurfaceOwner::Instance().TerminationStatus(display) !=
         EglWindowSurfaceTermination::kFailed;
}

bool EglWindowBackend::SwapWindowResources(
    void* display, void* surface, EglWindowSurfaceOwner::Lease* lease,
    bool* transferred) {
  if (lease == nullptr || table_.swap_buffers == nullptr) return false;
  if (transferred != nullptr) *transferred = false;
  auto value = lease->Snapshot();
  // Capture guest state before resize/rebind or any texture/FBO operation.
  // This state belongs to the caller's current GL context, not to the
  // backend's destination resources, and must be restored on every exit.
  const auto guest_state = CaptureGuestGlState(table_, value.texture_target);
  const auto restore_guest_state = [&]() {
    RestoreGuestGlState(table_, value.texture_target, guest_state);
  };
  if (value.host != nullptr && value.native_window == nullptr &&
      resources_.acquire_iosurface != nullptr) {
    void* current_iosurface = nullptr;
    std::uint32_t current_width = 0;
    std::uint32_t current_height = 0;
    if (resources_.acquire_iosurface(resources_.context, value.host,
                                     &current_iosurface, &current_width,
                                     &current_height)) {
      if (!SameHostSurface(value, current_iosurface, current_width,
                           current_height) &&
          table_.create_pbuffer_from_client_buffer != nullptr) {
        const std::uint32_t requested_width = std::min(
            current_width, std::max<std::uint32_t>(1, value.render_width));
        const std::uint32_t requested_height = std::min(
            current_height, std::max<std::uint32_t>(1, value.render_height));
        const std::int32_t attributes[] = {
            kEglWidth, static_cast<std::int32_t>(requested_width),
            kEglHeight, static_cast<std::int32_t>(requested_height),
            kEglIosurfacePlaneAngle, 0,
            kEglTextureTarget, value.bind_target,
            kEglTextureInternalFormatAngle, static_cast<std::int32_t>(kGlBgraExt),
            kEglTextureFormat, 0x305E,
            kEglTextureTypeAngle, static_cast<std::int32_t>(kGlUnsignedByte),
            kEglNone};
        void* replacement = table_.create_pbuffer_from_client_buffer(
            display, kEglIosurfaceAngle, current_iosurface, value.config,
            attributes);
        if (replacement != nullptr) {
          if (value.tex_image_bound && value.iosurface_target != nullptr &&
              table_.release_tex_image != nullptr)
            (void)table_.release_tex_image(display, value.iosurface_target,
                                            kEglBackBuffer);
          if (value.framebuffer != 0 &&
              table_.gl_delete_framebuffers != nullptr)
            table_.gl_delete_framebuffers(1, &value.framebuffer);
          if (value.texture != 0 && table_.gl_delete_textures != nullptr)
            table_.gl_delete_textures(1, &value.texture);
          if (value.iosurface_target != nullptr &&
              table_.destroy_surface != nullptr)
            (void)table_.destroy_surface(display, value.iosurface_target);
          if (value.owns_iosurface_ref &&
              resources_.release_iosurface != nullptr)
            resources_.release_iosurface(resources_.context, value.iosurface);
          lease->SetIosurfaceTarget(current_iosurface, replacement,
                                    requested_width, requested_height, true);
          current_iosurface = nullptr;
          value = lease->Snapshot();
        }
      }
      if (current_iosurface != nullptr &&
          resources_.release_iosurface != nullptr)
        resources_.release_iosurface(resources_.context, current_iosurface);
    }
  }
  if (!value.target_bound && value.iosurface_target != nullptr &&
      table_.gl_gen_textures != nullptr && table_.gl_bind_texture != nullptr &&
      table_.bind_tex_image != nullptr && table_.gl_gen_framebuffers != nullptr &&
      table_.gl_bind_framebuffer != nullptr &&
      table_.gl_framebuffer_texture_2d != nullptr &&
      table_.gl_check_framebuffer_status != nullptr) {
    std::uint32_t texture = 0;
    table_.gl_gen_textures(1, &texture);
    if (texture != 0) {
      table_.gl_bind_texture(value.texture_target, texture);
      if (table_.gl_tex_parameter_i != nullptr) {
        table_.gl_tex_parameter_i(value.texture_target, 0x2801, 0x2601);
        table_.gl_tex_parameter_i(value.texture_target, 0x2800, 0x2601);
      }
      bool bound = table_.bind_tex_image(display, value.iosurface_target,
                                         kEglBackBuffer) != 0;
      std::uint32_t framebuffer = 0;
      if (bound) {
        table_.gl_gen_framebuffers(1, &framebuffer);
        table_.gl_bind_framebuffer(kGlFramebuffer, framebuffer);
        table_.gl_framebuffer_texture_2d(
            kGlFramebuffer, kGlColorAttachment0, value.texture_target, texture,
            0);
        bound = framebuffer != 0 &&
                table_.gl_check_framebuffer_status(kGlFramebuffer) ==
                    kGlFramebufferComplete;
      }
      if (bound) {
        lease->SetTextureAndFramebuffer(texture, framebuffer, true, true);
        value = lease->Snapshot();
      } else {
        if (framebuffer != 0 && table_.gl_delete_framebuffers != nullptr)
          table_.gl_delete_framebuffers(1, &framebuffer);
        if (table_.release_tex_image != nullptr)
          (void)table_.release_tex_image(display, value.iosurface_target,
                                         kEglBackBuffer);
        if (table_.gl_delete_textures != nullptr)
          table_.gl_delete_textures(1, &texture);
      }
    }
  }
  if (value.target_bound && table_.gl_bind_framebuffer != nullptr &&
      table_.gl_blit_framebuffer != nullptr) {
    if (table_.gl_bind_texture != nullptr)
      table_.gl_bind_texture(value.texture_target, value.texture);
    table_.gl_bind_framebuffer(kGlFramebuffer, 0);
    table_.gl_bind_framebuffer(kGlDrawFramebuffer, value.framebuffer);
    if (table_.gl_disable != nullptr) table_.gl_disable(0x0C11);
    table_.gl_blit_framebuffer(0, 0, static_cast<std::int32_t>(value.render_width),
                               static_cast<std::int32_t>(value.render_height),
                               0, 0, static_cast<std::int32_t>(value.width),
                               static_cast<std::int32_t>(value.height),
                               0x00004000, 0x2600);
    if (transferred != nullptr) *transferred = true;
  }
  restore_guest_state();
  return table_.swap_buffers(display, surface) != 0;
}

bool EglWindowBackend::Swap(void* display, void* surface) {
  if (display == nullptr || surface == nullptr || table_.swap_buffers == nullptr)
    return false;
  EglWindowSurfaceAdmission admission;
  auto lease = EglWindowSurfaceOwner::Instance().Swap(display, surface,
                                                       &admission);
  if (!lease) {
    if (admission == EglWindowSurfaceAdmission::kUnknown)
      return table_.swap_buffers(display, surface) != 0;
    return false;
  }
  bool transferred = false;
  if (!SwapWindowResources(display, surface, &lease, &transferred)) return false;
  // A raw swap is not a completed transfer until the backend has an explicit
  // completion fence and that fence reports success.  In particular, a
  // missing wait hook must not silently turn a transferred frame into a
  // publishable/queueable one.
  if (table_.wait_gl == nullptr || table_.wait_gl() == 0) return false;
  const bool gpu_complete = transferred;
  const auto value = lease.Snapshot();
  if (gpu_complete && value.host != nullptr && value.native_window == nullptr &&
      resources_.set_embedded_extent != nullptr &&
      resources_.publish_embedded != nullptr) {
    resources_.set_embedded_extent(resources_.context, value.host, value.width,
                                   value.height);
    resources_.publish_embedded(resources_.context, value.host);
  }
  if (gpu_complete && value.native_window != nullptr &&
      value.native_buffer != nullptr &&
      resources_.native_queue != nullptr) {
    void* queued = value.native_buffer;
    lease.SetNativeBuffer(nullptr);
    if (resources_.native_queue(resources_.context, value.native_window, queued,
                                -1) != 0)
      return false;
    if (resources_.native_dequeue != nullptr) {
      void* next_hardware_buffer = nullptr;
      void* next_native_buffer = nullptr;
      int acquire_fence = -1;
      if (resources_.native_dequeue(resources_.context, value.native_window,
                                     &next_hardware_buffer, &next_native_buffer,
                                     &acquire_fence) != 0)
        return false;
      if (acquire_fence >= 0) {
        if (resources_.wait_fence != nullptr)
          (void)resources_.wait_fence(resources_.context, acquire_fence, -1);
        if (resources_.close_fence != nullptr)
          (void)resources_.close_fence(resources_.context, acquire_fence);
      }
      std::uint32_t next_width = 0;
      std::uint32_t next_height = 0;
      void* next_iosurface = resources_.buffer_iosurface == nullptr
                                 ? nullptr
                                 : resources_.buffer_iosurface(
                                       resources_.context, next_hardware_buffer);
      if (resources_.describe_buffer != nullptr)
        resources_.describe_buffer(resources_.context, next_hardware_buffer,
                                   &next_width, &next_height);
      void* next_target = nullptr;
      if (next_iosurface != nullptr && next_width != 0 && next_height != 0) {
        const std::int32_t attributes[] = {
            kEglWidth, static_cast<std::int32_t>(next_width),
            kEglHeight, static_cast<std::int32_t>(next_height),
            kEglIosurfacePlaneAngle, 0,
            kEglTextureTarget, value.bind_target,
            kEglTextureInternalFormatAngle, static_cast<std::int32_t>(kGlBgraExt),
            kEglTextureFormat, 0x305E,
            kEglTextureTypeAngle, static_cast<std::int32_t>(kGlUnsignedByte),
            kEglNone};
        next_target = table_.create_pbuffer_from_client_buffer(
            display, kEglIosurfaceAngle, next_iosurface, value.config,
            attributes);
      }
      if (next_target == nullptr) {
        if (resources_.native_cancel != nullptr)
          (void)resources_.native_cancel(resources_.context, value.native_window,
                                         next_native_buffer, -1);
        return false;
      }
      if (value.tex_image_bound && value.iosurface_target != nullptr &&
          table_.release_tex_image != nullptr)
        (void)table_.release_tex_image(display, value.iosurface_target,
                                       kEglBackBuffer);
      if (value.framebuffer != 0 &&
          table_.gl_delete_framebuffers != nullptr)
        table_.gl_delete_framebuffers(1, &value.framebuffer);
      if (value.texture != 0 && table_.gl_delete_textures != nullptr)
        table_.gl_delete_textures(1, &value.texture);
      if (value.iosurface_target != nullptr && table_.destroy_surface != nullptr)
        (void)table_.destroy_surface(display, value.iosurface_target);
      lease.SetNativeBuffer(next_native_buffer);
      lease.SetIosurfaceTarget(next_iosurface, next_target, next_width,
                               next_height, false);
    }
  }
  return true;
}

bool EglWindowBackend::ReleaseThread() {
  if (table_.release_thread == nullptr) return false;
  if (resources_.reset_composition != nullptr &&
      !resources_.reset_composition(resources_.context, nullptr))
    return false;
  return table_.release_thread() != 0;
}

}  // namespace darwin_art::graphics
