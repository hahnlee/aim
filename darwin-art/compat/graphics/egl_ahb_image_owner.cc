#include "egl_ahb_image_owner.h"

#include <algorithm>
#include <array>
#include <atomic>
#include <cstdlib>
#include <cstring>
#include <fstream>
#include <iostream>
#include <memory>
#include <mutex>
#include <string>
#include <string_view>
#include <unordered_map>
#include <utility>
#include <vector>
#include <unistd.h>

namespace darwin_art::graphics {

constexpr EglInt kEglNone = 0x3038;
constexpr EglEnum kEglIosurfaceAngle = 0x3454;
constexpr EglInt kEglDeviceExt = 0x322C;
constexpr EglInt kEglMetalDeviceAngle = 0x34A6;
constexpr EglEnum kEglMetalTextureAngle = 0x34A7;
constexpr EglInt kEglWidth = 0x3057;
constexpr EglInt kEglHeight = 0x3056;
constexpr EglInt kEglSurfaceType = 0x3033;
constexpr EglInt kEglPbufferBit = 0x0001;
constexpr EglInt kEglBindToTextureRgba = 0x303B;
constexpr EglInt kEglTrue = 1;
constexpr EglEnum kEglNativeBufferAndroid = 0x3140;
constexpr EglInt kEglIosurfacePlaneAngle = 0x345A;
constexpr EglInt kEglTextureTypeAngle = 0x345C;
constexpr EglInt kEglTextureInternalFormatAngle = 0x345D;
constexpr EglInt kEglBindToTextureTargetAngle = 0x348D;
constexpr EglInt kEglTextureFormat = 0x3080;
constexpr EglInt kEglTextureTarget = 0x3081;
constexpr EglInt kEglTextureRgba = 0x305E;
constexpr EglInt kEglBackBuffer = 0x3084;
constexpr EglInt kEglTexture2d = 0x305F;
constexpr EglInt kEglTextureRectangleAngle = 0x345B;
constexpr EglInt kGlBgraExt = 0x80E1;
constexpr EglInt kGlUnsignedByte = 0x1401;
constexpr EglEnum kGlTexture2d = 0x0DE1;
constexpr EglEnum kGlTextureRectangleAngle = 0x84F5;
constexpr EglInt kGlFramebufferAttachmentTexture = 0x1702;
constexpr EglEnum kGlDrawFramebuffer = 0x8CA9;
constexpr EglEnum kGlColorAttachment0 = 0x8CE0;
constexpr EglEnum kGlFramebufferComplete = 0x8CD5;
constexpr EglEnum kGlFramebufferAttachmentObjectType = 0x8CD0;
constexpr EglEnum kGlFramebufferAttachmentObjectName = 0x8CD1;

struct DarwinAhbEglImage {
  EglDisplayHandle display = nullptr;
  EglContextHandle owner_context = nullptr;
  EglSurfaceHandle owner_draw_surface = nullptr;
  EglSurfaceHandle owner_read_surface = nullptr;
  EglSurfaceHandle pbuffer = nullptr;
  AHardwareBuffer* buffer = nullptr;
  // Newer ANGLE versions can expose a Metal texture as an EglImageHandle. The Metal
  // texture and AHardwareBuffer wrap the same IOSurface, matching Android's
  // EGL_NATIVE_BUFFER_ANDROID storage identity without an intermediate copy.
  EglImageHandle metal_image = nullptr;
  void* metal_texture = nullptr;
  EglInt bind_target = 0;
  std::uint32_t width = 0;
  std::uint32_t height = 0;
  std::uint64_t usage = 0;
  // Name owned by the Android GL client. It remains a valid ANGLE texture
  // object for the complete lifetime expected by Chromium's SharedImage
  // representation.
  std::uint32_t client_texture = 0;
  // Private render-target-capable GL_TEXTURE_2D storage used only by this
  // compatibility layer. Guest bind/attach/query operations translate
  // client_texture to this name without deleting or redefining the client's
  // object.
  std::uint32_t client_staging_texture = 0;
  std::uint32_t iosurface_texture = 0;
  std::uint64_t association_generation = 0;
  // The IOSurface is the persistent backing store for one BufferQueue slot.
  // ANGLE's GL_TEXTURE_2D staging texture is recreated whenever Chromium
  // binds the EglImageHandle, so track the contents independently from the texture
  // association.  A non-zero IOSurface generation means this process has
  // observed published contents for the slot.  The staging generation says
  // whether those exact contents have been restored into the current 2D
  // texture before a partial producer update.
  std::uint64_t iosurface_content_generation = 0;
  std::uint64_t staging_content_generation = 0;
  // The BufferQueue slot whose published IOSurface currently seeds the 2D
  // staging texture. Android's preserved-buffer semantics allow a producer to
  // submit only accumulated damage even when it rotates to another slot.
  AHardwareBuffer* staging_source_buffer = nullptr;
  bool bound = false;
};

static std::atomic<std::uint64_t>& NextAhbTextureAssociation() {
  static std::atomic<std::uint64_t> generation{1};
  return generation;
}

static std::mutex& AhbEglImageMutex() {
  static std::mutex mutex;
  return mutex;
}

static std::unordered_map<EglImageHandle, std::unique_ptr<DarwinAhbEglImage>>&
AhbEglImages() {
  static std::unordered_map<EglImageHandle, std::unique_ptr<DarwinAhbEglImage>>
      images;
  return images;
}

static std::uint32_t GuestTextureForHostTextureLocked(EglContextHandle context,
                                                std::uint32_t host_texture) {
  if (host_texture == 0) return 0;
  for (const auto& [handle, image] : AhbEglImages()) {
    (void)handle;
    if (image->owner_context == context &&
        image->client_staging_texture == host_texture &&
        image->client_texture != 0) {
      return image->client_texture;
    }
  }
  return host_texture;
}

static std::uint32_t HostTextureForGuestTextureLocked(EglContextHandle context,
                                                std::uint32_t guest_texture) {
  if (guest_texture == 0) return 0;
  for (const auto& [handle, image] : AhbEglImages()) {
    (void)handle;
    if (image->owner_context == context &&
        image->client_texture == guest_texture &&
        image->client_staging_texture != 0) {
      return image->client_staging_texture;
    }
  }
  return guest_texture;
}

std::uint32_t GuestTextureForHostTexture(EglContextHandle context,
                                         std::uint32_t host_texture) {
  std::lock_guard<std::mutex> lock(AhbEglImageMutex());
  return GuestTextureForHostTextureLocked(context, host_texture);
}

std::uint32_t HostTextureForGuestTexture(EglContextHandle context,
                                         std::uint32_t guest_texture) {
  std::lock_guard<std::mutex> lock(AhbEglImageMutex());
  return HostTextureForGuestTextureLocked(context, guest_texture);
}

static std::unordered_map<EglContextHandle, AHardwareBuffer*>&
LastPresentedAhbByContext() {
  static std::unordered_map<EglContextHandle, AHardwareBuffer*> buffers;
  return buffers;
}

static std::unordered_map<EglContextHandle, std::uint64_t>&
PresentedGenerationByContext() {
  static std::unordered_map<EglContextHandle, std::uint64_t> generations;
  return generations;
}

static std::unordered_map<AHardwareBuffer*, void*>& QueueByAhb() {
  static std::unordered_map<AHardwareBuffer*, void*> queues;
  return queues;
}

static std::unordered_map<void*, AHardwareBuffer*>& LastPresentedAhbByQueue() {
  static std::unordered_map<void*, AHardwareBuffer*> buffers;
  return buffers;
}

static std::unordered_map<void*, std::uint64_t>& PresentedGenerationByQueue() {
  static std::unordered_map<void*, std::uint64_t> generations;
  return generations;
}

static EglConfigHandle ChooseIosurfaceTextureConfig(
    EglDisplayHandle display, EglInt* bind_target,
    const EglAhbImageBackend& backend) {
  const auto& api = backend;
  const EglInt attributes[] = {kEglSurfaceType, kEglPbufferBit,
                               kEglBindToTextureRgba, kEglTrue, kEglNone};
  std::array<EglConfigHandle, 64> configs{};
  EglInt count = 0;
  if (!api.choose_config(display, attributes, configs.data(), configs.size(),
                         &count)) {
    return nullptr;
  }
  for (EglInt index = 0;
       index < std::min<EglInt>(count, static_cast<EglInt>(configs.size()));
       ++index) {
    EglInt target = 0;
    if (api.get_config_attrib(display, configs[index],
                              kEglBindToTextureTargetAngle, &target) &&
        target == kEglTexture2d) {
      *bind_target = target;
      return configs[index];
    }
  }
  for (EglInt index = 0;
       index < std::min<EglInt>(count, static_cast<EglInt>(configs.size()));
       ++index) {
    EglInt target = 0;
    if (api.get_config_attrib(display, configs[index],
                              kEglBindToTextureTargetAngle, &target) &&
        target == kEglTextureRectangleAngle) {
      *bind_target = target;
      return configs[index];
    }
  }
  return nullptr;
}
EglImageHandle CreateAndroidImage(
    EglDisplayHandle display, EglContextHandle context, EglEnum target,
    void* client_buffer, const EglInt* attributes,
    const EglAhbImageBackend& backend, bool debug) {
  (void)context;
  (void)attributes;
  const auto& api = backend;
  // EGLImage names are display-local in ANGLE. Android's loader can hand this
  // bridge the public EglDisplayHandle while Chromium's passthrough decoder is
  // current on the corresponding host display. Always create an imported AHB
  // image on that current host display when one exists; otherwise a small
  // image ID can alias an unrelated (often YUV) image in the decoder's display.
  const EglDisplayHandle current_display =
      api.get_current_display == nullptr ? nullptr : api.get_current_display();
  const EglDisplayHandle image_display =
      current_display == nullptr ? display : current_display;
  if (debug) {
    std::cerr << "ART Android EGL: eglCreateImageKHR target=0x" << std::hex
              << target << std::dec << " client=" << client_buffer
              << " display=" << display << " current=" << current_display
              << " image_display=" << image_display << "\n";
  }
  if (target != kEglNativeBufferAndroid) return nullptr;
  auto* buffer =
      backend.from_client_buffer(client_buffer);
  AHardwareBuffer_Desc description{};
  backend.ahb_describe(buffer, &description);
  void* iosurface = backend.ahb_iosurface(buffer);
  if (iosurface == nullptr || description.width == 0 ||
      description.height == 0) {
    if (debug) {
      std::cerr << "ART Android EGL: invalid AHardwareBuffer client=" << buffer
                << " iosurface=" << iosurface << " size="
                << description.width << "x" << description.height
                << " format=" << description.format << "\n";
    }
    return nullptr;
  }
  using QueryDisplayAttrib = EglBoolean (*)(EglDisplayHandle, EglInt, EglAttrib*);
  using QueryDeviceAttrib = EglBoolean (*)(void*, EglInt, EglAttrib*);
  using CreateImage = EglImageHandle (*)(EglDisplayHandle, EglContextHandle, EglEnum, void*,
                                   const EglInt*);
  auto query_display_attrib = reinterpret_cast<QueryDisplayAttrib>(
      api.get_proc_address("eglQueryDisplayAttribEXT"));
  auto query_device_attrib = reinterpret_cast<QueryDeviceAttrib>(
      api.get_proc_address("eglQueryDeviceAttribEXT"));
  auto create_image =
      reinterpret_cast<CreateImage>(api.get_proc_address("eglCreateImageKHR"));
  EglAttrib egl_device = 0;
  EglAttrib metal_device = 0;
  void* metal_texture = nullptr;
  EglImageHandle metal_image = nullptr;
  if (query_display_attrib != nullptr && query_device_attrib != nullptr &&
      create_image != nullptr &&
      query_display_attrib(image_display, kEglDeviceExt, &egl_device) != 0 &&
      query_device_attrib(reinterpret_cast<void*>(egl_device),
                          kEglMetalDeviceAngle, &metal_device) != 0) {
    metal_texture = backend.ahb_metal_texture(
        buffer, reinterpret_cast<void*>(metal_device));
    if (metal_texture != nullptr) {
      const EglInt image_attributes[] = {kEglNone};
      metal_image = create_image(image_display, nullptr, kEglMetalTextureAngle,
                                 metal_texture, image_attributes);
      if (metal_image == nullptr) {
        backend.metal_texture_release(metal_texture);
        metal_texture = nullptr;
      }
    }
  }
  if (metal_image != nullptr) {
    backend.ahb_acquire(buffer);
    auto image = std::make_unique<DarwinAhbEglImage>();
    image->display = image_display;
    image->buffer = buffer;
    image->metal_image = metal_image;
    image->metal_texture = metal_texture;
    image->width = description.width;
    image->height = description.height;
    image->usage = description.usage;
    // Preserve the host EGL ABI at the boundary. Chromium obtains some GLES
    // entry points directly from libGLESv2 rather than exclusively through
    // our eglGetProcAddress wrapper. Returning the metadata object's address
    // therefore lets a direct glEglImageHandleTargetTexture2DOES call hand ANGLE a
    // non-EGL pointer (which ANGLE can misclassify as a YUV image). Key the
    // compatibility metadata by, and return, the actual Metal EGLImage. Calls
    // that pass through our wrapper still recover the Android buffer state,
    // while direct GLES calls now receive a valid native image as required by
    // EGL's opaque-handle contract.
    EglImageHandle handle = metal_image;
    {
      std::lock_guard<std::mutex> lock(AhbEglImageMutex());
      AhbEglImages().emplace(handle, std::move(image));
    }
    if (debug) {
      std::cerr << "ART Android EGL: AHardwareBuffer Metal EGLImage=" << handle
                << " native=" << metal_image << " size=" << description.width
                << "x" << description.height << "\n";
    }
    return handle;
  }
  EglInt bind_target = 0;
  EglConfigHandle config = ChooseIosurfaceTextureConfig(
      image_display, &bind_target, backend);
  if (config == nullptr) {
    if (debug)
      std::cerr << "ART Android EGL: no IOSurface texture config\n";
    return nullptr;
  }
  const EglInt iosurface_attributes[] = {
      kEglWidth,
      static_cast<EglInt>(description.width),
      kEglHeight,
      static_cast<EglInt>(description.height),
      kEglIosurfacePlaneAngle,
      0,
      kEglTextureTarget,
      bind_target,
      kEglTextureInternalFormatAngle,
      kGlBgraExt,
      kEglTextureFormat,
      kEglTextureRgba,
      kEglTextureTypeAngle,
      kGlUnsignedByte,
      kEglNone,
  };
  EglSurfaceHandle pbuffer = api.create_pbuffer_from_client_buffer(
      image_display, kEglIosurfaceAngle, iosurface, config,
      iosurface_attributes);
  if (pbuffer == nullptr) {
    if (debug) {
      std::cerr << "ART Android EGL: IOSurface image pbuffer failed size="
                << description.width << "x" << description.height
                << " error=0x" << std::hex << api.get_error() << std::dec
                << "\n";
    }
    return nullptr;
  }
  backend.ahb_acquire(buffer);
  auto image = std::make_unique<DarwinAhbEglImage>();
  image->display = image_display;
  image->pbuffer = pbuffer;
  image->buffer = buffer;
  image->bind_target = bind_target;
  image->width = description.width;
  image->height = description.height;
  image->usage = description.usage;
  EglImageHandle handle = image.get();
  {
    std::lock_guard<std::mutex> lock(AhbEglImageMutex());
    AhbEglImages().emplace(handle, std::move(image));
  }
  if (debug) {
    std::cerr << "ART Android EGL: AHardwareBuffer IOSurface image=" << handle
              << " size=" << description.width << "x" << description.height
              << "\n";
  }
  return handle;
}

EglBoolean DestroyAndroidImage(EglDisplayHandle display, EglImageHandle image,
                               const EglAhbImageBackend& backend, bool debug) {
  (void)debug;
  std::unique_ptr<DarwinAhbEglImage> owned;
  {
    std::lock_guard<std::mutex> lock(AhbEglImageMutex());
    auto found = AhbEglImages().find(image);
    if (found != AhbEglImages().end()) {
      owned = std::move(found->second);
      AhbEglImages().erase(found);
    }
  }
  if (owned != nullptr) {
    const auto& api = backend;
    {
      std::lock_guard<std::mutex> lock(AhbEglImageMutex());
      for (auto iterator = LastPresentedAhbByContext().begin();
           iterator != LastPresentedAhbByContext().end();) {
        if (iterator->second == owned->buffer) {
          iterator = LastPresentedAhbByContext().erase(iterator);
        } else {
          ++iterator;
        }
      }
      auto queue = QueueByAhb().find(owned->buffer);
      if (queue != QueueByAhb().end()) {
        auto last = LastPresentedAhbByQueue().find(queue->second);
        if (last != LastPresentedAhbByQueue().end() &&
            last->second == owned->buffer) {
          LastPresentedAhbByQueue().erase(last);
        }
        QueueByAhb().erase(queue);
      }
    }
    if (owned->metal_image != nullptr) {
      using Function = EglBoolean (*)(EglDisplayHandle, EglImageHandle);
      auto function = reinterpret_cast<Function>(
          api.get_proc_address("eglDestroyImageKHR"));
      if (function != nullptr) function(owned->display, owned->metal_image);
      backend.metal_texture_release(owned->metal_texture);
      owned->metal_image = nullptr;
      owned->metal_texture = nullptr;
    }
    if (owned->bound)
      api.release_tex_image(owned->display, owned->pbuffer, kEglBackBuffer);
    if (owned->client_staging_texture != 0)
      api.gl_delete_textures(1, &owned->client_staging_texture);
    if (owned->iosurface_texture != 0)
      api.gl_delete_textures(1, &owned->iosurface_texture);
    const EglBoolean result =
        owned->pbuffer == nullptr
            ? 1
            : api.destroy_surface(owned->display, owned->pbuffer);
    backend.ahb_release(owned->buffer);
    return result;
  }
  using Function = EglBoolean (*)(EglDisplayHandle, EglImageHandle);
  auto function = reinterpret_cast<Function>(
      backend.get_proc_address("eglDestroyImageKHR"));
  return function == nullptr ? 0 : function(display, image);
}

static bool CopyIosurfaceToAhbClientTextureLocked(
    DarwinAhbEglImage& source, DarwinAhbEglImage& destination,
    const EglAhbImageBackend& backend, bool debug);
void RestoreBufferQueueSlotIfNeeded(
    std::uint32_t texture, const EglAhbImageBackend& backend, bool debug);

void BindImageTexture(EglEnum target, EglImageHandle image,
                      const EglAhbImageBackend& backend, bool debug) {
  if (debug) {
    std::cerr << "ART Android EGL: glEGLImageTargetTexture2DOES target=0x"
              << std::hex << target << std::dec << " image=" << image
              << "\n";
  }
  {
    std::unique_lock<std::mutex> lock(AhbEglImageMutex());
    auto found = AhbEglImages().find(image);
    if (found != AhbEglImages().end()) {
      DarwinAhbEglImage& owned = *found->second;
      if (target != kGlTexture2d) return;
      const auto& api = backend;
      owned.owner_context =
          api.get_current_context == nullptr ? nullptr
                                             : api.get_current_context();
      owned.owner_draw_surface = api.get_current_surface == nullptr
                                     ? nullptr
                                     : api.get_current_surface(0x3059);
      owned.owner_read_surface = api.get_current_surface == nullptr
                                     ? nullptr
                                     : api.get_current_surface(0x305A);
      std::int32_t client_texture = 0;
      api.gl_get_integer_v(0x8069, &client_texture);  // GL_TEXTURE_BINDING_2D
      if (owned.metal_image != nullptr) {
        using Function = void (*)(std::uint32_t, EglImageHandle);
        auto function = reinterpret_cast<Function>(
            api.get_proc_address("glEGLImageTargetTexture2DOES"));
        const std::uint32_t guest_texture = GuestTextureForHostTextureLocked(
            owned.owner_context, static_cast<std::uint32_t>(client_texture));
        // One Chromium texture name is rebound across rotating BufferQueue
        // slots. Keep a private GL name permanently attached to each slot's
        // Metal EglImageHandle; otherwise rebinding the guest name destroys access
        // to the previous IOSurface and a partial update starts from black.
        for (auto& [other_handle, other] : AhbEglImages()) {
          (void)other_handle;
          if (other.get() != &owned &&
              other->owner_context == owned.owner_context &&
              other->client_texture == guest_texture) {
            other->client_texture = 0;
          }
        }
        owned.client_texture = guest_texture;
        if (owned.client_staging_texture == 0) {
          api.gl_gen_textures(1, &owned.client_staging_texture);
        }
        api.gl_bind_texture(kGlTexture2d, owned.client_staging_texture);
        while (api.gl_get_error() != 0) {
        }
        if (function != nullptr) function(target, owned.metal_image);
        owned.association_generation = NextAhbTextureAssociation().fetch_add(
            1, std::memory_order_relaxed);
        owned.bound = function != nullptr && api.gl_get_error() == 0;
        if (debug) {
          std::cerr << "ART Android EGL: bound Metal AHB image texture="
                    << guest_texture << " staging="
                    << owned.client_staging_texture << " native_image="
                    << owned.metal_image
                    << " success=" << owned.bound << "\n";
        }
        // Chromium commonly attaches the texture name to its draw FBO before
        // replacing that name's storage with the EglImageHandle.  Consequently the
        // framebuffer wrapper cannot observe the AHB association yet.  Treat
        // the successful image bind as the acquisition boundary as well, and
        // seed a rotating Metal/IOSurface slot before any partial raster.
        const std::uint32_t acquired_texture = guest_texture;
        const bool acquired = owned.bound;
        lock.unlock();
        if (acquired)
          RestoreBufferQueueSlotIfNeeded(acquired_texture, backend, debug);
        return;
      }
      client_texture = static_cast<std::int32_t>(GuestTextureForHostTextureLocked(
          owned.owner_context, static_cast<std::uint32_t>(client_texture)));
      for (auto& [other_handle, other] : AhbEglImages()) {
        (void)other_handle;
        if (other.get() != &owned &&
            other->owner_context == owned.owner_context &&
            other->client_texture ==
                static_cast<std::uint32_t>(client_texture)) {
          other->client_texture = 0;
          if (other->client_staging_texture != 0) {
            api.gl_delete_textures(1, &other->client_staging_texture);
            other->client_staging_texture = 0;
          }
          other->association_generation = 0;
          other->staging_content_generation = 0;
          other->staging_source_buffer = nullptr;
        }
      }
      owned.client_texture = static_cast<std::uint32_t>(client_texture);
      owned.association_generation =
          NextAhbTextureAssociation().fetch_add(1, std::memory_order_relaxed);
      owned.staging_content_generation = 0;
      owned.staging_source_buffer = nullptr;
      if (owned.bind_target == kEglTexture2d) {
        if (owned.bound) {
          api.release_tex_image(owned.display, owned.pbuffer, kEglBackBuffer);
        }
        owned.bound =
            api.bind_tex_image(owned.display, owned.pbuffer, kEglBackBuffer) !=
            0;
        return;
      }
      // ANGLE's Metal IOSurface backend exposes rectangle textures. Android's
      // EGLImage contract exposes a render-target-capable 2D texture. Keep the
      // client's texture object intact and bind a private 2D staging object
      // behind that guest name; deleting/recreating Chromium's service object
      // breaks SharedImage representation identity across partial rasters.
      constexpr std::array<std::uint32_t, 4> kTextureParameters{
          0x2801,  // GL_TEXTURE_MIN_FILTER
          0x2800,  // GL_TEXTURE_MAG_FILTER
          0x2802,  // GL_TEXTURE_WRAP_S
          0x2803,  // GL_TEXTURE_WRAP_T
      };
      std::array<std::int32_t, 4> texture_parameter_values{
          0x2601, 0x2601, 0x812F, 0x812F};  // LINEAR / CLAMP_TO_EDGE
      if (api.gl_get_tex_parameter_iv != nullptr) {
        for (std::size_t index = 0; index < kTextureParameters.size(); ++index) {
          api.gl_get_tex_parameter_iv(kGlTexture2d,
                                      kTextureParameters[index],
                                      &texture_parameter_values[index]);
        }
      }
      while (api.gl_get_error() != 0) {
      }
      if (owned.client_staging_texture == 0)
        api.gl_gen_textures(1, &owned.client_staging_texture);
      api.gl_bind_texture(kGlTexture2d, owned.client_staging_texture);
      api.gl_tex_image_2d(kGlTexture2d, 0, 0x1908, owned.width, owned.height,
                          0, 0x1908, kGlUnsignedByte, nullptr);
      const std::uint32_t storage_error = api.gl_get_error();
      for (std::size_t index = 0; index < kTextureParameters.size(); ++index) {
        api.gl_tex_parameter_i(kGlTexture2d, kTextureParameters[index],
                               texture_parameter_values[index]);
      }
      if (debug) {
        std::cerr << "ART Android EGL: defined AHB texture context="
                  << owned.owner_context << " texture=" << client_texture
                  << " staging=" << owned.client_staging_texture
                  << " size=" << owned.width << "x" << owned.height
                  << " storage_error=0x" << std::hex << storage_error
                  << std::dec << "\n";
      }
      std::int32_t previous_rectangle = 0;
      api.gl_get_integer_v(0x84F6, &previous_rectangle);
      if (owned.iosurface_texture == 0)
        api.gl_gen_textures(1, &owned.iosurface_texture);
      api.gl_bind_texture(kGlTextureRectangleAngle, owned.iosurface_texture);
      if (!owned.bound) {
        owned.bound = api.bind_tex_image(owned.display, owned.pbuffer,
                                         kEglBackBuffer) != 0;
      }
      api.gl_bind_texture(kGlTextureRectangleAngle,
                          static_cast<std::uint32_t>(previous_rectangle));
      // Do not blit here: Chromium can bind an EGLImage while ANGLE already
      // has an active Metal render encoder. The acquire-fence path below is
      // the safe Android synchronization boundary for IOSurface -> 2D refresh.
      return;
    }
  }
  using Function = void (*)(std::uint32_t, EglImageHandle);
  const auto& api = backend;
  auto function = reinterpret_cast<Function>(
      api.get_proc_address("glEGLImageTargetTexture2DOES"));
  if (function != nullptr) function(target, image);
}

static bool CopyIosurfaceToAhbClientTextureLocked(
    DarwinAhbEglImage& source, DarwinAhbEglImage& destination,
    const EglAhbImageBackend& backend, bool debug) {
  const bool direct_metal = source.metal_image != nullptr &&
                            destination.metal_image != nullptr &&
                            source.client_staging_texture != 0 &&
                            destination.client_staging_texture != 0;
  const bool staged_iosurface =
      source.bind_target == kEglTextureRectangleAngle &&
      source.iosurface_texture != 0 &&
      destination.client_staging_texture != 0;
  if (!source.bound || !destination.bound ||
      (!direct_metal && !staged_iosurface) ||
      source.width != destination.width ||
      source.height != destination.height) {
    return false;
  }
  // ANGLE's Metal EglImageHandle path already gives the guest GL texture the exact
  // IOSurface storage identity.  A different BufferQueue slot is still a
  // different IOSurface, though, and Chromium may render only accumulated
  // damage into it.  Preserve Android's buffer-age contract with a GPU blit
  // between those two IOSurface-backed textures before the destination is
  // attached for drawing.  The older pbuffer path performs the equivalent
  // rectangle-to-staging copy below.
  const std::uint32_t source_target =
      direct_metal ? kGlTexture2d : kGlTextureRectangleAngle;
  const std::uint32_t source_texture =
      direct_metal ? source.client_staging_texture : source.iosurface_texture;
  const std::uint32_t destination_texture =
      destination.client_staging_texture;
  if (source_texture == 0 || destination_texture == 0) return false;
  if (direct_metal && source_texture == destination_texture) return true;
  const auto& api = backend;
  std::int32_t previous_read_framebuffer = 0;
  std::int32_t previous_draw_framebuffer = 0;
  const bool scissor_enabled = api.gl_is_enabled(0x0C11) != 0;
  api.gl_get_integer_v(0x8CAA, &previous_read_framebuffer);
  api.gl_get_integer_v(0x8CA6, &previous_draw_framebuffer);
  std::uint32_t framebuffers[2]{};
  api.gl_gen_framebuffers(2, framebuffers);
  api.gl_bind_framebuffer(0x8CA8, framebuffers[0]);  // GL_READ_FRAMEBUFFER
  api.gl_framebuffer_texture_2d(0x8CA8, kGlColorAttachment0, source_target,
                                source_texture, 0);
  api.gl_bind_framebuffer(0x8CA9, framebuffers[1]);  // GL_DRAW_FRAMEBUFFER
  api.gl_framebuffer_texture_2d(0x8CA9, kGlColorAttachment0, kGlTexture2d,
                                destination_texture, 0);
  const std::uint32_t read_status =
      api.gl_check_framebuffer_status(0x8CA8);
  const std::uint32_t draw_status =
      api.gl_check_framebuffer_status(0x8CA9);
  if (read_status == kGlFramebufferComplete &&
      draw_status == kGlFramebufferComplete) {
    auto debug_samples = [&](const char* phase, std::uint32_t framebuffer) {
      if (std::getenv("DARWIN_ART_DEBUG_AHB_PIXELS") == nullptr) return;
      api.gl_bind_framebuffer(0x8CA8, framebuffer);  // GL_READ_FRAMEBUFFER
      std::cerr << "ART Android EGL: AHB acquire " << phase
                << " client=" << destination.client_texture << " size="
                << destination.width << "x" << destination.height;
      for (float y : std::array<float, 3>{0.25f, 0.5f, 0.75f}) {
        std::uint8_t pixel[4]{};
        api.gl_read_pixels(static_cast<std::int32_t>(destination.width / 2),
                           static_cast<std::int32_t>(destination.height * y),
                           1, 1, 0x1908, kGlUnsignedByte, pixel);
        std::cerr << " [0.5," << y << "]="
                  << static_cast<int>(pixel[0]) << ","
                  << static_cast<int>(pixel[1]) << ","
                  << static_cast<int>(pixel[2]) << ","
                  << static_cast<int>(pixel[3]);
      }
      std::cerr << "\n";
    };
    debug_samples("source", framebuffers[0]);
    api.gl_disable(0x0C11);  // GL_SCISSOR_TEST
    api.gl_blit_framebuffer_angle(
        0, 0, source.width, source.height, 0, 0, destination.width,
        destination.height, 0x00004000, 0x2600);
    if (std::getenv("DARWIN_ART_DEBUG_AHB_DUMP") != nullptr &&
        (destination.height == 1280 || destination.height == 352)) {
      static std::atomic<std::uint32_t> dump_index{0};
      const std::uint32_t index =
          dump_index.fetch_add(1, std::memory_order_relaxed);
      api.gl_bind_framebuffer(0x8CA8, framebuffers[1]);
      std::vector<std::uint8_t> pixels(
          static_cast<std::size_t>(destination.width) * destination.height * 4);
      api.gl_read_pixels(0, 0, destination.width, destination.height, 0x1908,
                         kGlUnsignedByte, pixels.data());
      const std::string path = "/tmp/darwin-art-restored-ahb-" +
          std::to_string(getpid()) + "-" + std::to_string(index) + "-" +
          std::to_string(destination.width) + "x" +
          std::to_string(destination.height) + ".ppm";
      std::ofstream output(path, std::ios::binary);
      output << "P6\n" << destination.width << " " << destination.height
             << "\n255\n";
      for (std::uint32_t y = 0; y < destination.height; ++y) {
        const std::size_t row =
            static_cast<std::size_t>(y) * destination.width * 4;
        for (std::uint32_t x = 0; x < destination.width; ++x) {
          output.write(
              reinterpret_cast<const char*>(pixels.data() + row + x * 4), 3);
        }
      }
    }
    debug_samples("destination", framebuffers[1]);
  } else if (debug) {
    std::cerr << "ART Android EGL: AHB import blit incomplete read=0x"
              << std::hex << read_status << " draw=0x" << draw_status
              << std::dec << " size=" << destination.width << "x"
              << destination.height << "\n";
  }
  api.gl_bind_framebuffer(0x8CA8,
                          static_cast<std::uint32_t>(previous_read_framebuffer));
  api.gl_bind_framebuffer(0x8CA9,
                          static_cast<std::uint32_t>(previous_draw_framebuffer));
  if (scissor_enabled) api.gl_enable(0x0C11);
  api.gl_delete_framebuffers(2, framebuffers);
  return read_status == kGlFramebufferComplete &&
         draw_status == kGlFramebufferComplete;
}

static bool CopyAhbImageToIosurfaceLocked(
    DarwinAhbEglImage& image, const EglAhbImageBackend& backend, bool debug) {
  (void)debug;
  if (!image.bound || image.bind_target != kEglTextureRectangleAngle ||
      image.client_staging_texture == 0 || image.iosurface_texture == 0) {
    return false;
  }
  const auto& api = backend;
  std::int32_t previous_read_framebuffer = 0;
  std::int32_t previous_draw_framebuffer = 0;
  const bool scissor_enabled = api.gl_is_enabled(0x0C11) != 0;
  api.gl_get_integer_v(0x8CAA, &previous_read_framebuffer);
  api.gl_get_integer_v(0x8CA6, &previous_draw_framebuffer);
  std::uint32_t framebuffers[2]{};
  api.gl_gen_framebuffers(2, framebuffers);
  api.gl_bind_framebuffer(0x8CA8, framebuffers[0]);  // GL_READ_FRAMEBUFFER
  api.gl_framebuffer_texture_2d(0x8CA8, kGlColorAttachment0, kGlTexture2d,
                                image.client_staging_texture, 0);
  if (std::getenv("DARWIN_ART_DEBUG_AHB_DUMP") != nullptr &&
      (image.height == 1280 || image.height == 352)) {
    static std::atomic<std::uint32_t> dump_index{0};
    const std::uint32_t index =
        dump_index.fetch_add(1, std::memory_order_relaxed);
    std::vector<std::uint8_t> pixels(
        static_cast<std::size_t>(image.width) * image.height * 4);
    api.gl_read_pixels(0, 0, image.width, image.height, 0x1908,
                       kGlUnsignedByte, pixels.data());
    const std::string path = "/tmp/darwin-art-client-ahb-" +
        std::to_string(getpid()) + "-" + std::to_string(index) + "-" +
        std::to_string(image.width) + "x" + std::to_string(image.height) +
        ".ppm";
    std::ofstream output(path, std::ios::binary);
    output << "P6\n" << image.width << " " << image.height << "\n255\n";
    for (std::uint32_t y = 0; y < image.height; ++y) {
      const std::size_t row = static_cast<std::size_t>(y) * image.width * 4;
      for (std::uint32_t x = 0; x < image.width; ++x) {
        output.write(reinterpret_cast<const char*>(pixels.data() + row + x * 4),
                     3);
      }
    }
  }
  if (std::getenv("DARWIN_ART_DEBUG_AHB_PIXELS") != nullptr) {
    constexpr std::array<float, 3> positions{0.25f, 0.5f, 0.75f};
    std::cerr << "ART Android EGL: AHB producer samples client="
              << image.client_texture << " size=" << image.width << "x"
              << image.height;
    for (float y : positions) {
      for (float x : positions) {
        std::uint8_t pixel[4]{};
        api.gl_read_pixels(static_cast<std::int32_t>(image.width * x),
                           static_cast<std::int32_t>(image.height * y), 1, 1,
                           0x1908, kGlUnsignedByte, pixel);
        std::cerr << " [" << x << "," << y << "]="
                  << static_cast<int>(pixel[0]) << ","
                  << static_cast<int>(pixel[1]) << ","
                  << static_cast<int>(pixel[2]) << ","
                  << static_cast<int>(pixel[3]);
      }
    }
    std::cerr << "\n";
  }
  api.gl_bind_framebuffer(0x8CA9, framebuffers[1]);  // GL_DRAW_FRAMEBUFFER
  api.gl_framebuffer_texture_2d(0x8CA9, kGlColorAttachment0,
                                kGlTextureRectangleAngle,
                                image.iosurface_texture, 0);
  api.gl_disable(0x0C11);  // GL_SCISSOR_TEST
  api.gl_blit_framebuffer_angle(0, 0, image.width, image.height, 0, 0,
                                image.width, image.height, 0x00004000,
                                0x2600);
  api.gl_bind_framebuffer(0x8CA8,
                          static_cast<std::uint32_t>(previous_read_framebuffer));
  api.gl_bind_framebuffer(0x8CA9,
                          static_cast<std::uint32_t>(previous_draw_framebuffer));
  if (scissor_enabled) api.gl_enable(0x0C11);
  api.gl_delete_framebuffers(2, framebuffers);
  return true;
}

void SynchronizeIosurfaceToAhbClientTextures(
    const EglAhbImageBackend& backend, bool debug) {
  std::lock_guard<std::mutex> lock(AhbEglImageMutex());
  const auto& api = backend;
  const EglContextHandle current =
      api.get_current_context == nullptr ? nullptr : api.get_current_context();
  if (current == nullptr) return;
  std::int32_t bound_client_texture = 0;
  api.gl_get_integer_v(0x8069, &bound_client_texture);  // GL_TEXTURE_BINDING_2D
  if (debug) {
    std::cerr << "ART Android EGL: acquire-fence refresh context=" << current
              << " bound=" << bound_client_texture;
  }
  for (auto& [handle, image] : AhbEglImages()) {
    (void)handle;
    if (image->owner_context != current) continue;
    if (bound_client_texture == 0 ||
        image->client_staging_texture !=
            static_cast<std::uint32_t>(bound_client_texture)) {
      continue;
    }
    if (debug) {
      std::cerr << " {client=" << image->client_texture << " " << image->width
                << "x" << image->height << " buffer=" << image->buffer
                << " source=" << image->buffer
                << "}";
    }
    if (CopyIosurfaceToAhbClientTextureLocked(*image, *image, backend, debug)) {
      // An imported acquire fence is the authoritative notification that the
      // shared IOSurface contains a newer producer frame, including when this
      // process did not create that frame and has no local generation history.
      ++image->iosurface_content_generation;
      image->staging_content_generation =
          image->iosurface_content_generation;
      image->staging_source_buffer = image->buffer;
    }
  }
  if (debug) std::cerr << "\n";
}

void SynchronizeAhbImagesToIosurface(
    const EglAhbImageBackend& backend, bool debug) {
  std::lock_guard<std::mutex> lock(AhbEglImageMutex());
  const auto& api = backend;
  const EglContextHandle current =
      api.get_current_context == nullptr ? nullptr : api.get_current_context();
  if (current == nullptr) return;
  std::int32_t draw_framebuffer = 0;
  std::int32_t attachment_type = 0;
  std::int32_t attachment_name = 0;
  std::int32_t viewport[4]{};
  std::int32_t scissor[4]{};
  api.gl_get_integer_v(0x8CA6, &draw_framebuffer);  // GL_DRAW_FRAMEBUFFER_BINDING
  api.gl_get_integer_v(0x0BA2, viewport);  // GL_VIEWPORT
  api.gl_get_integer_v(0x0C10, scissor);   // GL_SCISSOR_BOX
  // GL_COLOR_ATTACHMENT0 only names an attachment on a user-created FBO.
  // The default framebuffer (name zero) uses GL_BACK; querying it as a color
  // attachment generates GL_INVALID_OPERATION and poisons HWUI's next GL
  // checkpoint during a SurfaceView transition.
  if (draw_framebuffer != 0 &&
      api.gl_get_framebuffer_attachment_parameter_iv != nullptr) {
    api.gl_get_framebuffer_attachment_parameter_iv(
        kGlDrawFramebuffer, kGlColorAttachment0,
        kGlFramebufferAttachmentObjectType, &attachment_type);
    api.gl_get_framebuffer_attachment_parameter_iv(
        kGlDrawFramebuffer, kGlColorAttachment0,
        kGlFramebufferAttachmentObjectName, &attachment_name);
  }
  if (debug) {
    std::cerr << "ART Android EGL: native-fence source context=" << current
              << " draw_fbo=" << draw_framebuffer << " attachment_type=0x"
              << std::hex << attachment_type << std::dec
              << " attachment_name=" << attachment_name << " viewport=["
              << viewport[0] << "," << viewport[1] << "," << viewport[2]
              << "," << viewport[3] << "] scissor_enabled="
              << (api.gl_is_enabled(0x0C11) != 0) << " scissor=["
              << scissor[0] << "," << scissor[1] << "," << scissor[2]
              << "," << scissor[3] << "] ahb_images=";
    for (const auto& [handle, image] : AhbEglImages()) {
      (void)handle;
      if (image->owner_context == current) {
        std::cerr << " {client=" << image->client_texture
                  << " iosurface=" << image->iosurface_texture << " "
                  << image->width << "x" << image->height << "}";
      }
    }
    std::cerr << "\n";
  }
  for (auto& [handle, image] : AhbEglImages()) {
    (void)handle;
    // A native fence publishes the commands which precede it in the current
    // GL context.  Chromium keeps several AHardwareBuffer-backed textures
    // alive in one context, but only the texture attached to the current draw
    // framebuffer is the producer for this fence.  Copying every live image
    // here lets stale swapchain slots overwrite their IOSurfaces and turns a
    // later SurfaceControl frame black.
    if (image->owner_context == current &&
        attachment_type == kGlFramebufferAttachmentTexture &&
        attachment_name != 0) {
      const bool direct_metal_attachment =
          image->metal_image != nullptr &&
          image->client_staging_texture ==
              static_cast<std::uint32_t>(attachment_name);
      const bool staged_attachment =
          image->client_staging_texture ==
          static_cast<std::uint32_t>(attachment_name);
      if (direct_metal_attachment) {
        // The client texture is the IOSurface itself. Fence creation publishes
        // a new generation without a copy, allowing the next rotating slot to
        // distinguish newer contents from the same predecessor buffer.
        ++image->iosurface_content_generation;
        image->staging_content_generation =
            image->iosurface_content_generation;
        image->staging_source_buffer = image->buffer;
      } else if (staged_attachment && CopyAhbImageToIosurfaceLocked(*image, backend, debug)) {
        ++image->iosurface_content_generation;
        image->staging_content_generation =
            image->iosurface_content_generation;
        image->staging_source_buffer = image->buffer;
      }
    }
  }
}

bool IsAndroidImage(EglImageHandle image) {
  std::lock_guard<std::mutex> lock(AhbEglImageMutex());
  return AhbEglImages().contains(image);
}

std::optional<SnapshotProducerBinding> SnapshotProducerBindingForBuffer(
    AHardwareBuffer* buffer) {
  std::lock_guard<std::mutex> lock(AhbEglImageMutex());
  for (const auto& [handle, image] : AhbEglImages()) {
    (void)handle;
    if (image->buffer == buffer && image->display != nullptr &&
        image->owner_context != nullptr) {
      return SnapshotProducerBinding{
          .display = image->display,
          .context = image->owner_context,
          .draw_surface = image->owner_draw_surface,
          .read_surface = image->owner_read_surface,
      };
    }
  }
  return std::nullopt;
}

std::optional<LocalPresentation> PrepareLocalPresentation(
    AHardwareBuffer* buffer, void* queue) {
  std::lock_guard<std::mutex> lock(AhbEglImageMutex());
  auto found = std::find_if(
      AhbEglImages().begin(), AhbEglImages().end(),
      [buffer](const auto& entry) { return entry.second->buffer == buffer; });
  if (found == AhbEglImages().end()) return std::nullopt;
  DarwinAhbEglImage& image = *found->second;
  const bool direct_metal =
      image.metal_image != nullptr && image.client_staging_texture != 0;
  const bool staged_rectangle =
      image.bind_target == kEglTextureRectangleAngle &&
      image.client_staging_texture != 0 && image.iosurface_texture != 0;
  if (!image.bound || (!direct_metal && !staged_rectangle))
    return std::nullopt;
  LastPresentedAhbByContext()[image.owner_context] = image.buffer;
  ++PresentedGenerationByContext()[image.owner_context];
  if (queue != nullptr) {
    QueueByAhb()[image.buffer] = queue;
    LastPresentedAhbByQueue()[queue] = image.buffer;
    ++PresentedGenerationByQueue()[queue];
  }
  return LocalPresentation{
      .buffer = image.buffer,
      .width = image.width,
      .height = image.height,
      .usage = image.usage,
  };
}

void MarkHardwareBufferReleased(AHardwareBuffer* buffer, bool debug) {
  if (buffer == nullptr) return;
  std::lock_guard<std::mutex> lock(AhbEglImageMutex());
  for (auto& [handle, image] : AhbEglImages()) {
    (void)handle;
    if (image->buffer != buffer) continue;
    image->staging_content_generation = 0;
    image->staging_source_buffer = nullptr;
    if (debug) {
      std::cerr << "ART Android EGL: released AHB slot buffer=" << buffer
                << " context=" << image->owner_context
                << " texture=" << image->client_texture << " content="
                << image->iosurface_content_generation << "\n";
    }
  }
}
void RestoreBufferQueueSlotIfNeeded(
    std::uint32_t texture, const EglAhbImageBackend& backend, bool debug) {
  // Chromium's SurfaceControl output uses rotating AHB slots for both the root
  // compositor surface and renderer tile surfaces. A producer may redraw only
  // accumulated damage when it reattaches a slot with a positive buffer age.
  // This path has no imported EGL acquire fence, so use the actual FBO
  // attachment as the BufferQueue acquisition boundary. Restore the acquired
  // slot's own persistent IOSurface before Chromium issues any partial draw
  // commands. Its buffer age already tells Chromium which intervening damage
  // regions to accumulate; another slot is never a valid preservation source.
  if (texture == 0) return;
  std::lock_guard<std::mutex> lock(AhbEglImageMutex());
  const auto& api = backend;
  const EglContextHandle current = api.get_current_context == nullptr
                                 ? nullptr
                                 : api.get_current_context();
  if (current != nullptr) {
    DarwinAhbEglImage* destination = nullptr;
    for (auto& [handle, image] : AhbEglImages()) {
      (void)handle;
      if (image->owner_context == current &&
          image->client_texture == texture) {
        destination = image.get();
      }
    }
    if (destination == nullptr) return;
    // A newly-associated AHB has no slot history in this process. Chromium's
    // SurfaceControl SharedImage path can nevertheless begin with partial
    // raster because the logical image already had contents before the AHB
    // representation was installed. Seed that one transition from the last
    // published image in the same producer context. Once this slot has ever
    // been published, its own IOSurface is authoritative: copying a different
    // rotating slot over it destroys the accumulated-damage history and makes
    // small text/caret updates retain stale pixels.
    if (destination->iosurface_content_generation == 0) {
      DarwinAhbEglImage* predecessor = nullptr;
      const auto last = LastPresentedAhbByContext().find(current);
      if (last != LastPresentedAhbByContext().end() &&
          last->second != destination->buffer) {
        for (auto& [handle, candidate] : AhbEglImages()) {
          (void)handle;
          if (candidate->buffer == last->second &&
              candidate->owner_context == current &&
              candidate->width == destination->width &&
              candidate->height == destination->height &&
              candidate->iosurface_content_generation != 0) {
            predecessor = candidate.get();
            break;
          }
        }
      }
      if (predecessor != nullptr &&
          CopyIosurfaceToAhbClientTextureLocked(*predecessor, *destination, backend, debug)) {
        destination->staging_content_generation =
            predecessor->iosurface_content_generation;
        destination->staging_source_buffer = predecessor->buffer;
        if (debug) {
          std::cerr << "ART Android EGL: seeded new AHB representation context="
                    << current << " texture=" << texture << " previous="
                    << predecessor->buffer << " destination="
                    << destination->buffer << " content="
                    << predecessor->iosurface_content_generation << "\n";
        }
      }
      return;
    }
    // Metal EglImageHandles already share the slot IOSurface directly. The
    // rectangle fallback performs the equivalent self restore below.
    if (destination->association_generation != 0 &&
        destination->iosurface_content_generation != 0 &&
        destination->staging_content_generation !=
            destination->iosurface_content_generation &&
        CopyIosurfaceToAhbClientTextureLocked(*destination, *destination, backend, debug)) {
      destination->staging_content_generation =
          destination->iosurface_content_generation;
      destination->staging_source_buffer = destination->buffer;
      if (debug) {
        std::cerr << "ART Android EGL: restored own AHB slot context="
                  << current << " texture=" << texture << " previous="
                  << destination->buffer << " destination="
                  << destination->buffer << " association="
                  << destination->association_generation << " content="
                  << destination->iosurface_content_generation << "\n";
      }
    }
  }
}

void DetachBoundTextureForStorage(
    EglEnum target, const char* operation, EglInt width, EglInt height,
    const EglAhbImageBackend& backend, bool debug) {
  if (target != kGlTexture2d) return;
  const auto& api = backend;
  std::int32_t host_texture = 0;
  api.gl_get_integer_v(0x8069, &host_texture);  // GL_TEXTURE_BINDING_2D
  if (host_texture == 0) return;
  const EglContextHandle current = api.get_current_context == nullptr
                                 ? nullptr
                                 : api.get_current_context();
  std::lock_guard<std::mutex> lock(AhbEglImageMutex());
  for (auto& [handle, image] : AhbEglImages()) {
    (void)handle;
    if (image->owner_context != current ||
        image->client_staging_texture !=
            static_cast<std::uint32_t>(host_texture)) {
      continue;
    }
    if (debug) {
      std::cerr << "ART Android EGL: detached redefined AHB texture context="
                << current << " texture=" << image->client_texture
                << " operation=" << operation << " new_size=" << width
                << "x" << height << " buffer=" << image->buffer
                << " old_size=" << image->width << "x" << image->height
                << "\n";
    }
    const std::uint32_t guest_texture = image->client_texture;
    const std::uint32_t staging_texture = image->client_staging_texture;
    // The explicit storage operation supersedes the EglImageHandle association.
    // Restore the guest object as the physical binding before forwarding it.
    api.gl_bind_texture(kGlTexture2d, guest_texture);
    image->client_texture = 0;
    image->client_staging_texture = 0;
    image->association_generation = 0;
    image->staging_content_generation = 0;
    image->staging_source_buffer = nullptr;
    api.gl_delete_textures(1, &staging_texture);
    break;
  }
}

void DetachDeletedTextures(EglContextHandle context, EglInt count,
                           const std::uint32_t* textures, bool debug) {
  if (count <= 0 || textures == nullptr) return;
  std::lock_guard<std::mutex> lock(AhbEglImageMutex());
  for (auto& [handle, image] : AhbEglImages()) {
    (void)handle;
    if (image->owner_context != context || image->client_texture == 0)
      continue;
    if (std::find(textures, textures + count, image->client_texture) ==
        textures + count) {
      continue;
    }
    if (debug) {
      std::cerr << "ART Android EGL: detached deleted AHB texture context="
                << context << " texture=" << image->client_texture
                << " buffer=" << image->buffer << " size=" << image->width
                << "x" << image->height << "\n";
    }
    image->client_texture = 0;
  }
}

}  // namespace darwin_art::graphics
