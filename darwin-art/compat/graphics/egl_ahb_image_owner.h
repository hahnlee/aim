#pragma once

#include <android/hardware_buffer.h>

#include <cstdint>
#include <optional>

namespace darwin_art::graphics {

// The image owner receives a borrowed view of the facade's already-resolved
// ANGLE entry points. These aliases deliberately keep the EGL ABI opaque and
// fixed-width at this boundary; the owner never sees AngleApi or a loader.
using EglBoolean = std::uint32_t;
using EglInt = std::int32_t;
using EglEnum = std::uint32_t;
using EglAttrib = std::intptr_t;
using EglDisplayHandle = void*;
using EglConfigHandle = void*;
using EglContextHandle = void*;
using EglSurfaceHandle = void*;
using EglImageHandle = void*;

// Borrowed backend view: every PFN and opaque handle is resolved and owned by
// the facade. Calls must not outlive that loaded backend; this owner retains
// neither a cache nor a library handle.
struct EglAhbImageBackend {
  EglDisplayHandle (*get_current_display)() = nullptr;
  EglContextHandle (*get_current_context)() = nullptr;
  EglSurfaceHandle (*get_current_surface)(EglInt) = nullptr;
  EglBoolean (*choose_config)(EglDisplayHandle, const EglInt*,
                              EglConfigHandle*, EglInt, EglInt*) = nullptr;
  EglBoolean (*get_config_attrib)(EglDisplayHandle, EglConfigHandle, EglInt,
                                  EglInt*) = nullptr;
  EglSurfaceHandle (*create_pbuffer_from_client_buffer)(
      EglDisplayHandle, EglEnum, void*, EglConfigHandle,
      const EglInt*) = nullptr;
  EglBoolean (*bind_tex_image)(EglDisplayHandle, EglSurfaceHandle, EglInt) =
      nullptr;
  EglBoolean (*release_tex_image)(EglDisplayHandle, EglSurfaceHandle,
                                  EglInt) = nullptr;
  EglBoolean (*destroy_surface)(EglDisplayHandle, EglSurfaceHandle) = nullptr;
  EglInt (*get_error)() = nullptr;
  void* (*get_proc_address)(const char*) = nullptr;

  void (*gl_disable)(EglEnum) = nullptr;
  void (*gl_enable)(EglEnum) = nullptr;
  std::uint8_t (*gl_is_enabled)(EglEnum) = nullptr;
  void (*gl_get_integer_v)(EglEnum, EglInt*) = nullptr;
  std::uint32_t (*gl_get_error)() = nullptr;
  void (*gl_gen_textures)(EglInt, std::uint32_t*) = nullptr;
  void (*gl_bind_texture)(EglEnum, std::uint32_t) = nullptr;
  void (*gl_tex_parameter_i)(EglEnum, EglEnum, EglInt) = nullptr;
  void (*gl_get_tex_parameter_iv)(EglEnum, EglEnum, EglInt*) = nullptr;
  void (*gl_delete_textures)(EglInt, const std::uint32_t*) = nullptr;
  void (*gl_tex_image_2d)(EglEnum, EglInt, EglInt, EglInt, EglInt, EglInt,
                          EglEnum, EglEnum, const void*) = nullptr;
  void (*gl_gen_framebuffers)(EglInt, std::uint32_t*) = nullptr;
  void (*gl_bind_framebuffer)(EglEnum, std::uint32_t) = nullptr;
  void (*gl_framebuffer_texture_2d)(EglEnum, EglEnum, EglEnum,
                                    std::uint32_t, EglInt) = nullptr;
  std::uint32_t (*gl_check_framebuffer_status)(EglEnum) = nullptr;
  void (*gl_get_framebuffer_attachment_parameter_iv)(EglEnum, EglEnum,
                                                     EglEnum, EglInt*) =
      nullptr;
  void (*gl_delete_framebuffers)(EglInt, const std::uint32_t*) = nullptr;
  void (*gl_blit_framebuffer_angle)(EglInt, EglInt, EglInt, EglInt, EglInt,
                                    EglInt, EglInt, EglInt, EglEnum,
                                    EglEnum) = nullptr;
  void (*gl_read_pixels)(EglInt, EglInt, EglInt, EglInt, EglEnum, EglEnum,
                         void*) = nullptr;

  AHardwareBuffer* (*from_client_buffer)(void*) = nullptr;
  void (*ahb_describe)(const AHardwareBuffer*, AHardwareBuffer_Desc*) = nullptr;
  void* (*ahb_iosurface)(AHardwareBuffer*) = nullptr;
  void* (*ahb_metal_texture)(AHardwareBuffer*, void*) = nullptr;
  void (*metal_texture_release)(void*) = nullptr;
  void (*ahb_acquire)(AHardwareBuffer*) = nullptr;
  void (*ahb_release)(AHardwareBuffer*) = nullptr;
};

struct SnapshotProducerBinding {
  // Borrowed value snapshot: no EGLContext/EGLSurface retain, validity, or
  // thread-migration guarantee is provided after the owner call returns.
  EglDisplayHandle display = nullptr;
  EglContextHandle context = nullptr;
  EglSurfaceHandle draw_surface = nullptr;
  EglSurfaceHandle read_surface = nullptr;
};

struct LocalPresentation {
  // Borrowed caller-retained AHardwareBuffer alias. The caller must establish
  // its independent CompositionBufferLease before using this metadata.
  AHardwareBuffer* buffer = nullptr;
  std::uint32_t width = 0;
  std::uint32_t height = 0;
  std::uint64_t usage = 0;
};

EglImageHandle CreateAndroidImage(EglDisplayHandle display,
                                  EglContextHandle context, EglEnum target,
                                  void* client_buffer, const EglInt* attributes,
                                  const EglAhbImageBackend& backend,
                                  bool debug);
EglBoolean DestroyAndroidImage(EglDisplayHandle display,
                               EglImageHandle image,
                               const EglAhbImageBackend& backend, bool debug);
bool IsAndroidImage(EglImageHandle image);

std::optional<SnapshotProducerBinding> SnapshotProducerBindingForBuffer(
    AHardwareBuffer* buffer);
std::optional<LocalPresentation> PrepareLocalPresentation(
    AHardwareBuffer* buffer, void* queue);
void MarkHardwareBufferReleased(AHardwareBuffer* buffer, bool debug);

void BindImageTexture(EglEnum target, EglImageHandle image,
                      const EglAhbImageBackend& backend, bool debug);
void SynchronizeIosurfaceToAhbClientTextures(
    const EglAhbImageBackend& backend, bool debug);
void SynchronizeAhbImagesToIosurface(const EglAhbImageBackend& backend,
                                     bool debug);

std::uint32_t GuestTextureForHostTexture(EglContextHandle context,
                                         std::uint32_t host_texture);
std::uint32_t HostTextureForGuestTexture(EglContextHandle context,
                                         std::uint32_t guest_texture);
void RestoreBufferQueueSlotIfNeeded(std::uint32_t texture,
                                    const EglAhbImageBackend& backend,
                                    bool debug);
void DetachBoundTextureForStorage(EglEnum target, const char* operation,
                                  EglInt width, EglInt height,
                                  const EglAhbImageBackend& backend, bool debug);
void DetachDeletedTextures(EglContextHandle context, EglInt count,
                           const std::uint32_t* textures, bool debug);

}  // namespace darwin_art::graphics
