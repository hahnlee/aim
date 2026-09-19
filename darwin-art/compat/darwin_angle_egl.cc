#include "darwin_angle_egl.h"

#include "graphics/egl_error_state.h"

#include "graphics/composition_buffer_lease.h"
#include "graphics/composition_consumer.h"
#include "graphics/egl_ahb_image_owner.h"
#include "graphics/egl_context_dispatch.h"
#include "graphics/egl_native_fence_owner.h"
#include "graphics/egl_window_backend.h"
#include "graphics/egl_window_surface_owner.h"

#include "darwin_surface_bridge.h"
#include "surfaceflinger/metal_composer.h"
#include "surfaceflinger/service_darwin.h"

#include <android/bitmap.h>
#include <android/hardware_buffer.h>
#include <dlfcn.h>

#include <algorithm>
#include <array>
#include <atomic>
#include <cerrno>
#include <cstdarg>
#include <cstdint>
#include <cstring>
#include <fstream>
#include <cstdlib>
#include <iostream>
#include <iterator>
#include <memory>
#include <mutex>
#include <new>
#include <string>
#include <string_view>
#include <unordered_map>
#include <utility>
#include <unistd.h>
#include <vector>

namespace darwin_art {
const char* EglQueryStringAndroid(void* display, std::int32_t name);
graphics::CompositionConsumerBackend CompositionBackend();
}

void eglBeginFrame(void* display, void* surface);

namespace {

using EGLBoolean = std::uint32_t;
using EGLint = std::int32_t;
using EGLenum = std::uint32_t;
using EGLAttrib = std::intptr_t;
using EGLDisplay = void*;
using EGLConfig = void*;
using EGLContext = void*;
using EGLSurface = void*;
using EGLImage = void*;
using EGLSync = void*;

constexpr EGLint kEglNone = 0x3038;
constexpr EGLint kEglWidth = 0x3057;
constexpr EGLint kEglHeight = 0x3056;
constexpr EGLint kEglExtensions = 0x3055;
constexpr EGLint kEglNotInitialized = 0x3001;
constexpr EGLenum kEglNativeBufferAndroid = 0x3140;
constexpr EGLint kEglSyncNativeFenceFdAndroid = 0x3145;
constexpr EGLenum kEglPlatformAngle = 0x3202;
constexpr EGLint kEglPlatformAngleType = 0x3203;
constexpr EGLint kEglPlatformAngleTypeOpenGles = 0x320E;
constexpr EGLint kEglPlatformAngleTypeMetal = 0x3489;
constexpr EGLenum kEglIosurfaceAngle = 0x3454;
constexpr EGLint kEglIosurfacePlaneAngle = 0x345A;
constexpr EGLint kEglTextureTypeAngle = 0x345C;
constexpr EGLint kEglTextureInternalFormatAngle = 0x345D;
constexpr EGLint kEglBindToTextureTargetAngle = 0x348D;
constexpr EGLint kEglTextureFormat = 0x3080;
constexpr EGLint kEglTextureTarget = 0x3081;
constexpr EGLint kEglTextureRgba = 0x305E;
constexpr EGLint kEglBackBuffer = 0x3084;
constexpr EGLint kEglTexture2d = 0x305F;
constexpr EGLint kEglTextureRectangleAngle = 0x345B;
constexpr EGLint kGlBgraExt = 0x80E1;
constexpr EGLint kGlUnsignedByte = 0x1401;
constexpr std::uint32_t kGlTexture2d = 0x0DE1;
constexpr std::uint32_t kGlTextureRectangleAngle = 0x84F5;
constexpr std::int32_t kGlFramebufferAttachmentTexture = 0x1702;
constexpr std::uint32_t kGlFramebuffer = 0x8D40;
constexpr std::uint32_t kGlDrawFramebuffer = 0x8CA9;
constexpr std::uint32_t kGlColorAttachment0 = 0x8CE0;
constexpr std::uint32_t kGlFramebufferComplete = 0x8CD5;
constexpr std::uint32_t kGlFramebufferAttachmentObjectType = 0x8CD0;
constexpr std::uint32_t kGlFramebufferAttachmentObjectName = 0x8CD1;
constexpr std::uint32_t kGlExtensions = 0x1F03;
constexpr std::uint32_t kGlNumExtensions = 0x821D;

extern "C" void* darwin_art_android_hardware_buffer_iosurface(
    AHardwareBuffer* buffer);
extern "C" AHardwareBuffer*
darwin_art_android_hardware_buffer_from_client_buffer(void* client_buffer);
extern "C" void* darwin_art_android_hardware_buffer_metal_texture(
    AHardwareBuffer* buffer, void* metal_device);
extern "C" void* darwin_art_android_iosurface_metal_texture(
    void* iosurface, std::uint32_t width, std::uint32_t height,
    void* metal_device);
extern "C" void darwin_art_android_metal_texture_release(void* texture);
extern "C" void* darwin_art_android_metal_shared_event_create(
    void* metal_device, std::uint64_t* signal_value);
extern "C" int darwin_art_android_metal_shared_event_fence_fd(
    void* shared_event, std::uint64_t signal_value);
extern "C" void darwin_art_android_metal_shared_event_release(
    void* shared_event);
extern "C" int darwin_art_bionic_fd_import_from_scm(int host_fd);
extern "C" int darwin_art_bionic_socket_broker_dup(int fd);
extern "C" int darwin_art_bionic_socket_broker_pipe(
    std::int32_t descriptors[2]);
extern "C" std::intptr_t darwin_art_bionic_socket_broker_write(
    int fd, const void* bytes, std::size_t count);
extern "C" int darwin_art_bionic_socket_broker_close(int fd);
extern "C" int sync_wait(int fd, int timeout_ms);

struct AngleApi {
  void* egl_library = nullptr;
  void* gles_library = nullptr;
  EGLDisplay (*get_display)(void*) = nullptr;
  EGLBoolean (*initialize)(EGLDisplay, EGLint*, EGLint*) = nullptr;
  EGLBoolean (*choose_config)(EGLDisplay, const EGLint*, EGLConfig*, EGLint,
                              EGLint*) = nullptr;
  EGLBoolean (*get_configs)(EGLDisplay, EGLConfig*, EGLint, EGLint*) = nullptr;
  EGLBoolean (*get_config_attrib)(EGLDisplay, EGLConfig, EGLint, EGLint*) =
      nullptr;
  EGLContext (*create_context)(EGLDisplay, EGLConfig, EGLContext,
                               const EGLint*) = nullptr;
  EGLSurface (*create_pbuffer_surface)(EGLDisplay, EGLConfig,
                                       const EGLint*) = nullptr;
  EGLSurface (*create_pbuffer_from_client_buffer)(
      EGLDisplay, EGLenum, void*, EGLConfig, const EGLint*) = nullptr;
  EGLBoolean (*make_current)(EGLDisplay, EGLSurface, EGLSurface, EGLContext) =
      nullptr;
  EGLBoolean (*destroy_context)(EGLDisplay, EGLContext) = nullptr;
  EGLBoolean (*destroy_surface)(EGLDisplay, EGLSurface) = nullptr;
  EGLBoolean (*terminate)(EGLDisplay) = nullptr;
  EGLBoolean (*swap_buffers)(EGLDisplay, EGLSurface) = nullptr;
  EGLBoolean (*bind_tex_image)(EGLDisplay, EGLSurface, EGLint) = nullptr;
  EGLBoolean (*release_tex_image)(EGLDisplay, EGLSurface, EGLint) = nullptr;
  EGLBoolean (*query_context)(EGLDisplay, EGLContext, EGLint, EGLint*) =
      nullptr;
  EGLBoolean (*query_surface)(EGLDisplay, EGLSurface, EGLint, EGLint*) =
      nullptr;
  const char* (*query_string)(EGLDisplay, EGLint) = nullptr;
  EGLDisplay (*get_current_display)() = nullptr;
  EGLContext (*get_current_context)() = nullptr;
  EGLSurface (*get_current_surface)(EGLint) = nullptr;
  EGLint (*get_error)() = nullptr;
  void* (*get_proc_address)(const char*) = nullptr;
  EGLBoolean (*release_thread)() = nullptr;
  EGLBoolean (*wait_gl)() = nullptr;
  EGLBoolean (*wait_native)(EGLint) = nullptr;

  void (*gl_pixel_store_i)(std::uint32_t, std::int32_t) = nullptr;
  void (*gl_disable)(std::uint32_t) = nullptr;
  void (*gl_enable)(std::uint32_t) = nullptr;
  std::uint8_t (*gl_is_enabled)(std::uint32_t) = nullptr;
  void (*gl_scissor)(std::int32_t, std::int32_t, std::int32_t,
                     std::int32_t) = nullptr;
  void (*gl_depth_mask)(std::uint8_t) = nullptr;
  void (*gl_clear_color)(float, float, float, float) = nullptr;
  void (*gl_clear)(std::uint32_t) = nullptr;
  void (*gl_viewport)(std::int32_t, std::int32_t, std::int32_t,
                      std::int32_t) = nullptr;
  std::uint8_t (*gl_is_texture)(std::uint32_t) = nullptr;
  void (*gl_gen_textures)(std::int32_t, std::uint32_t*) = nullptr;
  void (*gl_active_texture)(std::uint32_t) = nullptr;
  void (*gl_bind_texture)(std::uint32_t, std::uint32_t) = nullptr;
  void (*gl_tex_parameter_i)(std::uint32_t, std::uint32_t, std::int32_t) =
      nullptr;
  void (*gl_get_tex_parameter_iv)(std::uint32_t, std::uint32_t,
                                  std::int32_t*) = nullptr;
  std::uint32_t (*gl_get_error)() = nullptr;
  void (*gl_delete_textures)(std::int32_t, const std::uint32_t*) = nullptr;
  void (*gl_tex_image_2d)(std::uint32_t, std::int32_t, std::int32_t,
                          std::int32_t, std::int32_t, std::int32_t,
                          std::uint32_t, std::uint32_t, const void*) = nullptr;
  void (*gl_tex_sub_image_2d)(std::uint32_t, std::int32_t, std::int32_t,
                              std::int32_t, std::int32_t, std::int32_t,
                              std::uint32_t, std::uint32_t, const void*) =
      nullptr;
  void (*gl_get_integer_v)(std::uint32_t, std::int32_t*) = nullptr;
  void (*gl_get_boolean_v)(std::uint32_t, std::uint8_t*) = nullptr;
  void (*gl_color_mask)(std::uint8_t, std::uint8_t, std::uint8_t,
                        std::uint8_t) = nullptr;
  void (*gl_tex_storage_1d_ext)(std::uint32_t, std::int32_t, std::uint32_t,
                                std::int32_t) = nullptr;
  void (*gl_tex_storage_2d_ext)(std::uint32_t, std::int32_t, std::uint32_t,
                                std::int32_t, std::int32_t) = nullptr;
  void (*gl_tex_storage_3d_ext)(std::uint32_t, std::int32_t, std::uint32_t,
                                std::int32_t, std::int32_t,
                                std::int32_t) = nullptr;
  void (*gl_read_pixels)(std::int32_t, std::int32_t, std::int32_t,
                         std::int32_t, std::uint32_t, std::uint32_t,
                         void*) = nullptr;
  void (*gl_gen_framebuffers)(std::int32_t, std::uint32_t*) = nullptr;
  void (*gl_bind_framebuffer)(std::uint32_t, std::uint32_t) = nullptr;
  void (*gl_framebuffer_texture_2d)(std::uint32_t, std::uint32_t,
                                    std::uint32_t, std::uint32_t,
                                    std::int32_t) = nullptr;
  std::uint32_t (*gl_check_framebuffer_status)(std::uint32_t) = nullptr;
  void (*gl_get_framebuffer_attachment_parameter_iv)(
      std::uint32_t, std::uint32_t, std::uint32_t, std::int32_t*) = nullptr;
  void (*gl_delete_framebuffers)(std::int32_t, const std::uint32_t*) = nullptr;
  void (*gl_blit_framebuffer_angle)(std::int32_t, std::int32_t, std::int32_t,
                                    std::int32_t, std::int32_t, std::int32_t,
                                    std::int32_t, std::int32_t, std::uint32_t,
                                    std::uint32_t) = nullptr;
  std::uint32_t (*gl_create_shader)(std::uint32_t) = nullptr;
  void (*gl_shader_source)(std::uint32_t, std::int32_t, const char* const*,
                           const std::int32_t*) = nullptr;
  void (*gl_compile_shader)(std::uint32_t) = nullptr;
  void (*gl_get_shader_iv)(std::uint32_t, std::uint32_t, std::int32_t*) =
      nullptr;
  void (*gl_get_shader_info_log)(std::uint32_t, std::int32_t, std::int32_t*,
                                 char*) = nullptr;
  void (*gl_delete_shader)(std::uint32_t) = nullptr;
  std::uint32_t (*gl_create_program)() = nullptr;
  void (*gl_attach_shader)(std::uint32_t, std::uint32_t) = nullptr;
  void (*gl_link_program)(std::uint32_t) = nullptr;
  void (*gl_get_program_iv)(std::uint32_t, std::uint32_t, std::int32_t*) =
      nullptr;
  void (*gl_get_program_info_log)(std::uint32_t, std::int32_t, std::int32_t*,
                                  char*) = nullptr;
  void (*gl_delete_program)(std::uint32_t) = nullptr;
  void (*gl_use_program)(std::uint32_t) = nullptr;
  std::int32_t (*gl_get_uniform_location)(std::uint32_t, const char*) = nullptr;
  void (*gl_uniform_1i)(std::int32_t, std::int32_t) = nullptr;
  void (*gl_uniform_1f)(std::int32_t, float) = nullptr;
  void (*gl_uniform_4f)(std::int32_t, float, float, float, float) = nullptr;
  void (*gl_blend_func_separate)(std::uint32_t, std::uint32_t, std::uint32_t,
                                 std::uint32_t) = nullptr;
  void (*gl_blend_equation_separate)(std::uint32_t, std::uint32_t) = nullptr;
  void (*gl_draw_arrays)(std::uint32_t, std::int32_t, std::int32_t) = nullptr;

  bool ready = false;
};

template <typename T>
T LoadSymbol(void* library, const char* name) {
  return reinterpret_cast<T>(dlsym(library, name));
}

AngleApi& GetAngleApi() {
  static AngleApi api;
  static std::once_flag once;
  std::call_once(once, [] {
    const char* directory = std::getenv("DARWIN_ART_ANGLE_DIRECTORY");
    if (directory == nullptr || directory[0] != '/') {
      std::cerr << "ART Android EGL: DARWIN_ART_ANGLE_DIRECTORY must be an "
                   "absolute path\n";
      return;
    }
    const std::string gles_path =
        std::string(directory) + "/libGLESv2.dylib";
    const std::string egl_path = std::string(directory) + "/libEGL.dylib";
    api.gles_library = dlopen(gles_path.c_str(), RTLD_NOW | RTLD_GLOBAL);
    api.egl_library = dlopen(egl_path.c_str(), RTLD_NOW | RTLD_GLOBAL);
    if (api.gles_library == nullptr || api.egl_library == nullptr) {
      const char* error = dlerror();
      std::cerr << "ART Android EGL: ANGLE load failed: "
                << (error == nullptr ? "unknown error" : error) << "\n";
      return;
    }
#define LOAD_EGL(field, symbol)                                               \
  api.field = LoadSymbol<decltype(api.field)>(api.egl_library, symbol)
    LOAD_EGL(get_display, "eglGetDisplay");
    LOAD_EGL(initialize, "eglInitialize");
    LOAD_EGL(choose_config, "eglChooseConfig");
    LOAD_EGL(get_configs, "eglGetConfigs");
    LOAD_EGL(get_config_attrib, "eglGetConfigAttrib");
    LOAD_EGL(create_context, "eglCreateContext");
    LOAD_EGL(create_pbuffer_surface, "eglCreatePbufferSurface");
    LOAD_EGL(create_pbuffer_from_client_buffer,
             "eglCreatePbufferFromClientBuffer");
    LOAD_EGL(make_current, "eglMakeCurrent");
    LOAD_EGL(destroy_context, "eglDestroyContext");
    LOAD_EGL(destroy_surface, "eglDestroySurface");
    LOAD_EGL(terminate, "eglTerminate");
    LOAD_EGL(swap_buffers, "eglSwapBuffers");
    LOAD_EGL(bind_tex_image, "eglBindTexImage");
    LOAD_EGL(release_tex_image, "eglReleaseTexImage");
    LOAD_EGL(query_context, "eglQueryContext");
    LOAD_EGL(query_surface, "eglQuerySurface");
    LOAD_EGL(query_string, "eglQueryString");
    LOAD_EGL(get_current_display, "eglGetCurrentDisplay");
    LOAD_EGL(get_current_context, "eglGetCurrentContext");
    LOAD_EGL(get_current_surface, "eglGetCurrentSurface");
    LOAD_EGL(get_error, "eglGetError");
    LOAD_EGL(get_proc_address, "eglGetProcAddress");
    LOAD_EGL(release_thread, "eglReleaseThread");
    LOAD_EGL(wait_gl, "eglWaitGL");
    LOAD_EGL(wait_native, "eglWaitNative");
#undef LOAD_EGL
#define LOAD_GL(field, symbol)                                                \
  api.field = LoadSymbol<decltype(api.field)>(api.gles_library, symbol)
    LOAD_GL(gl_pixel_store_i, "glPixelStorei");
    LOAD_GL(gl_disable, "glDisable");
    LOAD_GL(gl_enable, "glEnable");
    LOAD_GL(gl_is_enabled, "glIsEnabled");
    LOAD_GL(gl_scissor, "glScissor");
    LOAD_GL(gl_depth_mask, "glDepthMask");
    LOAD_GL(gl_clear_color, "glClearColor");
    LOAD_GL(gl_clear, "glClear");
    LOAD_GL(gl_viewport, "glViewport");
    LOAD_GL(gl_is_texture, "glIsTexture");
    LOAD_GL(gl_gen_textures, "glGenTextures");
    LOAD_GL(gl_active_texture, "glActiveTexture");
    LOAD_GL(gl_bind_texture, "glBindTexture");
    LOAD_GL(gl_tex_parameter_i, "glTexParameteri");
    LOAD_GL(gl_get_tex_parameter_iv, "glGetTexParameteriv");
    LOAD_GL(gl_get_error, "glGetError");
    LOAD_GL(gl_delete_textures, "glDeleteTextures");
    LOAD_GL(gl_tex_image_2d, "glTexImage2D");
    LOAD_GL(gl_tex_sub_image_2d, "glTexSubImage2D");
    LOAD_GL(gl_get_integer_v, "glGetIntegerv");
    LOAD_GL(gl_get_boolean_v, "glGetBooleanv");
    LOAD_GL(gl_color_mask, "glColorMask");
    LOAD_GL(gl_tex_storage_1d_ext, "glTexStorage1DEXT");
    LOAD_GL(gl_tex_storage_2d_ext, "glTexStorage2DEXT");
    LOAD_GL(gl_tex_storage_3d_ext, "glTexStorage3DEXT");
    LOAD_GL(gl_read_pixels, "glReadPixels");
    LOAD_GL(gl_gen_framebuffers, "glGenFramebuffers");
    LOAD_GL(gl_bind_framebuffer, "glBindFramebuffer");
    LOAD_GL(gl_framebuffer_texture_2d, "glFramebufferTexture2D");
    LOAD_GL(gl_check_framebuffer_status, "glCheckFramebufferStatus");
    LOAD_GL(gl_get_framebuffer_attachment_parameter_iv,
            "glGetFramebufferAttachmentParameteriv");
    LOAD_GL(gl_delete_framebuffers, "glDeleteFramebuffers");
    LOAD_GL(gl_blit_framebuffer_angle, "glBlitFramebufferANGLE");
    LOAD_GL(gl_create_shader, "glCreateShader");
    LOAD_GL(gl_shader_source, "glShaderSource");
    LOAD_GL(gl_compile_shader, "glCompileShader");
    LOAD_GL(gl_get_shader_iv, "glGetShaderiv");
    LOAD_GL(gl_get_shader_info_log, "glGetShaderInfoLog");
    LOAD_GL(gl_delete_shader, "glDeleteShader");
    LOAD_GL(gl_create_program, "glCreateProgram");
    LOAD_GL(gl_attach_shader, "glAttachShader");
    LOAD_GL(gl_link_program, "glLinkProgram");
    LOAD_GL(gl_get_program_iv, "glGetProgramiv");
    LOAD_GL(gl_get_program_info_log, "glGetProgramInfoLog");
    LOAD_GL(gl_delete_program, "glDeleteProgram");
    LOAD_GL(gl_use_program, "glUseProgram");
    LOAD_GL(gl_get_uniform_location, "glGetUniformLocation");
    LOAD_GL(gl_uniform_1i, "glUniform1i");
    LOAD_GL(gl_uniform_1f, "glUniform1f");
    LOAD_GL(gl_uniform_4f, "glUniform4f");
    LOAD_GL(gl_blend_func_separate, "glBlendFuncSeparate");
    LOAD_GL(gl_blend_equation_separate, "glBlendEquationSeparate");
    LOAD_GL(gl_draw_arrays, "glDrawArrays");
#undef LOAD_GL
    api.ready = api.get_display != nullptr && api.initialize != nullptr &&
                api.choose_config != nullptr &&
                api.get_configs != nullptr &&
                api.get_config_attrib != nullptr &&
                api.create_context != nullptr &&
                api.create_pbuffer_surface != nullptr &&
                api.create_pbuffer_from_client_buffer != nullptr &&
                api.make_current != nullptr && api.destroy_context != nullptr &&
                api.destroy_surface != nullptr && api.terminate != nullptr &&
                api.swap_buffers != nullptr && api.bind_tex_image != nullptr &&
                api.release_tex_image != nullptr &&
                api.query_context != nullptr &&
                api.query_surface != nullptr && api.query_string != nullptr &&
                api.get_current_display != nullptr &&
                api.get_current_context != nullptr &&
                api.get_current_surface != nullptr && api.get_error != nullptr &&
                api.get_proc_address != nullptr &&
                api.release_thread != nullptr && api.wait_gl != nullptr &&
                api.wait_native != nullptr && api.gl_pixel_store_i != nullptr &&
                api.gl_disable != nullptr && api.gl_depth_mask != nullptr &&
                api.gl_enable != nullptr && api.gl_is_enabled != nullptr &&
                api.gl_scissor != nullptr &&
                api.gl_clear_color != nullptr && api.gl_clear != nullptr &&
                api.gl_viewport != nullptr && api.gl_is_texture != nullptr &&
                api.gl_gen_textures != nullptr &&
                api.gl_active_texture != nullptr &&
                api.gl_bind_texture != nullptr &&
                api.gl_tex_parameter_i != nullptr &&
                api.gl_get_error != nullptr &&
                api.gl_delete_textures != nullptr &&
                api.gl_tex_image_2d != nullptr &&
                api.gl_tex_sub_image_2d != nullptr &&
                api.gl_get_integer_v != nullptr &&
                api.gl_read_pixels != nullptr &&
                api.gl_gen_framebuffers != nullptr &&
                api.gl_bind_framebuffer != nullptr &&
                api.gl_framebuffer_texture_2d != nullptr &&
                api.gl_check_framebuffer_status != nullptr &&
                api.gl_delete_framebuffers != nullptr &&
                api.gl_blit_framebuffer_angle != nullptr;
    if (!api.ready) {
      std::cerr << "ART Android EGL: ANGLE export set is incomplete\n";
    }
  });
  return api;
}

darwin_art::graphics::EglWindowBackendTable MakeWindowBackendTable(
    const AngleApi& api) {
  darwin_art::graphics::EglWindowBackendTable table;
  table.initialize = api.initialize;
  table.terminate = api.terminate;
  table.create_pbuffer_surface = api.create_pbuffer_surface;
  table.create_pbuffer_from_client_buffer = api.create_pbuffer_from_client_buffer;
  table.destroy_surface = api.destroy_surface;
  table.swap_buffers = api.swap_buffers;
  table.bind_tex_image = api.bind_tex_image;
  table.release_tex_image = api.release_tex_image;
  table.wait_gl = api.wait_gl;
  table.release_thread = api.release_thread;
  table.get_config_attrib = api.get_config_attrib;
  table.get_error = api.get_error;
  table.gl_get_integer_v = api.gl_get_integer_v;
  table.gl_is_enabled = api.gl_is_enabled;
  table.gl_disable = api.gl_disable;
  table.gl_enable = api.gl_enable;
  table.gl_scissor = api.gl_scissor;
  table.gl_bind_texture = api.gl_bind_texture;
  table.gl_active_texture = api.gl_active_texture;
  table.gl_tex_parameter_i = api.gl_tex_parameter_i;
  table.gl_gen_textures = api.gl_gen_textures;
  table.gl_delete_textures = api.gl_delete_textures;
  table.gl_gen_framebuffers = api.gl_gen_framebuffers;
  table.gl_bind_framebuffer = api.gl_bind_framebuffer;
  table.gl_framebuffer_texture_2d = api.gl_framebuffer_texture_2d;
  table.gl_check_framebuffer_status = api.gl_check_framebuffer_status;
  table.gl_delete_framebuffers = api.gl_delete_framebuffers;
  table.gl_blit_framebuffer = api.gl_blit_framebuffer_angle;
  table.gl_get_error = api.gl_get_error;
  return table;
}

void* BackendCurrentHost(void*) { return darwin_art_surface_active_gpu(); }
bool BackendAcquireIosurface(void*, void* host, void** iosurface,
                             std::uint32_t* width, std::uint32_t* height) {
  return host != nullptr && darwin_art_surface_gpu_acquire_iosurface(
                                 static_cast<DarwinArtSurface*>(host), iosurface,
                                 width, height);
}
bool BackendLookupIosurface(void*, std::uint32_t id, void** iosurface,
                            std::uint32_t* width, std::uint32_t* height) {
  return darwin_art_surface_gpu_lookup_iosurface(id, iosurface, width, height);
}
void BackendReleaseIosurface(void*, void* iosurface) {
  darwin_art_surface_gpu_release_iosurface(iosurface);
}
void BackendEmbeddedGeometry(void*, void* host, std::int32_t* x,
                             std::int32_t* y, std::uint32_t* width,
                             std::uint32_t* height) {
  darwin_art_surface_gpu_get_embedded_geometry(
      static_cast<DarwinArtSurface*>(host), x, y, width, height);
}
void BackendSetEmbeddedExtent(void*, void* host, std::uint32_t width,
                              std::uint32_t height) {
  darwin_art_surface_gpu_set_embedded_buffer_extent(
      static_cast<DarwinArtSurface*>(host), width, height);
}
void BackendPublishEmbedded(void*, void* host) {
  darwin_art_surface_gpu_publish_embedded(static_cast<DarwinArtSurface*>(host));
}
void BackendNativeAcquire(void*, void* window) {
  darwin_art_android_ANativeWindow_acquire(window);
}
void BackendNativeRelease(void*, void* window) {
  darwin_art_android_ANativeWindow_release(window);
}
int BackendNativeDequeue(void*, void* window, void** hardware_buffer,
                         void** native_buffer, int* acquire_fence) {
  return darwin_art_android_ANativeWindow_dequeue_hardware_buffer(
      window, reinterpret_cast<AHardwareBuffer**>(hardware_buffer),
      native_buffer, acquire_fence);
}
int BackendNativeQueue(void*, void* window, void* native_buffer, int fence) {
  return darwin_art_android_ANativeWindow_queue_hardware_buffer(
      window, native_buffer, fence);
}
int BackendNativeCancel(void*, void* window, void* native_buffer, int fence) {
  return darwin_art_android_ANativeWindow_cancel_hardware_buffer(
      window, native_buffer, fence);
}
void BackendDescribeBuffer(void*, void* buffer, std::uint32_t* width,
                           std::uint32_t* height) {
  AHardwareBuffer_Desc description{};
  AHardwareBuffer_describe(static_cast<AHardwareBuffer*>(buffer), &description);
  if (width != nullptr) *width = description.width;
  if (height != nullptr) *height = description.height;
}
void* BackendBufferIosurface(void*, void* buffer) {
  return darwin_art_android_hardware_buffer_iosurface(
      static_cast<AHardwareBuffer*>(buffer));
}
bool BackendResetComposition(void*, void* display) {
  return darwin_art::graphics::ResetComposition(darwin_art::CompositionBackend(),
                                                display);
}
int BackendWaitFence(void*, int fd, int timeout_ms) {
  return sync_wait(fd, timeout_ms);
}
int BackendCloseFence(void*, int fd) {
  return darwin_art_bionic_socket_broker_close(fd);
}

darwin_art::graphics::EglWindowBackend& WindowBackend() {
  static const darwin_art::graphics::EglWindowBackend backend(
      MakeWindowBackendTable(GetAngleApi()),
      darwin_art::graphics::EglWindowBackendResources{
          .context = nullptr,
          .current_host = &BackendCurrentHost,
          .acquire_iosurface = &BackendAcquireIosurface,
          .lookup_iosurface = &BackendLookupIosurface,
          .release_iosurface = &BackendReleaseIosurface,
          .embedded_geometry = &BackendEmbeddedGeometry,
          .set_embedded_extent = &BackendSetEmbeddedExtent,
          .publish_embedded = &BackendPublishEmbedded,
          .native_acquire = &BackendNativeAcquire,
          .native_release = &BackendNativeRelease,
          .native_dequeue = &BackendNativeDequeue,
          .native_queue = &BackendNativeQueue,
          .native_cancel = &BackendNativeCancel,
          .describe_buffer = &BackendDescribeBuffer,
          .buffer_iosurface = &BackendBufferIosurface,
          .reset_composition = &BackendResetComposition,
          .wait_fence = &BackendWaitFence,
          .close_fence = &BackendCloseFence});
  return const_cast<darwin_art::graphics::EglWindowBackend&>(backend);
}

void EglNativeClassInit(JNIEnv*, jclass) { (void)GetAngleApi(); }
void GlNativeClassInit(JNIEnv*, jclass) { (void)GetAngleApi(); }

jlong GetHandle(JNIEnv* env, jobject object, const char* field_name) {
  if (object == nullptr) return 0;
  jclass klass = env->GetObjectClass(object);
  jfieldID field =
      klass == nullptr ? nullptr : env->GetFieldID(klass, field_name, "J");
  const jlong result =
      field == nullptr || env->ExceptionCheck()
          ? 0
          : env->GetLongField(object, field);
  if (env->ExceptionCheck()) env->ExceptionClear();
  if (klass != nullptr) env->DeleteLocalRef(klass);
  return result;
}

template <typename T>
T HandleAs(JNIEnv* env, jobject object, const char* field_name) {
  return reinterpret_cast<T>(static_cast<std::uintptr_t>(
      GetHandle(env, object, field_name)));
}

std::vector<EGLint> CopyAttributes(JNIEnv* env, jintArray attributes) {
  if (attributes == nullptr) return {};
  const jsize count = env->GetArrayLength(attributes);
  std::vector<EGLint> result(static_cast<std::size_t>(std::max(0, count)));
  if (count > 0) env->GetIntArrayRegion(attributes, 0, count, result.data());
  return result;
}

jobject NewEglConfig(JNIEnv* env, EGLConfig config) {
  jclass klass = env->FindClass("com/google/android/gles_jni/EGLConfigImpl");
  jmethodID constructor =
      klass == nullptr ? nullptr : env->GetMethodID(klass, "<init>", "(J)V");
  jobject result =
      constructor == nullptr
          ? nullptr
          : env->NewObject(klass, constructor,
                           static_cast<jlong>(reinterpret_cast<std::uintptr_t>(
                               config)));
  if (klass != nullptr) env->DeleteLocalRef(klass);
  return result;
}

jlong EglCreateContext(JNIEnv* env, jobject, jobject display, jobject config,
                       jobject share, jintArray attributes) {
  auto& api = GetAngleApi();
  if (!api.ready) return 0;
  const auto values = CopyAttributes(env, attributes);
  EGLContext context = darwin_art::graphics::DispatchCreateContext(
      HandleAs<EGLDisplay>(env, display, "mEGLDisplay"),
      HandleAs<EGLConfig>(env, config, "mEGLConfig"),
      HandleAs<EGLContext>(env, share, "mEGLContext"),
      values.empty() ? nullptr : values.data(), api.create_context, false);
  if (context == nullptr && api.get_error != nullptr)
    darwin_art::graphics::SetEglError(api.get_error());
  return static_cast<jlong>(reinterpret_cast<std::uintptr_t>(context));
}

jlong EglCreatePbufferSurface(JNIEnv* env, jobject, jobject display,
                              jobject config, jintArray attributes) {
  auto& api = GetAngleApi();
  if (!api.ready) return 0;
  const auto values = CopyAttributes(env, attributes);
  EGLSurface surface = api.create_pbuffer_surface(
      HandleAs<EGLDisplay>(env, display, "mEGLDisplay"),
      HandleAs<EGLConfig>(env, config, "mEGLConfig"),
      values.empty() ? nullptr : values.data());
  return static_cast<jlong>(reinterpret_cast<std::uintptr_t>(surface));
}

jlong EglCreateWindowSurface(JNIEnv* env, jobject, jobject display,
                             jobject config, jobject, jintArray) {
  const auto result = WindowBackend().CreateWindowWithError(
      HandleAs<EGLDisplay>(env, display, "mEGLDisplay"),
      HandleAs<EGLConfig>(env, config, "mEGLConfig"), nullptr);
  return static_cast<jlong>(reinterpret_cast<std::uintptr_t>(result.surface));
}

void EglCreatePixmapSurface(JNIEnv* env, jobject self, jobject result,
                            jobject display, jobject config, jobject native,
                            jintArray attributes) {
  const jlong surface = EglCreateWindowSurface(
      env, self, display, config, native, attributes);
  jclass klass = result == nullptr ? nullptr : env->GetObjectClass(result);
  jfieldID field =
      klass == nullptr ? nullptr : env->GetFieldID(klass, "mEGLSurface", "J");
  if (field != nullptr && !env->ExceptionCheck()) {
    env->SetLongField(result, field, surface);
  }
  if (klass != nullptr) env->DeleteLocalRef(klass);
}

jlong EglGetCurrentContext(JNIEnv*, jobject) {
  auto& api = GetAngleApi();
  return !api.ready
             ? 0
             : static_cast<jlong>(reinterpret_cast<std::uintptr_t>(
                   api.get_current_context()));
}
jlong EglGetCurrentDisplay(JNIEnv*, jobject) {
  auto& api = GetAngleApi();
  return !api.ready
             ? 0
             : static_cast<jlong>(reinterpret_cast<std::uintptr_t>(
                   api.get_current_display()));
}
jlong EglGetCurrentSurface(JNIEnv*, jobject, jint selector) {
  auto& api = GetAngleApi();
  return !api.ready
             ? 0
             : static_cast<jlong>(reinterpret_cast<std::uintptr_t>(
                   api.get_current_surface(selector)));
}
jlong EglGetDisplay(JNIEnv*, jobject, jobject) {
  auto& api = GetAngleApi();
  return !api.ready
             ? 0
             : static_cast<jlong>(reinterpret_cast<std::uintptr_t>(
                   api.get_display(nullptr)));
}
jint EglGetInitCount(JNIEnv*, jclass, jobject) {
  return GetAngleApi().ready ? 1 : 0;
}

jboolean EglChooseConfig(JNIEnv* env, jobject, jobject display,
                         jintArray attributes, jobjectArray configs,
                         jint config_size, jintArray count_out) {
  auto& api = GetAngleApi();
  if (!api.ready) return JNI_FALSE;
  const auto values = CopyAttributes(env, attributes);
  const EGLint capacity = std::max<jint>(0, config_size);
  std::vector<EGLConfig> native_configs(static_cast<std::size_t>(capacity));
  EGLint count = 0;
  const EGLBoolean ok = api.choose_config(
      HandleAs<EGLDisplay>(env, display, "mEGLDisplay"),
      values.empty() ? nullptr : values.data(),
      native_configs.empty() ? nullptr : native_configs.data(), capacity,
      &count);
  if (count_out != nullptr && env->GetArrayLength(count_out) > 0) {
    env->SetIntArrayRegion(count_out, 0, 1, &count);
  }
  const jsize output_capacity =
      configs == nullptr ? 0 : env->GetArrayLength(configs);
  const jint output_count =
      std::min<jint>({count, capacity, output_capacity});
  for (jint index = 0; index < output_count && !env->ExceptionCheck(); ++index) {
    jobject config = NewEglConfig(env, native_configs[index]);
    if (config == nullptr) return JNI_FALSE;
    env->SetObjectArrayElement(configs, index, config);
    env->DeleteLocalRef(config);
  }
  return ok ? JNI_TRUE : JNI_FALSE;
}

jboolean EglGetConfigs(JNIEnv* env, jobject, jobject display,
                       jobjectArray configs, jint config_size,
                       jintArray count_out) {
  auto& api = GetAngleApi();
  if (!api.ready) return JNI_FALSE;
  const EGLint capacity = std::max<jint>(0, config_size);
  std::vector<EGLConfig> native_configs(static_cast<std::size_t>(capacity));
  EGLint count = 0;
  const EGLBoolean ok = api.get_configs(
      HandleAs<EGLDisplay>(env, display, "mEGLDisplay"),
      native_configs.empty() ? nullptr : native_configs.data(), capacity,
      &count);
  if (count_out != nullptr && env->GetArrayLength(count_out) > 0) {
    env->SetIntArrayRegion(count_out, 0, 1, &count);
  }
  const jsize output_capacity =
      configs == nullptr ? 0 : env->GetArrayLength(configs);
  const jint output_count =
      std::min<jint>({count, capacity, output_capacity});
  for (jint index = 0; index < output_count && !env->ExceptionCheck(); ++index) {
    jobject config = NewEglConfig(env, native_configs[index]);
    if (config == nullptr) return JNI_FALSE;
    env->SetObjectArrayElement(configs, index, config);
    env->DeleteLocalRef(config);
  }
  return ok ? JNI_TRUE : JNI_FALSE;
}

jboolean EglInitialize(JNIEnv* env, jobject, jobject display,
                       jintArray version) {
  auto& api = GetAngleApi();
  if (!api.ready) return JNI_FALSE;
  EGLint major = 0;
  EGLint minor = 0;
  const bool ok = WindowBackend().Initialize(
      HandleAs<EGLDisplay>(env, display, "mEGLDisplay"), &major, &minor);
  if (!ok) return JNI_FALSE;
  if (version != nullptr && env->GetArrayLength(version) >= 2) {
    const jint values[] = {major, minor};
    env->SetIntArrayRegion(version, 0, 2, values);
  }
  return JNI_TRUE;
}

jboolean EglGetConfigAttrib(JNIEnv* env, jobject, jobject display,
                            jobject config, jint attribute,
                            jintArray value_out) {
  auto& api = GetAngleApi();
  if (!api.ready || value_out == nullptr ||
      env->GetArrayLength(value_out) == 0) {
    return JNI_FALSE;
  }
  EGLint value = 0;
  const EGLBoolean ok = api.get_config_attrib(
      HandleAs<EGLDisplay>(env, display, "mEGLDisplay"),
      HandleAs<EGLConfig>(env, config, "mEGLConfig"), attribute, &value);
  if (ok) env->SetIntArrayRegion(value_out, 0, 1, &value);
  return ok ? JNI_TRUE : JNI_FALSE;
}

jboolean EglMakeCurrent(JNIEnv* env, jobject, jobject display, jobject draw,
                        jobject read, jobject context) {
  auto& api = GetAngleApi();
  if (!api.ready) return JNI_FALSE;
  return darwin_art::graphics::DispatchMakeCurrent(
             HandleAs<EGLDisplay>(env, display, "mEGLDisplay"),
             HandleAs<EGLSurface>(env, draw, "mEGLSurface"),
             HandleAs<EGLSurface>(env, read, "mEGLSurface"),
             HandleAs<EGLContext>(env, context, "mEGLContext"),
             api.make_current, false)
         ? JNI_TRUE
         : JNI_FALSE;
}

jboolean EglDestroyContext(JNIEnv* env, jobject, jobject display,
                           jobject context) {
  auto& api = GetAngleApi();
  return api.ready && api.destroy_context(
                          HandleAs<EGLDisplay>(env, display, "mEGLDisplay"),
                          HandleAs<EGLContext>(env, context, "mEGLContext"))
             ? JNI_TRUE
             : JNI_FALSE;
}
bool DestroyHostWindowSurface(EGLDisplay native_display,
                              EGLSurface native_surface) {
  return WindowBackend().Destroy(native_display, native_surface);
}
jboolean EglDestroySurface(JNIEnv* env, jobject, jobject display,
                           jobject surface) {
  return DestroyHostWindowSurface(
             HandleAs<EGLDisplay>(env, display, "mEGLDisplay"),
             HandleAs<EGLSurface>(env, surface, "mEGLSurface"))
             ? JNI_TRUE
             : JNI_FALSE;
}
EGLBoolean TerminateHostDisplay(EGLDisplay native_display) {
  return WindowBackend().Terminate(native_display) ? 1u : 0u;
}
jboolean EglTerminate(JNIEnv* env, jobject, jobject display) {
  return TerminateHostDisplay(
      HandleAs<EGLDisplay>(env, display, "mEGLDisplay"))
             ? JNI_TRUE
             : JNI_FALSE;
}
bool SwapHostWindowSurface(EGLDisplay native_display,
                           EGLSurface native_surface) {
  return WindowBackend().Swap(native_display, native_surface);
}
jboolean EglSwapBuffers(JNIEnv* env, jobject, jobject display,
                        jobject surface) {
  return SwapHostWindowSurface(
             HandleAs<EGLDisplay>(env, display, "mEGLDisplay"),
             HandleAs<EGLSurface>(env, surface, "mEGLSurface"))
             ? JNI_TRUE
             : JNI_FALSE;
}
jboolean EglQueryContext(JNIEnv* env, jobject, jobject display, jobject context,
                         jint attribute, jintArray value_out) {
  auto& api = GetAngleApi();
  if (!api.ready || value_out == nullptr ||
      env->GetArrayLength(value_out) == 0) {
    return JNI_FALSE;
  }
  EGLint value = 0;
  const EGLBoolean ok = api.query_context(
      HandleAs<EGLDisplay>(env, display, "mEGLDisplay"),
      HandleAs<EGLContext>(env, context, "mEGLContext"), attribute, &value);
  if (ok) env->SetIntArrayRegion(value_out, 0, 1, &value);
  return ok ? JNI_TRUE : JNI_FALSE;
}
jboolean EglQuerySurface(JNIEnv* env, jobject, jobject display, jobject surface,
                         jint attribute, jintArray value_out) {
  auto& api = GetAngleApi();
  if (!api.ready || value_out == nullptr ||
      env->GetArrayLength(value_out) == 0) {
    return JNI_FALSE;
  }
  EGLint value = 0;
  const EGLBoolean ok = api.query_surface(
      HandleAs<EGLDisplay>(env, display, "mEGLDisplay"),
      HandleAs<EGLSurface>(env, surface, "mEGLSurface"), attribute, &value);
  if (ok) env->SetIntArrayRegion(value_out, 0, 1, &value);
  return ok ? JNI_TRUE : JNI_FALSE;
}
jstring EglQueryString(JNIEnv* env, jobject, jobject display, jint name) {
  const char* value = darwin_art::EglQueryStringAndroid(
      HandleAs<EGLDisplay>(env, display, "mEGLDisplay"), name);
  return value == nullptr ? nullptr : env->NewStringUTF(value);
}
jint EglGetError(JNIEnv*, jobject) {
  auto& api = GetAngleApi();
  if (!api.ready) darwin_art::graphics::SetEglError(kEglNotInitialized);
  return darwin_art::graphics::ConsumeEglError();
}
EGLBoolean ReleaseHostThread() {
  return WindowBackend().ReleaseThread() ? 1u : 0u;
}
jboolean EglReleaseThread(JNIEnv*, jobject) {
  return ReleaseHostThread() ? JNI_TRUE : JNI_FALSE;
}
jboolean EglWaitGl(JNIEnv*, jobject) {
  auto& api = GetAngleApi();
  return api.ready && api.wait_gl() ? JNI_TRUE : JNI_FALSE;
}
jboolean EglWaitNative(JNIEnv*, jobject, jint engine, jobject) {
  auto& api = GetAngleApi();
  return api.ready && api.wait_native(engine) ? JNI_TRUE : JNI_FALSE;
}
jboolean EglCopyBuffers(JNIEnv*, jobject, jobject, jobject, jobject) {
  return JNI_FALSE;
}

void GlesPixelStoreI(JNIEnv*, jclass, jint name, jint value) {
  auto& api = GetAngleApi();
  if (api.ready) api.gl_pixel_store_i(name, value);
}
void GlesDisable(JNIEnv*, jclass, jint cap) {
  auto& api = GetAngleApi();
  if (api.ready) api.gl_disable(cap);
}
void GlesDepthMask(JNIEnv*, jclass, jboolean enabled) {
  auto& api = GetAngleApi();
  if (api.ready) api.gl_depth_mask(enabled == JNI_TRUE ? 1 : 0);
}
void GlesClearColor(JNIEnv*, jclass, jfloat red, jfloat green, jfloat blue,
                    jfloat alpha) {
  auto& api = GetAngleApi();
  if (api.ready) api.gl_clear_color(red, green, blue, alpha);
}
void GlesClear(JNIEnv*, jclass, jint mask) {
  auto& api = GetAngleApi();
  if (api.ready) api.gl_clear(mask);
}
void GlesViewport(JNIEnv*, jclass, jint x, jint y, jint width, jint height) {
  auto& api = GetAngleApi();
  if (api.ready) api.gl_viewport(x, y, width, height);
}
jboolean GlesIsTexture(JNIEnv*, jclass, jint texture) {
  auto& api = GetAngleApi();
  return api.ready && api.gl_is_texture(static_cast<std::uint32_t>(texture))
             ? JNI_TRUE
             : JNI_FALSE;
}
void GlesGenTextures(JNIEnv* env, jclass, jint count, jintArray textures,
                     jint offset) {
  auto& api = GetAngleApi();
  if (!api.ready || textures == nullptr || count < 0 || offset < 0 ||
      offset > env->GetArrayLength(textures) - count) {
    return;
  }
  std::vector<std::uint32_t> values(static_cast<std::size_t>(count));
  api.gl_gen_textures(count, values.data());
  env->SetIntArrayRegion(textures, offset, count,
                         reinterpret_cast<const jint*>(values.data()));
}
void GlesActiveTexture(JNIEnv*, jclass, jint texture) {
  auto& api = GetAngleApi();
  if (api.ready) api.gl_active_texture(static_cast<std::uint32_t>(texture));
}
void GlesBindTexture(JNIEnv*, jclass, jint target, jint texture) {
  auto& api = GetAngleApi();
  if (api.ready) {
    api.gl_bind_texture(static_cast<std::uint32_t>(target),
                        static_cast<std::uint32_t>(texture));
  }
}
void GlesTexParameterI(JNIEnv*, jclass, jint target, jint name, jint value) {
  auto& api = GetAngleApi();
  if (api.ready) {
    api.gl_tex_parameter_i(static_cast<std::uint32_t>(target),
                           static_cast<std::uint32_t>(name), value);
  }
}
jint GlesGetError(JNIEnv*, jclass) {
  auto& api = GetAngleApi();
  return api.ready ? static_cast<jint>(api.gl_get_error()) : kEglNotInitialized;
}
void GlesDeleteTextures(JNIEnv* env, jclass, jint count, jintArray textures,
                        jint offset) {
  auto& api = GetAngleApi();
  if (!api.ready || textures == nullptr || count < 0 || offset < 0 ||
      offset > env->GetArrayLength(textures) - count) {
    return;
  }
  std::vector<jint> values(static_cast<std::size_t>(count));
  env->GetIntArrayRegion(textures, offset, count, values.data());
  api.gl_delete_textures(
      count, reinterpret_cast<const std::uint32_t*>(values.data()));
}

struct BitmapGlFormat {
  jint format;
  jint type;
  std::size_t bytes_per_pixel;
};

bool GetBitmapGlFormat(const AndroidBitmapInfo& info, BitmapGlFormat* out) {
  if (out == nullptr) return false;
  switch (info.format) {
    case ANDROID_BITMAP_FORMAT_RGBA_8888:
      *out = {0x1908, 0x1401, 4};  // GL_RGBA, GL_UNSIGNED_BYTE
      return true;
    case ANDROID_BITMAP_FORMAT_RGB_565:
      *out = {0x1907, 0x8363, 2};  // GL_RGB, GL_UNSIGNED_SHORT_5_6_5
      return true;
    case ANDROID_BITMAP_FORMAT_RGBA_4444:
      *out = {0x1908, 0x8033, 2};  // GL_RGBA, GL_UNSIGNED_SHORT_4_4_4_4
      return true;
    case ANDROID_BITMAP_FORMAT_A_8:
      *out = {0x1906, 0x1401, 1};  // GL_ALPHA, GL_UNSIGNED_BYTE
      return true;
    default:
      return false;
  }
}

const void* CompactBitmapPixels(const AndroidBitmapInfo& info,
                                const BitmapGlFormat& format,
                                const void* pixels,
                                std::vector<std::uint8_t>* compact) {
  const std::size_t row_bytes =
      static_cast<std::size_t>(info.width) * format.bytes_per_pixel;
  if (info.stride == row_bytes) return pixels;
  compact->resize(row_bytes * static_cast<std::size_t>(info.height));
  const auto* source = static_cast<const std::uint8_t*>(pixels);
  for (std::uint32_t row = 0; row < info.height; ++row) {
    std::copy_n(source + static_cast<std::size_t>(row) * info.stride, row_bytes,
                compact->data() + static_cast<std::size_t>(row) * row_bytes);
  }
  return compact->data();
}

jint GlUtilsGetInternalFormat(JNIEnv* env, jclass, jobject bitmap) {
  AndroidBitmapInfo info = {};
  BitmapGlFormat format = {};
  return AndroidBitmap_getInfo(env, bitmap, &info) ==
                 ANDROID_BITMAP_RESULT_SUCCESS &&
             GetBitmapGlFormat(info, &format)
         ? format.format
         : -1;
}

jint GlUtilsGetType(JNIEnv* env, jclass, jobject bitmap) {
  AndroidBitmapInfo info = {};
  BitmapGlFormat format = {};
  return AndroidBitmap_getInfo(env, bitmap, &info) ==
                 ANDROID_BITMAP_RESULT_SUCCESS &&
             GetBitmapGlFormat(info, &format)
         ? format.type
         : -1;
}

jint GlUtilsTexImage2D(JNIEnv* env, jclass, jint target, jint level,
                       jint internal_format, jobject bitmap, jint type,
                       jint border) {
  auto& api = GetAngleApi();
  AndroidBitmapInfo info = {};
  BitmapGlFormat format = {};
  void* pixels = nullptr;
  if (!api.ready ||
      AndroidBitmap_getInfo(env, bitmap, &info) !=
          ANDROID_BITMAP_RESULT_SUCCESS ||
      !GetBitmapGlFormat(info, &format) ||
      AndroidBitmap_lockPixels(env, bitmap, &pixels) !=
          ANDROID_BITMAP_RESULT_SUCCESS ||
      pixels == nullptr) {
    return -1;
  }
  std::vector<std::uint8_t> compact;
  const void* upload = CompactBitmapPixels(info, format, pixels, &compact);
  api.gl_tex_image_2d(
      static_cast<std::uint32_t>(target), level,
      internal_format < 0 ? format.format : internal_format, info.width,
      info.height, border, static_cast<std::uint32_t>(format.format),
      static_cast<std::uint32_t>(type < 0 ? format.type : type), upload);
  AndroidBitmap_unlockPixels(env, bitmap);
  return 0;
}

jint GlUtilsTexSubImage2D(JNIEnv* env, jclass, jint target, jint level,
                          jint x_offset, jint y_offset, jobject bitmap,
                          jint requested_format, jint type) {
  auto& api = GetAngleApi();
  AndroidBitmapInfo info = {};
  BitmapGlFormat format = {};
  void* pixels = nullptr;
  if (!api.ready ||
      AndroidBitmap_getInfo(env, bitmap, &info) !=
          ANDROID_BITMAP_RESULT_SUCCESS ||
      !GetBitmapGlFormat(info, &format) ||
      AndroidBitmap_lockPixels(env, bitmap, &pixels) !=
          ANDROID_BITMAP_RESULT_SUCCESS ||
      pixels == nullptr) {
    return -1;
  }
  std::vector<std::uint8_t> compact;
  const void* upload = CompactBitmapPixels(info, format, pixels, &compact);
  api.gl_tex_sub_image_2d(
      static_cast<std::uint32_t>(target), level, x_offset, y_offset, info.width,
      info.height,
      static_cast<std::uint32_t>(requested_format < 0 ? format.format
                                                       : requested_format),
      static_cast<std::uint32_t>(type < 0 ? format.type : type), upload);
  AndroidBitmap_unlockPixels(env, bitmap);
  return 0;
}

bool Register(JNIEnv* env, const char* class_name, JNINativeMethod* methods,
              jint count) {
  jclass klass = env->FindClass(class_name);
  if (klass == nullptr) return false;
  const bool ok = env->RegisterNatives(klass, methods, count) == JNI_OK;
  env->DeleteLocalRef(klass);
  return ok;
}

}  // namespace

namespace darwin_art {

// Public platform-dispatch entry point.  The JNI EGLImpl path above and the
// ELF resolver both use the same WindowBackend owner; this declaration gives
// the standard C EGL wrapper a stable cross-TU forwarding target without
// exposing the internal backend or creating another registry.
std::uint32_t TerminateHostDisplay(void* display) {
  return WindowBackend().Terminate(display) ? 1u : 0u;
}

}  // namespace darwin_art

extern "C" void* darwin_art_android_eglCreateWindowSurface(
    void* display, void* config, void* native_window, const int32_t*) {
  const auto result =
      WindowBackend().CreateWindowWithError(display, config, native_window);
  void* surface = result.surface;
  if (std::getenv("DARWIN_ART_DEBUG_GRAPHICS_DSO") != nullptr) {
    std::cerr << "ART Android EGL: eglCreateWindowSurface window="
              << native_window << " result=" << surface << " error=0x"
              << std::hex << (surface == nullptr
                                   ? darwin_art::graphics::PeekEglError()
                                   : 0)
              << std::dec << "\n";
  }
  return surface;
}

extern "C" uint32_t darwin_art_android_eglSwapBuffers(void* display,
                                                        void* surface) {
  return SwapHostWindowSurface(display, surface) ? 1u : 0u;
}

extern "C" uint32_t darwin_art_android_eglDestroySurface(void* display,
                                                           void* surface) {
  return DestroyHostWindowSurface(display, surface) ? 1u : 0u;
}

extern "C" uint32_t darwin_art_android_eglMakeCurrent(
    void* display, void* draw, void* read, void* context) {
  auto& api = GetAngleApi();
  const uint32_t result =
      api.ready && darwin_art::graphics::DispatchMakeCurrent(
                       display, draw, read, context, api.make_current,
                       std::getenv("DARWIN_ART_DEBUG_GRAPHICS_DSO") != nullptr)
          ? 1u
          : 0u;
  return result;
}

extern "C" uint32_t darwin_art_android_eglQuerySurface(
    void* display, void* surface, int32_t attribute, int32_t* value) {
  auto& api = GetAngleApi();
  const uint32_t result =
      api.ready && api.query_surface(display, surface, attribute, value) ? 1u
                                                                        : 0u;
  if (std::getenv("DARWIN_ART_DEBUG_GRAPHICS_DSO") != nullptr) {
    std::cerr << "ART Android EGL: eglQuerySurface surface=" << surface
              << " attribute=0x" << std::hex << attribute << std::dec
              << " value=" << (value == nullptr ? -1 : *value)
              << " result=" << result << "\n";
  }
  return result;
}

extern "C" uint32_t darwin_art_android_eglSurfaceAttrib(
    void* display, void* surface, int32_t attribute, int32_t value) {
  using Function = EGLBoolean (*)(EGLDisplay, EGLSurface, EGLint, EGLint);
  auto function = LoadSymbol<Function>(GetAngleApi().egl_library,
                                       "eglSurfaceAttrib");
  const uint32_t result =
      function != nullptr && function(display, surface, attribute, value) ? 1u
                                                                          : 0u;
  if (std::getenv("DARWIN_ART_DEBUG_GRAPHICS_DSO") != nullptr) {
    std::cerr << "ART Android EGL: eglSurfaceAttrib surface=" << surface
              << " attribute=0x" << std::hex << attribute << " value=0x"
              << value << std::dec << " result=" << result << "\n";
  }
  return result;
}

extern "C" uint32_t darwin_art_android_eglSwapInterval(void* display,
                                                         int32_t interval) {
  using Function = EGLBoolean (*)(EGLDisplay, EGLint);
  auto function =
      LoadSymbol<Function>(GetAngleApi().egl_library, "eglSwapInterval");
  return function != nullptr && function(display, interval) ? 1u : 0u;
}

extern "C" uint32_t darwin_art_android_eglSetDamageRegion(
    void*, void*, const int32_t*, int32_t) {
  // The persistent render pbuffer preserves untouched pixels. Every swap is
  // copied on the GPU into a freshly dequeued IOSurface, so Android damage is
  // scheduling metadata rather than an ANGLE pbuffer mutation.
  return 1u;
}

extern "C" void darwin_art_android_glTexImage2D(
    uint32_t target, int32_t level, int32_t internal_format, int32_t width,
    int32_t height, int32_t border, uint32_t format, uint32_t type,
    const void* pixels) {
  static std::atomic<uint32_t> calls{0};
  const uint32_t call = calls.fetch_add(1, std::memory_order_relaxed) + 1;
  if (call <= 12 || call % 120 == 0) {
    std::cerr << "ART Android GLES: glTexImage2D call=" << call << " size="
              << width << "x" << height << " internal=0x" << std::hex
              << internal_format << " format=0x" << format << " type=0x"
              << type << std::dec << " pixels=" << (pixels != nullptr)
              << "\n";
  }
  GetAngleApi().gl_tex_image_2d(target, level, internal_format, width, height,
                                border, format, type, pixels);
}

extern "C" void darwin_art_android_glTexSubImage2D(
    uint32_t target, int32_t level, int32_t x, int32_t y, int32_t width,
    int32_t height, uint32_t format, uint32_t type, const void* pixels) {
  static std::atomic<uint32_t> calls{0};
  const uint32_t call = calls.fetch_add(1, std::memory_order_relaxed) + 1;
  if (call <= 12 || call % 120 == 0) {
    uint8_t minimum = 255;
    uint8_t maximum = 0;
    uint32_t checksum = 2166136261u;
    const size_t sample_size =
        pixels == nullptr
            ? 0
            : std::min<size_t>(static_cast<size_t>(std::max(0, width)) *
                                   static_cast<size_t>(std::max(0, height)),
                               4096);
    const auto* sample = static_cast<const uint8_t*>(pixels);
    for (size_t index = 0; index < sample_size; ++index) {
      minimum = std::min(minimum, sample[index]);
      maximum = std::max(maximum, sample[index]);
      checksum = (checksum ^ sample[index]) * 16777619u;
    }
    std::cerr << "ART Android GLES: glTexSubImage2D call=" << call << " size="
              << width << "x" << height << " offset=" << x << "," << y
              << " format=0x" << std::hex << format << " type=0x" << type
              << " sample_crc=0x" << checksum << std::dec
              << " sample_minmax=" << static_cast<int>(minimum) << ","
              << static_cast<int>(maximum)
              << " pixels=" << (pixels != nullptr) << "\n";
  }
  GetAngleApi().gl_tex_sub_image_2d(target, level, x, y, width, height, format,
                                    type, pixels);
}

extern "C" void darwin_art_android_glDrawArrays(uint32_t mode, int32_t first,
                                                  int32_t count) {
  static std::atomic<uint32_t> calls{0};
  const uint32_t call = calls.fetch_add(1, std::memory_order_relaxed) + 1;
  if (call <= 12 || call % 120 == 0) {
    std::cerr << "ART Android GLES: glDrawArrays call=" << call << " mode=0x"
              << std::hex << mode << std::dec << " first=" << first
              << " count=" << count << "\n";
  }
  using Function = void (*)(uint32_t, int32_t, int32_t);
  static Function function =
      LoadSymbol<Function>(GetAngleApi().gles_library, "glDrawArrays");
  function(mode, first, count);
}

extern "C" void darwin_art_android_glDrawElements(
    uint32_t mode, int32_t count, uint32_t type, const void* indices) {
  static std::atomic<uint32_t> calls{0};
  const uint32_t call = calls.fetch_add(1, std::memory_order_relaxed) + 1;
  const bool sample = call <= 12 || call % 120 == 0;
  if (sample) {
    int32_t program = 0;
    int32_t array_buffer = 0;
    int32_t element_buffer = 0;
    int32_t viewport[4] = {};
    auto& api = GetAngleApi();
    api.gl_get_integer_v(0x8B8D, &program);  // GL_CURRENT_PROGRAM
    api.gl_get_integer_v(0x8894, &array_buffer);  // GL_ARRAY_BUFFER_BINDING
    api.gl_get_integer_v(0x8895, &element_buffer);  // GL_ELEMENT_ARRAY_BUFFER_BINDING
    api.gl_get_integer_v(0x0BA2, viewport);  // GL_VIEWPORT
    std::cerr << "ART Android GLES: glDrawElements call=" << call << " mode=0x"
              << std::hex << mode << " type=0x" << type << std::dec
              << " count=" << count << " indices=" << indices
              << " program=" << program << " buffers=" << array_buffer << ","
              << element_buffer << " viewport=" << viewport[0] << ","
              << viewport[1] << "," << viewport[2] << "x" << viewport[3]
              << "\n";
  }
  using Function = void (*)(uint32_t, int32_t, uint32_t, const void*);
  static Function function =
      LoadSymbol<Function>(GetAngleApi().gles_library, "glDrawElements");
  function(mode, count, type, indices);
  if (sample) {
    auto& api = GetAngleApi();
    int32_t viewport[4] = {};
    uint8_t pixel[4] = {};
    api.gl_get_integer_v(0x0BA2, viewport);
    api.gl_read_pixels(viewport[0] + viewport[2] / 2,
                       viewport[1] + viewport[3] / 2, 1, 1, 0x1908, 0x1401,
                       pixel);
    std::cerr << "ART Android GLES: draw result rgba="
              << static_cast<int>(pixel[0]) << ","
              << static_cast<int>(pixel[1]) << ","
              << static_cast<int>(pixel[2]) << ","
              << static_cast<int>(pixel[3]) << " error=0x" << std::hex
              << api.gl_get_error() << std::dec << "\n";
  }
}

extern "C" void darwin_art_android_glUseProgram(uint32_t program) {
  static std::atomic<uint32_t> calls{0};
  const uint32_t call = calls.fetch_add(1, std::memory_order_relaxed) + 1;
  if (call <= 12 || call % 120 == 0) {
    std::cerr << "ART Android GLES: glUseProgram call=" << call
              << " program=" << program << "\n";
  }
  using Function = void (*)(uint32_t);
  static Function function =
      LoadSymbol<Function>(GetAngleApi().gles_library, "glUseProgram");
  function(program);
}

namespace darwin_art {

darwin_art::graphics::EglAhbImageBackend AhbImageBackend() {
  auto& api = GetAngleApi();
  darwin_art::graphics::EglAhbImageBackend backend;
  // This is a borrowed view of the facade-owned ANGLE table. Keep all EGL and
  // GL resolution here; the image owner receives no loader or private state.
  backend.get_current_display = api.get_current_display;
  backend.get_current_context = api.get_current_context;
  backend.get_current_surface = api.get_current_surface;
  backend.choose_config = api.choose_config;
  backend.get_config_attrib = api.get_config_attrib;
  backend.create_pbuffer_from_client_buffer =
      api.create_pbuffer_from_client_buffer;
  backend.bind_tex_image = api.bind_tex_image;
  backend.release_tex_image = api.release_tex_image;
  backend.destroy_surface = api.destroy_surface;
  backend.get_error = api.get_error;
  backend.get_proc_address = api.get_proc_address;
  backend.gl_disable = api.gl_disable;
  backend.gl_enable = api.gl_enable;
  backend.gl_is_enabled = api.gl_is_enabled;
  backend.gl_get_integer_v = api.gl_get_integer_v;
  backend.gl_get_error = api.gl_get_error;
  backend.gl_gen_textures = api.gl_gen_textures;
  backend.gl_bind_texture = api.gl_bind_texture;
  backend.gl_tex_parameter_i = api.gl_tex_parameter_i;
  backend.gl_get_tex_parameter_iv = api.gl_get_tex_parameter_iv;
  backend.gl_delete_textures = api.gl_delete_textures;
  backend.gl_tex_image_2d = api.gl_tex_image_2d;
  backend.gl_gen_framebuffers = api.gl_gen_framebuffers;
  backend.gl_bind_framebuffer = api.gl_bind_framebuffer;
  backend.gl_framebuffer_texture_2d = api.gl_framebuffer_texture_2d;
  backend.gl_check_framebuffer_status = api.gl_check_framebuffer_status;
  backend.gl_get_framebuffer_attachment_parameter_iv =
      api.gl_get_framebuffer_attachment_parameter_iv;
  backend.gl_delete_framebuffers = api.gl_delete_framebuffers;
  backend.gl_blit_framebuffer_angle = api.gl_blit_framebuffer_angle;
  backend.gl_read_pixels = api.gl_read_pixels;
  backend.from_client_buffer =
      darwin_art_android_hardware_buffer_from_client_buffer;
  backend.ahb_describe = AHardwareBuffer_describe;
  backend.ahb_iosurface = darwin_art_android_hardware_buffer_iosurface;
  backend.ahb_metal_texture =
      darwin_art_android_hardware_buffer_metal_texture;
  backend.metal_texture_release = darwin_art_android_metal_texture_release;
  backend.ahb_acquire = AHardwareBuffer_acquire;
  backend.ahb_release = AHardwareBuffer_release;
  return backend;
}

namespace {
std::atomic<jint> g_host_surface_width{0};
std::atomic<jint> g_host_surface_height{0};
}  // namespace

void ConfigureDarwinAngleHostSurface(jint x, jint y, jint width, jint height) {
  if (width <= 0 || height <= 0) return;
  g_host_surface_width.store(width, std::memory_order_release);
  g_host_surface_height.store(height, std::memory_order_release);
  DarwinArtSurface* surface = darwin_art_surface_active_gpu();
  // Service children do not own the browser's embedded Metal surface, but
  // ANativeWindow still needs the dimensions transported in Surface's Parcel.
  if (surface == nullptr) return;
  darwin_art_surface_gpu_configure_embedded(
      surface, x, y, static_cast<uint32_t>(width),
      static_cast<uint32_t>(height));
  darwin_art_surface_gpu_set_embedded_buffer_extent(
      surface, static_cast<uint32_t>(width), static_cast<uint32_t>(height));
  darwin_art_surface_gpu_publish_embedded(surface);
  const uint32_t surface_id = darwin_art_surface_gpu_iosurface_id(surface);
  if (surface_id != 0) {
    const std::string encoded = std::to_string(surface_id);
    setenv("DARWIN_ART_HOST_IOSURFACE_ID", encoded.c_str(), 1);
  }
  if (std::getenv("DARWIN_ART_DEBUG_RESIZE") != nullptr) {
    std::cerr << "ART Android EGL: embedded geometry=" << x << "," << y
              << " " << width << "x" << height << "\n";
  }
}

void ConfigureDarwinAngleDisplayTarget(jint width, jint height) {
  if (width <= 0 || height <= 0) return;
  DarwinArtSurface* surface = darwin_art_surface_active_gpu();
  if (surface == nullptr) {
    // Headless/service children have no AppKit surface, but still publish the
    // logical Android display dimensions for EGL clients.
    g_host_surface_width.store(width, std::memory_order_release);
    g_host_surface_height.store(height, std::memory_order_release);
    return;
  }
  if (darwin_art_surface_configure_display_extent(
          surface, static_cast<uint32_t>(width), static_cast<uint32_t>(height)) !=
      DARWIN_ART_SURFACE_OK) {
    return;
  }
  g_host_surface_width.store(width, std::memory_order_release);
  g_host_surface_height.store(height, std::memory_order_release);
  uint32_t backing_width = 0;
  uint32_t backing_height = 0;
  if (!darwin_art_surface_get_size(surface, &backing_width, &backing_height) ||
      backing_width == 0 || backing_height == 0) {
    return;
  }
  // SurfaceFlinger consumes Android logical display coordinates and projects
  // them into this physical IOSurface. Scanout then copies that completed
  // physical target pixel-for-pixel into CAMetalLayer; it must not reuse the
  // logical extent as a source or destination rectangle.
  darwin_art_surface_gpu_configure_embedded(surface, 0, 0, backing_width,
                                            backing_height);
  darwin_art_surface_gpu_set_embedded_buffer_extent(surface, backing_width,
                                                    backing_height);
  darwin_art_surface_gpu_publish_embedded(surface);
  const uint32_t surface_id =
      darwin_art_surface_gpu_iosurface_id(surface);
  if (surface_id != 0) {
    const std::string encoded = std::to_string(surface_id);
    setenv("DARWIN_ART_HOST_IOSURFACE_ID", encoded.c_str(), 1);
  }
  if (std::getenv("DARWIN_ART_DEBUG_RESIZE") != nullptr) {
    std::cerr << "ART Android EGL: display logical=" << width << "x"
              << height << " backing=" << backing_width << "x"
              << backing_height << "\n";
  }
}

jint DarwinAngleHostSurfaceWidth() {
  const jint configured =
      g_host_surface_width.load(std::memory_order_acquire);
  if (configured > 0) return configured;
  DarwinArtSurface* surface = darwin_art_surface_active_gpu();
  uint32_t width = 0;
  uint32_t height = 0;
  return surface != nullptr &&
          darwin_art_surface_get_logical_size(surface, &width, &height) &&
          width <= static_cast<uint32_t>(INT32_MAX)
      ? static_cast<jint>(width)
      : 0;
}

jint DarwinAngleHostSurfaceHeight() {
  const jint configured =
      g_host_surface_height.load(std::memory_order_acquire);
  if (configured > 0) return configured;
  DarwinArtSurface* surface = darwin_art_surface_active_gpu();
  uint32_t width = 0;
  uint32_t height = 0;
  return surface != nullptr &&
          darwin_art_surface_get_logical_size(surface, &width, &height) &&
          height <= static_cast<uint32_t>(INT32_MAX)
      ? static_cast<jint>(height)
      : 0;
}

bool RegisterDarwinAngleEglNatives(JNIEnv* env) {
  JNINativeMethod egl_methods[] = {
      {const_cast<char*>("_nativeClassInit"), const_cast<char*>("()V"),
       reinterpret_cast<void*>(&EglNativeClassInit)},
      {const_cast<char*>("_eglCreateContext"),
       const_cast<char*>("(Ljavax/microedition/khronos/egl/EGLDisplay;Ljavax/microedition/khronos/egl/EGLConfig;Ljavax/microedition/khronos/egl/EGLContext;[I)J"),
       reinterpret_cast<void*>(&EglCreateContext)},
      {const_cast<char*>("_eglCreatePbufferSurface"),
       const_cast<char*>("(Ljavax/microedition/khronos/egl/EGLDisplay;Ljavax/microedition/khronos/egl/EGLConfig;[I)J"),
       reinterpret_cast<void*>(&EglCreatePbufferSurface)},
      {const_cast<char*>("_eglCreatePixmapSurface"),
       const_cast<char*>("(Ljavax/microedition/khronos/egl/EGLSurface;Ljavax/microedition/khronos/egl/EGLDisplay;Ljavax/microedition/khronos/egl/EGLConfig;Ljava/lang/Object;[I)V"),
       reinterpret_cast<void*>(&EglCreatePixmapSurface)},
      {const_cast<char*>("_eglCreateWindowSurface"),
       const_cast<char*>("(Ljavax/microedition/khronos/egl/EGLDisplay;Ljavax/microedition/khronos/egl/EGLConfig;Ljava/lang/Object;[I)J"),
       reinterpret_cast<void*>(&EglCreateWindowSurface)},
      {const_cast<char*>("_eglCreateWindowSurfaceTexture"),
       const_cast<char*>("(Ljavax/microedition/khronos/egl/EGLDisplay;Ljavax/microedition/khronos/egl/EGLConfig;Ljava/lang/Object;[I)J"),
       reinterpret_cast<void*>(&EglCreateWindowSurface)},
      {const_cast<char*>("_eglGetCurrentContext"), const_cast<char*>("()J"),
       reinterpret_cast<void*>(&EglGetCurrentContext)},
      {const_cast<char*>("_eglGetCurrentDisplay"), const_cast<char*>("()J"),
       reinterpret_cast<void*>(&EglGetCurrentDisplay)},
      {const_cast<char*>("_eglGetCurrentSurface"), const_cast<char*>("(I)J"),
       reinterpret_cast<void*>(&EglGetCurrentSurface)},
      {const_cast<char*>("_eglGetDisplay"),
       const_cast<char*>("(Ljava/lang/Object;)J"),
       reinterpret_cast<void*>(&EglGetDisplay)},
      {const_cast<char*>("getInitCount"),
       const_cast<char*>("(Ljavax/microedition/khronos/egl/EGLDisplay;)I"),
       reinterpret_cast<void*>(&EglGetInitCount)},
      {const_cast<char*>("eglChooseConfig"),
       const_cast<char*>("(Ljavax/microedition/khronos/egl/EGLDisplay;[I[Ljavax/microedition/khronos/egl/EGLConfig;I[I)Z"),
       reinterpret_cast<void*>(&EglChooseConfig)},
      {const_cast<char*>("eglCopyBuffers"),
       const_cast<char*>("(Ljavax/microedition/khronos/egl/EGLDisplay;Ljavax/microedition/khronos/egl/EGLSurface;Ljava/lang/Object;)Z"),
       reinterpret_cast<void*>(&EglCopyBuffers)},
      {const_cast<char*>("eglDestroyContext"),
       const_cast<char*>("(Ljavax/microedition/khronos/egl/EGLDisplay;Ljavax/microedition/khronos/egl/EGLContext;)Z"),
       reinterpret_cast<void*>(&EglDestroyContext)},
      {const_cast<char*>("eglDestroySurface"),
       const_cast<char*>("(Ljavax/microedition/khronos/egl/EGLDisplay;Ljavax/microedition/khronos/egl/EGLSurface;)Z"),
       reinterpret_cast<void*>(&EglDestroySurface)},
      {const_cast<char*>("eglGetConfigAttrib"),
       const_cast<char*>("(Ljavax/microedition/khronos/egl/EGLDisplay;Ljavax/microedition/khronos/egl/EGLConfig;I[I)Z"),
       reinterpret_cast<void*>(&EglGetConfigAttrib)},
      {const_cast<char*>("eglGetConfigs"),
       const_cast<char*>("(Ljavax/microedition/khronos/egl/EGLDisplay;[Ljavax/microedition/khronos/egl/EGLConfig;I[I)Z"),
       reinterpret_cast<void*>(&EglGetConfigs)},
      {const_cast<char*>("eglGetError"), const_cast<char*>("()I"),
       reinterpret_cast<void*>(&EglGetError)},
      {const_cast<char*>("eglInitialize"),
       const_cast<char*>("(Ljavax/microedition/khronos/egl/EGLDisplay;[I)Z"),
       reinterpret_cast<void*>(&EglInitialize)},
      {const_cast<char*>("eglMakeCurrent"),
       const_cast<char*>("(Ljavax/microedition/khronos/egl/EGLDisplay;Ljavax/microedition/khronos/egl/EGLSurface;Ljavax/microedition/khronos/egl/EGLSurface;Ljavax/microedition/khronos/egl/EGLContext;)Z"),
       reinterpret_cast<void*>(&EglMakeCurrent)},
      {const_cast<char*>("eglQueryContext"),
       const_cast<char*>("(Ljavax/microedition/khronos/egl/EGLDisplay;Ljavax/microedition/khronos/egl/EGLContext;I[I)Z"),
       reinterpret_cast<void*>(&EglQueryContext)},
      {const_cast<char*>("eglQueryString"),
       const_cast<char*>("(Ljavax/microedition/khronos/egl/EGLDisplay;I)Ljava/lang/String;"),
       reinterpret_cast<void*>(&EglQueryString)},
      {const_cast<char*>("eglQuerySurface"),
       const_cast<char*>("(Ljavax/microedition/khronos/egl/EGLDisplay;Ljavax/microedition/khronos/egl/EGLSurface;I[I)Z"),
       reinterpret_cast<void*>(&EglQuerySurface)},
      {const_cast<char*>("eglReleaseThread"), const_cast<char*>("()Z"),
       reinterpret_cast<void*>(&EglReleaseThread)},
      {const_cast<char*>("eglSwapBuffers"),
       const_cast<char*>("(Ljavax/microedition/khronos/egl/EGLDisplay;Ljavax/microedition/khronos/egl/EGLSurface;)Z"),
       reinterpret_cast<void*>(&EglSwapBuffers)},
      {const_cast<char*>("eglTerminate"),
       const_cast<char*>("(Ljavax/microedition/khronos/egl/EGLDisplay;)Z"),
       reinterpret_cast<void*>(&EglTerminate)},
      {const_cast<char*>("eglWaitGL"), const_cast<char*>("()Z"),
       reinterpret_cast<void*>(&EglWaitGl)},
      {const_cast<char*>("eglWaitNative"),
       const_cast<char*>("(ILjava/lang/Object;)Z"),
       reinterpret_cast<void*>(&EglWaitNative)},
  };
  if (!Register(env, "com/google/android/gles_jni/EGLImpl", egl_methods,
                static_cast<jint>(std::size(egl_methods)))) {
    return false;
  }
  JNINativeMethod gl_impl_methods[] = {
      {const_cast<char*>("_nativeClassInit"), const_cast<char*>("()V"),
       reinterpret_cast<void*>(&GlNativeClassInit)},
  };
  if (!Register(env, "com/google/android/gles_jni/GLImpl", gl_impl_methods,
                static_cast<jint>(std::size(gl_impl_methods)))) {
    return false;
  }
  JNINativeMethod gles20_methods[] = {
      {const_cast<char*>("_nativeClassInit"), const_cast<char*>("()V"),
       reinterpret_cast<void*>(&GlNativeClassInit)},
      {const_cast<char*>("glPixelStorei"), const_cast<char*>("(II)V"),
       reinterpret_cast<void*>(&GlesPixelStoreI)},
      {const_cast<char*>("glDisable"), const_cast<char*>("(I)V"),
       reinterpret_cast<void*>(&GlesDisable)},
      {const_cast<char*>("glDepthMask"), const_cast<char*>("(Z)V"),
       reinterpret_cast<void*>(&GlesDepthMask)},
      {const_cast<char*>("glClearColor"), const_cast<char*>("(FFFF)V"),
       reinterpret_cast<void*>(&GlesClearColor)},
      {const_cast<char*>("glClear"), const_cast<char*>("(I)V"),
       reinterpret_cast<void*>(&GlesClear)},
      {const_cast<char*>("glViewport"), const_cast<char*>("(IIII)V"),
       reinterpret_cast<void*>(&GlesViewport)},
      {const_cast<char*>("glIsTexture"), const_cast<char*>("(I)Z"),
       reinterpret_cast<void*>(&GlesIsTexture)},
      {const_cast<char*>("glGenTextures"), const_cast<char*>("(I[II)V"),
       reinterpret_cast<void*>(&GlesGenTextures)},
      {const_cast<char*>("glActiveTexture"), const_cast<char*>("(I)V"),
       reinterpret_cast<void*>(&GlesActiveTexture)},
      {const_cast<char*>("glBindTexture"), const_cast<char*>("(II)V"),
       reinterpret_cast<void*>(&GlesBindTexture)},
      {const_cast<char*>("glTexParameteri"), const_cast<char*>("(III)V"),
       reinterpret_cast<void*>(&GlesTexParameterI)},
      {const_cast<char*>("glGetError"), const_cast<char*>("()I"),
       reinterpret_cast<void*>(&GlesGetError)},
      {const_cast<char*>("glDeleteTextures"), const_cast<char*>("(I[II)V"),
       reinterpret_cast<void*>(&GlesDeleteTextures)},
  };
  if (!Register(env, "android/opengl/GLES20", gles20_methods,
                static_cast<jint>(std::size(gles20_methods)))) {
    return false;
  }
  JNINativeMethod gl_utils_methods[] = {
      {const_cast<char*>("native_getInternalFormat"),
       const_cast<char*>("(Landroid/graphics/Bitmap;)I"),
       reinterpret_cast<void*>(&GlUtilsGetInternalFormat)},
      {const_cast<char*>("native_getType"),
       const_cast<char*>("(Landroid/graphics/Bitmap;)I"),
       reinterpret_cast<void*>(&GlUtilsGetType)},
      {const_cast<char*>("native_texImage2D"),
       const_cast<char*>("(IIILandroid/graphics/Bitmap;II)I"),
       reinterpret_cast<void*>(&GlUtilsTexImage2D)},
      {const_cast<char*>("native_texSubImage2D"),
       const_cast<char*>("(IIIILandroid/graphics/Bitmap;II)I"),
       reinterpret_cast<void*>(&GlUtilsTexSubImage2D)},
  };
  return Register(env, "android/opengl/GLUtils", gl_utils_methods,
                  static_cast<jint>(std::size(gl_utils_methods)));
}

template <typename Attribute>
std::vector<Attribute> MetalPlatformAttributes(const Attribute* attributes) {
  std::vector<Attribute> translated;
  if (attributes == nullptr) return translated;
  for (std::size_t index = 0; index < 128; index += 2) {
    const Attribute name = attributes[index];
    translated.push_back(name);
    if (name == static_cast<Attribute>(kEglNone)) break;
    Attribute value = attributes[index + 1];
    if (name == static_cast<Attribute>(kEglPlatformAngleType) &&
        value == static_cast<Attribute>(kEglPlatformAngleTypeOpenGles)) {
      value = static_cast<Attribute>(kEglPlatformAngleTypeMetal);
    }
    translated.push_back(value);
  }
  return translated;
}

EGLDisplay EglGetPlatformDisplayExtMetal(EGLenum platform, void* display,
                                         const EGLint* attributes) {
  auto& api = GetAngleApi();
  using Function = EGLDisplay (*)(EGLenum, void*, const EGLint*);
  auto function = reinterpret_cast<Function>(
      api.get_proc_address("eglGetPlatformDisplayEXT"));
  if (function == nullptr) return nullptr;
  if (platform != kEglPlatformAngle) {
    return function(platform, display, attributes);
  }
  const auto translated = MetalPlatformAttributes(attributes);
  if (std::getenv("DARWIN_ART_DEBUG_GRAPHICS_DSO") != nullptr) {
    std::cerr << "ART Android EGL: translated ANGLE OpenGL ES platform to "
                 "Metal\n";
  }
  return function(platform, display,
                  translated.empty() ? attributes : translated.data());
}

EGLDisplay EglGetPlatformDisplayMetal(EGLenum platform, void* display,
                                      const EGLAttrib* attributes) {
  auto& api = GetAngleApi();
  using Function = EGLDisplay (*)(EGLenum, void*, const EGLAttrib*);
  auto function = reinterpret_cast<Function>(
      api.get_proc_address("eglGetPlatformDisplay"));
  if (function == nullptr) return nullptr;
  if (platform != kEglPlatformAngle) {
    return function(platform, display, attributes);
  }
  const auto translated = MetalPlatformAttributes(attributes);
  if (std::getenv("DARWIN_ART_DEBUG_GRAPHICS_DSO") != nullptr) {
    std::cerr << "ART Android EGL: translated ANGLE OpenGL ES platform to "
                 "Metal\n";
  }
  return function(platform, display,
                  translated.empty() ? attributes : translated.data());
}

bool DebugGraphicsDso() {
  return std::getenv("DARWIN_ART_DEBUG_GRAPHICS_DSO") != nullptr;
}

EGLDisplay EglGetDisplayHost(void* display) {
  auto& api = GetAngleApi();
  EGLDisplay result = api.get_display(display);
  if (DebugGraphicsDso()) {
    std::cerr << "ART Android EGL: eglGetDisplay result=" << result << "\n";
  }
  return result;
}

EGLBoolean EglInitializeHost(EGLDisplay display, EGLint* major,
                             EGLint* minor) {
  EGLint initialized_major = 0;
  EGLint initialized_minor = 0;
  EGLBoolean result = WindowBackend().Initialize(
      display, &initialized_major, &initialized_minor) ? 1u : 0u;
  if (result != 0) {
    if (major != nullptr) *major = initialized_major;
    if (minor != nullptr) *minor = initialized_minor;
  }
  auto& api = GetAngleApi();
  if (DebugGraphicsDso()) {
    std::cerr << "ART Android EGL: eglInitialize result=" << result
              << " version=" << (major == nullptr ? -1 : *major) << "."
              << (minor == nullptr ? -1 : *minor) << " host_extensions="
              << (result == 0 ? "<unavailable>"
                              : api.query_string(display, kEglExtensions))
              << "\n";
  }
  return result;
}

EGLBoolean EglChooseConfigHost(EGLDisplay display, const EGLint* attributes,
                               EGLConfig* configs, EGLint config_size,
                               EGLint* count) {
  auto& api = GetAngleApi();
  EGLBoolean result =
      api.choose_config(display, attributes, configs, config_size, count);
  if (DebugGraphicsDso()) {
    std::cerr << "ART Android EGL: eglChooseConfig result=" << result
              << " count=" << (count == nullptr ? -1 : *count) << "\n";
  }
  return result;
}

EGLContext EglCreateContextHost(EGLDisplay display, EGLConfig config,
                                EGLContext share, const EGLint* attributes) {
  auto& api = GetAngleApi();
  EGLContext result = darwin_art::graphics::DispatchCreateContext(
      display, config, share, attributes, api.create_context,
      DebugGraphicsDso());
  if (result == nullptr && api.get_error != nullptr)
    darwin_art::graphics::SetEglError(api.get_error());
  return result;
}

EGLSurface EglCreatePbufferSurfaceHost(EGLDisplay display, EGLConfig config,
                                       const EGLint* attributes) {
  auto& api = GetAngleApi();
  EGLSurface result = api.create_pbuffer_surface(display, config, attributes);
  if (DebugGraphicsDso()) {
    std::cerr << "ART Android EGL: eglCreatePbufferSurface result=" << result
              << "\n";
  }
  return result;
}

EGLBoolean EglMakeCurrentHost(EGLDisplay display, EGLSurface draw,
                              EGLSurface read, EGLContext context) {
  auto& api = GetAngleApi();
  return darwin_art::graphics::DispatchMakeCurrent(
      display, draw, read, context, api.make_current, DebugGraphicsDso());
}

const std::uint8_t* GlGetStringHost(std::uint32_t name) {
  using Function = const std::uint8_t* (*)(std::uint32_t);
  auto function = reinterpret_cast<Function>(
      GetAngleApi().get_proc_address("glGetString"));
  const std::uint8_t* result = function == nullptr ? nullptr : function(name);
  if (name == kGlExtensions && result != nullptr &&
      std::strstr(reinterpret_cast<const char*>(result),
                  "GL_OES_EGL_image") == nullptr) {
    static std::string extensions;
    extensions = reinterpret_cast<const char*>(result);
    extensions += " GL_OES_EGL_image";
    result = reinterpret_cast<const std::uint8_t*>(extensions.c_str());
  }
  if (DebugGraphicsDso()) {
    std::cerr << "ART Android EGL: glGetString name=0x" << std::hex << name
              << std::dec << " result="
              << (result == nullptr
                      ? "<null>"
                      : reinterpret_cast<const char*>(result))
              << "\n";
  }
  return result;
}

const std::uint8_t* GlGetStringiHost(std::uint32_t name,
                                    std::uint32_t index) {
  if (name == kGlExtensions) {
    std::int32_t host_count = 0;
    GetAngleApi().gl_get_integer_v(kGlNumExtensions, &host_count);
    if (index == static_cast<std::uint32_t>(std::max(0, host_count))) {
      return reinterpret_cast<const std::uint8_t*>("GL_OES_EGL_image");
    }
  }
  using Function = const std::uint8_t* (*)(std::uint32_t, std::uint32_t);
  auto function = reinterpret_cast<Function>(
      GetAngleApi().get_proc_address("glGetStringi"));
  const std::uint8_t* result =
      function == nullptr ? nullptr : function(name, index);
  if (DebugGraphicsDso() &&
      (result == nullptr || name == 0x1F03 /* GL_EXTENSIONS */)) {
    std::cerr << "ART Android EGL: glGetStringi name=0x" << std::hex << name
              << std::dec << " index=" << index << " result="
              << (result == nullptr
                      ? "<null>"
                      : reinterpret_cast<const char*>(result))
              << "\n";
  }
  return result;
}

void GlGetIntegervHost(std::uint32_t name, std::int32_t* value) {
  auto& api = GetAngleApi();
  api.gl_get_integer_v(name, value);
  if (value != nullptr &&
      (name == 0x8069 ||  // GL_TEXTURE_BINDING_2D
       name == 0x8C1D ||  // GL_TEXTURE_BINDING_2D_ARRAY
       name == 0x8D67)) { // GL_TEXTURE_BINDING_EXTERNAL_OES
    const EGLContext current = api.get_current_context == nullptr
                                   ? nullptr
                                   : api.get_current_context();
    *value = static_cast<std::int32_t>(darwin_art::graphics::
        GuestTextureForHostTexture(current, static_cast<std::uint32_t>(*value)));
  }
  if (name == kGlNumExtensions && value != nullptr) ++*value;
}

const char* EglQueryStringAndroid(EGLDisplay display, EGLint name) {
  auto& api = GetAngleApi();
  const char* result = api.query_string(display, name);
  if (name != kEglExtensions || result == nullptr) return result;
  thread_local std::string extensions;
  extensions.clear();
  // ANGLE's native display supports damage tracking for its own window
  // surfaces. Android producers in this compatibility layer instead render
  // into a rotating set of AHardwareBuffer-backed IOSurfaces. Until the
  // BufferQueue buffer-age contract is implemented, advertising any of the
  // partial-update extensions lets Chromium redraw only the damaged region of
  // a fresh buffer and leaves its preserved region black. Hide those
  // capabilities so clients submit complete frames, which is the EGL-defined
  // fallback when buffer age is unavailable.
  constexpr std::array<std::string_view, 3> unsupported{
      "EGL_EXT_buffer_age",
      "EGL_KHR_partial_update",
      // The Metal IOSurface path exposes only RGBA_8888; advertising the
      // float pixel-format extension makes HWUI probe an unavailable FP16
      // config and emit a misleading wide-gamut error on every surface.
      "EGL_EXT_pixel_format_float",
  };
  std::string_view remaining(result);
  while (!remaining.empty()) {
    const std::size_t first = remaining.find_first_not_of(' ');
    if (first == std::string_view::npos) break;
    remaining.remove_prefix(first);
    const std::size_t separator = remaining.find(' ');
    const std::string_view extension = remaining.substr(0, separator);
    if (std::find(unsupported.begin(), unsupported.end(), extension) ==
        unsupported.end()) {
      if (!extensions.empty()) extensions.push_back(' ');
      extensions.append(extension);
    }
    if (separator == std::string_view::npos) break;
    remaining.remove_prefix(separator + 1);
  }
  if (extensions.find("EGL_KHR_image_base") == std::string::npos)
    extensions += " EGL_KHR_image_base";
  if (extensions.find("EGL_ANDROID_image_native_buffer") == std::string::npos)
    extensions += " EGL_ANDROID_image_native_buffer";
  // Chromium's Android shared-image path probes this companion extension
  // before importing AHardwareBuffer-backed images.  The bridge exports the
  // matching eglGetNativeClientBufferANDROID entry point and maps the client
  // buffer back to the IOSurface owner in EglCreateImageAndroid.
  if (extensions.find("EGL_ANDROID_get_native_client_buffer") ==
      std::string::npos) {
    extensions += " EGL_ANDROID_get_native_client_buffer";
  }
  if (extensions.find("EGL_KHR_fence_sync") == std::string::npos)
    extensions += " EGL_KHR_fence_sync";
  if (extensions.find("EGL_KHR_wait_sync") == std::string::npos)
    extensions += " EGL_KHR_wait_sync";
  if (extensions.find("EGL_ANDROID_native_fence_sync") == std::string::npos)
    extensions += " EGL_ANDROID_native_fence_sync";
  // The Darwin Android window adapter keeps a persistent ANGLE render
  // pbuffer and copies its complete contents into each dequeued IOSurface.
  // Damage rectangles are therefore accepted as scheduling metadata without
  // relying on undefined contents in a fresh BufferQueue slot.
  if (extensions.find("EGL_KHR_swap_buffers_with_damage") ==
      std::string::npos) {
    extensions += " EGL_KHR_swap_buffers_with_damage";
  }
  if (extensions.find("EGL_EXT_swap_buffers_with_damage") ==
      std::string::npos) {
    extensions += " EGL_EXT_swap_buffers_with_damage";
  }
  return extensions.c_str();
}

void SynchronizeIosurfaceToAhbFence() {
  darwin_art::graphics::SynchronizeIosurfaceToAhbClientTextures(
      AhbImageBackend(), DebugGraphicsDso());
}

void SynchronizeAhbToIosurfaceFence() {
  darwin_art::graphics::SynchronizeAhbImagesToIosurface(AhbImageBackend(),
                                                        DebugGraphicsDso());
}

darwin_art::graphics::EglNativeFenceBackend NativeFenceBackend() {
  auto& api = GetAngleApi();
  darwin_art::graphics::EglNativeFenceBackend backend;
  backend.create_sync_khr = LoadSymbol<decltype(backend.create_sync_khr)>(
      api.egl_library, "eglCreateSyncKHR");
  backend.destroy_sync = LoadSymbol<decltype(backend.destroy_sync)>(
      api.egl_library, "eglDestroySync");
  backend.destroy_sync_khr = LoadSymbol<decltype(backend.destroy_sync_khr)>(
      api.egl_library, "eglDestroySyncKHR");
  backend.client_wait_sync = LoadSymbol<decltype(backend.client_wait_sync)>(
      api.egl_library, "eglClientWaitSync");
  backend.client_wait_sync_khr =
      LoadSymbol<decltype(backend.client_wait_sync_khr)>(
          api.egl_library, "eglClientWaitSyncKHR");
  backend.wait_sync = LoadSymbol<decltype(backend.wait_sync)>(
      api.egl_library, "eglWaitSync");
  backend.wait_sync_khr = LoadSymbol<decltype(backend.wait_sync_khr)>(
      api.egl_library, "eglWaitSyncKHR");
  backend.get_sync_attrib = reinterpret_cast<decltype(backend.get_sync_attrib)>(
      api.get_proc_address("eglGetSyncAttrib"));
  backend.get_sync_attrib_khr =
      LoadSymbol<decltype(backend.get_sync_attrib_khr)>(
          api.egl_library, "eglGetSyncAttribKHR");
  backend.dup_native_fence_fd =
      LoadSymbol<decltype(backend.dup_native_fence_fd)>(
          api.egl_library, "eglDupNativeFenceFDANDROID");
  backend.dup_native_fence_fd_khr =
      LoadSymbol<decltype(backend.dup_native_fence_fd_khr)>(
          api.egl_library, "eglDupNativeFenceFDANDROID");
  backend.create_sync = reinterpret_cast<decltype(backend.create_sync)>(
      api.get_proc_address("eglCreateSync"));
  backend.query_display_attrib =
      reinterpret_cast<decltype(backend.query_display_attrib)>(
          api.get_proc_address("eglQueryDisplayAttribEXT"));
  backend.query_device_attrib =
      reinterpret_cast<decltype(backend.query_device_attrib)>(
          api.get_proc_address("eglQueryDeviceAttribEXT"));
  backend.metal_shared_event_create =
      darwin_art_android_metal_shared_event_create;
  backend.metal_shared_event_fence_fd =
      darwin_art_android_metal_shared_event_fence_fd;
  backend.metal_shared_event_release =
      darwin_art_android_metal_shared_event_release;
  backend.sync_wait = sync_wait;
  backend.close_fence_fd = darwin_art_bionic_socket_broker_close;
  backend.synchronize_iosurface_to_ahb = SynchronizeIosurfaceToAhbFence;
  backend.synchronize_ahb_to_iosurface = SynchronizeAhbToIosurfaceFence;
  backend.debug = DebugGraphicsDso();
  return backend;
}

EGLSync EglCreateSyncKhrAndroid(EGLDisplay display, EGLenum type,
                                const EGLint* attributes) {
  const auto backend = NativeFenceBackend();
  return darwin_art::graphics::CreateNativeFenceSync(
      display, type, attributes, backend);
}

EGLBoolean EglDestroySyncKhrAndroid(EGLDisplay display, EGLSync sync) {
  const auto backend = NativeFenceBackend();
  return darwin_art::graphics::DestroyNativeFenceSync(display, sync, backend);
}

EGLint EglClientWaitSyncKhrAndroid(EGLDisplay display, EGLSync sync,
                                   EGLint flags, std::uint64_t timeout) {
  const auto backend = NativeFenceBackend();
  return darwin_art::graphics::ClientWaitNativeFenceSync(
      display, sync, flags, timeout, backend);
}

EGLBoolean EglWaitSyncKhrAndroid(EGLDisplay display, EGLSync sync,
                                 EGLint flags) {
  const auto backend = NativeFenceBackend();
  return darwin_art::graphics::WaitNativeFenceSync(display, sync, flags,
                                                   backend);
}

EGLBoolean EglGetSyncAttribKhrAndroid(EGLDisplay display, EGLSync sync,
                                      EGLint attribute, EGLint* value) {
  const auto backend = NativeFenceBackend();
  return darwin_art::graphics::GetNativeFenceSyncAttrib(
      display, sync, attribute, value, backend);
}

EGLSync EglCreateSyncAndroid(EGLDisplay display, EGLenum type,
                             const EGLAttrib* attributes) {
  if (attributes != nullptr &&
      attributes[0] == kEglSyncNativeFenceFdAndroid) {
    const EGLint narrow[] = {kEglSyncNativeFenceFdAndroid,
                             static_cast<EGLint>(attributes[1]), kEglNone};
    return EglCreateSyncKhrAndroid(display, type, narrow);
  }
  return EglCreateSyncKhrAndroid(display, type, nullptr);
}

EGLBoolean EglGetSyncAttribAndroid(EGLDisplay display, EGLSync sync,
                                   EGLint attribute, EGLAttrib* value) {
  EGLint narrow = 0;
  const EGLBoolean result =
      EglGetSyncAttribKhrAndroid(display, sync, attribute, &narrow);
  if (result != 0 && value != nullptr) *value = narrow;
  return value == nullptr ? 0 : result;
}

EGLint EglDupNativeFenceFdAndroid(EGLDisplay display, EGLSync sync) {
  const auto backend = NativeFenceBackend();
  return darwin_art::graphics::DupNativeFenceFd(display, sync, backend);
}

darwin_art::graphics::CompositionProducerFence ExportCompositionProducerFence(
    void* display) {
  const auto backend = NativeFenceBackend();
  const auto exported = darwin_art::graphics::ExportNativeFence(display, backend);
  return {.token = exported.token,
          .shared_event = exported.shared_event,
          .signal_value = exported.signal_value};
}

void ReleaseCompositionProducerFence(void* display, void* token) {
  const auto backend = NativeFenceBackend();
  darwin_art::graphics::ReleaseNativeFence(display, token, backend);
}

bool TrackCompositionFence(int fence) {
  DarwinArtSurface* surface = darwin_art_surface_active_gpu();
  if (surface == nullptr || fence < 0) return false;
  const int monitor_fence = darwin_art_bionic_socket_broker_dup(fence);
  if (monitor_fence < 0) return false;
  // The monitor consumes its copy even on rejection. The original remains
  // owned by the transaction returned from EndComposition.
  return darwin_art_surface_gpu_track_composition_fence(surface, monitor_fence);
}

darwin_art::graphics::CompositionConsumerBackend CompositionBackend() {
  auto& api = GetAngleApi();
  darwin_art::graphics::CompositionConsumerBackend backend;
  backend.get_current_display = api.get_current_display;
  backend.get_current_context = api.get_current_context;
  backend.get_current_surface = api.get_current_surface;
  backend.make_current = api.make_current;
  backend.get_proc_address = api.get_proc_address;
  backend.get_error = api.get_error;
  backend.gl_is_enabled = api.gl_is_enabled;
  backend.gl_enable = api.gl_enable;
  backend.gl_get_integer_v = api.gl_get_integer_v;
  backend.gl_bind_framebuffer = api.gl_bind_framebuffer;
  backend.gl_disable = api.gl_disable;
  backend.gl_clear_color = api.gl_clear_color;
  backend.gl_clear = api.gl_clear;
  backend.lookup_iosurface = darwin_art_surface_gpu_lookup_iosurface;
  backend.release_iosurface = darwin_art_surface_gpu_release_iosurface;
  backend.set_composition_active =
      darwin_art_surface_gpu_set_iosurface_composition_active;
  backend.host_surface_width = DarwinAngleHostSurfaceWidth;
  backend.host_surface_height = DarwinAngleHostSurfaceHeight;
  backend.export_producer_fence = ExportCompositionProducerFence;
  backend.release_producer_fence = ReleaseCompositionProducerFence;
  backend.close_completion_fence = [](int fd) {
    (void)darwin_art_bionic_socket_broker_close(fd);
  };
  backend.present_remote = darwin_art_surfaceflinger_service_present;
  backend.present_remote_receipt = darwin_art_surfaceflinger_service_present_receipt;
  backend.compose_local = darwin_art_metal_composer_compose;
  backend.completion_fence_fd = darwin_art_android_metal_shared_event_fence_fd;
  backend.release_shared_event = darwin_art_android_metal_shared_event_release;
  backend.track_completion_fence = TrackCompositionFence;
  backend.lease_ops = {
      .retain = AHardwareBuffer_acquire,
      .release = AHardwareBuffer_release,
      .describe = AHardwareBuffer_describe,
      .iosurface = darwin_art_android_hardware_buffer_iosurface,
  };
  backend.debug = DebugGraphicsDso();
  return backend;
}

extern "C" bool darwin_art_android_begin_hardware_buffer_composition(
    void* opaque, bool clear, std::uint64_t transaction_id) {
  return darwin_art::graphics::BeginComposition(
      CompositionBackend(), opaque, clear, transaction_id);
}

extern "C" int darwin_art_android_end_hardware_buffer_composition() {
  return darwin_art::graphics::EndComposition(CompositionBackend());
}
extern "C" bool darwin_art_android_begin_hardware_buffer_composition_checked(
    void* buffer, bool clear, uint64_t transaction_id, bool* started) {
  if (started == nullptr) return false;
  const auto result = darwin_art::graphics::BeginCompositionResult(
      CompositionBackend(), buffer, clear, transaction_id);
  *started = result.started;
  return result.retry_safe;
}
extern "C" bool darwin_art_android_end_hardware_buffer_composition_checked(
    int* present_fence) {
  if (present_fence == nullptr) return false;
  const auto result = darwin_art::graphics::EndCompositionResult(CompositionBackend());
  if (!result.context_restored && result.present_fence >= 0) {
    (void)darwin_art_bionic_socket_broker_close(result.present_fence);
    *present_fence = -1;
    return false;
  }
  *present_fence = result.present_fence;
  return result.context_restored;
}
extern "C" bool darwin_art_android_end_hardware_buffer_composition_receipt(
    DarwinArtSurfaceFlingerReceipt* receipt) {
  if (receipt == nullptr) return false;
  const auto result = darwin_art::graphics::EndCompositionResult(CompositionBackend());
  *receipt = result.receipt;
  return result.context_restored;
}

extern "C" void darwin_art_android_set_hardware_buffer_composition_active(
    bool active) {
  darwin_art::graphics::SetCompositionActive(CompositionBackend(), active);
}

extern "C" void darwin_art_android_present_hardware_buffer(
    void* queue, std::uint32_t owner_process_id, std::uint32_t layer_id,
    std::uint32_t parent_owner_process_id, std::uint32_t parent_id,
    std::uint64_t what, std::uint32_t relative_parent_owner_process_id,
    std::uint32_t relative_parent_id, std::int32_t z, void* opaque,
    std::uint32_t transform, std::int32_t source_left,
    std::int32_t source_top, std::int32_t source_right,
    std::int32_t source_bottom, std::int32_t destination_left,
    std::int32_t destination_top, std::int32_t destination_right,
    std::int32_t destination_bottom, bool has_damage,
    std::int32_t damage_left, std::int32_t damage_top,
    std::int32_t damage_right, std::int32_t damage_bottom, float alpha) {
  darwin_art::graphics::PresentHardwareBuffer(
      CompositionBackend(), queue, owner_process_id, layer_id,
      parent_owner_process_id, parent_id, what,
      relative_parent_owner_process_id, relative_parent_id, z, opaque, transform,
      source_left, source_top, source_right, source_bottom, destination_left,
      destination_top, destination_right, destination_bottom, has_damage,
      damage_left, damage_top, damage_right, damage_bottom, alpha);
}

extern "C" void darwin_art_android_present_surface_control_state(
    std::uint32_t owner_process_id, std::uint32_t layer_id,
    std::uint32_t parent_owner_process_id, std::uint32_t parent_id,
    std::uint32_t relative_parent_owner_process_id,
    std::uint32_t relative_parent_id, std::uint64_t what,
    std::uint32_t flags, std::uint32_t mask, std::uint32_t transform,
    std::int32_t destination_left, std::int32_t destination_top,
    std::int32_t destination_right, std::int32_t destination_bottom,
    std::int32_t position_x, std::int32_t position_y, float scale_x,
    float scale_y, bool has_crop, std::int32_t crop_left,
    std::int32_t crop_top, std::int32_t crop_right, std::int32_t crop_bottom,
    std::int32_t z, float alpha, const std::int32_t* transparent_region_rects,
    std::uint32_t transparent_region_count) {
  darwin_art::graphics::PresentSurfaceControlState(
      owner_process_id, layer_id, parent_owner_process_id, parent_id,
      relative_parent_owner_process_id, relative_parent_id, what, flags, mask,
      transform, destination_left, destination_top, destination_right,
      destination_bottom, position_x, position_y, scale_x, scale_y, has_crop,
      crop_left, crop_top, crop_right, crop_bottom, z, alpha,
      transparent_region_rects, transparent_region_count);
}

extern "C" void darwin_art_android_mark_hardware_buffer_released(
    void* opaque) {
  auto* buffer = static_cast<AHardwareBuffer*>(opaque);
  darwin_art::graphics::MarkHardwareBufferReleased(buffer, DebugGraphicsDso());
}

void GlFramebufferTexture2dAndroid(std::uint32_t target,
                                   std::uint32_t attachment,
                                   std::uint32_t texture_target,
                                   std::uint32_t texture,
                                   std::int32_t level) {
  if ((target == kGlFramebuffer || target == kGlDrawFramebuffer) &&
      attachment == kGlColorAttachment0 && texture_target == kGlTexture2d &&
      level == 0) {
    // Seed the rotating slot before attaching it to Chromium's draw FBO.
    // Reattaching the same texture to a temporary blit FBO after this point
    // can invalidate ANGLE's active Metal render-pass bookkeeping.
    darwin_art::graphics::RestoreBufferQueueSlotIfNeeded(
        texture, AhbImageBackend(), DebugGraphicsDso());
  }
  if (texture_target == kGlTexture2d && texture != 0) {
    auto& api = GetAngleApi();
    const EGLContext current = api.get_current_context == nullptr
                                   ? nullptr
                                   : api.get_current_context();
    texture = darwin_art::graphics::HostTextureForGuestTexture(current, texture);
  }
  GetAngleApi().gl_framebuffer_texture_2d(target, attachment, texture_target,
                                          texture, level);
}

void GlFramebufferTexture2dMultisampleExtAndroid(
    std::uint32_t target, std::uint32_t attachment,
    std::uint32_t texture_target, std::uint32_t texture, std::int32_t level,
    std::int32_t samples) {
  if ((target == kGlFramebuffer || target == kGlDrawFramebuffer) &&
      attachment == kGlColorAttachment0 && texture_target == kGlTexture2d &&
      level == 0) {
    darwin_art::graphics::RestoreBufferQueueSlotIfNeeded(
        texture, AhbImageBackend(), DebugGraphicsDso());
  }
  if (texture_target == kGlTexture2d && texture != 0) {
    auto& api = GetAngleApi();
    const EGLContext current = api.get_current_context == nullptr
                                   ? nullptr
                                   : api.get_current_context();
    texture = darwin_art::graphics::HostTextureForGuestTexture(current, texture);
  }
  using Function = void (*)(std::uint32_t, std::uint32_t, std::uint32_t,
                            std::uint32_t, std::int32_t, std::int32_t);
  auto function = reinterpret_cast<Function>(
      GetAngleApi().get_proc_address("glFramebufferTexture2DMultisampleEXT"));
  if (function != nullptr) {
    function(target, attachment, texture_target, texture, level, samples);
  }
}

void GlBindTextureAndroid(std::uint32_t target, std::uint32_t texture) {
  auto& api = GetAngleApi();
  if (target == kGlTexture2d && texture != 0) {
    const EGLContext current = api.get_current_context == nullptr
                                   ? nullptr
                                   : api.get_current_context();
    texture = darwin_art::graphics::HostTextureForGuestTexture(current, texture);
  }
  api.gl_bind_texture(target, texture);
}

void GlGetFramebufferAttachmentParameterivAndroid(
    std::uint32_t target, std::uint32_t attachment, std::uint32_t pname,
    std::int32_t* params) {
  auto& api = GetAngleApi();
  if (api.gl_get_framebuffer_attachment_parameter_iv == nullptr) return;
  api.gl_get_framebuffer_attachment_parameter_iv(target, attachment, pname,
                                                  params);
  if (params != nullptr && pname == kGlFramebufferAttachmentObjectName) {
    const EGLContext current = api.get_current_context == nullptr
                                   ? nullptr
                                   : api.get_current_context();
    *params = static_cast<std::int32_t>(darwin_art::graphics::
        GuestTextureForHostTexture(current, static_cast<std::uint32_t>(*params)));
  }
}

void GlBindFramebufferAndroid(std::uint32_t target, std::uint32_t framebuffer) {
  auto& api = GetAngleApi();
  api.gl_bind_framebuffer(target, framebuffer);
  if ((target != kGlFramebuffer && target != kGlDrawFramebuffer) ||
      framebuffer == 0 ||
      api.gl_get_framebuffer_attachment_parameter_iv == nullptr) {
    return;
  }
  // Chromium attaches each BufferQueue slot once, then rotates by rebinding
  // the already-complete FBO. Treat that bind as the acquire boundary too;
  // waiting only for glFramebufferTexture2D preserves the first use but lets
  // every later partial update accumulate over stale/black slot contents.
  std::int32_t attachment_type = 0;
  std::int32_t attachment_name = 0;
  api.gl_get_framebuffer_attachment_parameter_iv(
      target, kGlColorAttachment0, kGlFramebufferAttachmentObjectType,
      &attachment_type);
  api.gl_get_framebuffer_attachment_parameter_iv(
      target, kGlColorAttachment0, kGlFramebufferAttachmentObjectName,
      &attachment_name);
  if (attachment_type == kGlFramebufferAttachmentTexture &&
      attachment_name != 0) {
    const EGLContext current = api.get_current_context == nullptr
                                   ? nullptr
                                   : api.get_current_context();
    darwin_art::graphics::RestoreBufferQueueSlotIfNeeded(
        darwin_art::graphics::GuestTextureForHostTexture(
            current, static_cast<std::uint32_t>(attachment_name)),
        AhbImageBackend(), DebugGraphicsDso());
  }
}

void GlDeleteTexturesAndroid(std::int32_t count,
                             const std::uint32_t* textures) {
  if (count <= 0 || textures == nullptr) return;
  auto& api = GetAngleApi();
  const EGLContext current = api.get_current_context == nullptr
                                 ? nullptr
                                 : api.get_current_context();
  darwin_art::graphics::DetachDeletedTextures(
      current, count, textures, DebugGraphicsDso());
  api.gl_delete_textures(count, textures);
}

void GlTexImage2dAndroid(std::uint32_t target, std::int32_t level,
                         std::int32_t internal_format, std::int32_t width,
                         std::int32_t height, std::int32_t border,
                         std::uint32_t format, std::uint32_t type,
                         const void* pixels) {
  if (level == 0) {
    darwin_art::graphics::DetachBoundTextureForStorage(
        target, "glTexImage2D", width, height, AhbImageBackend(),
        DebugGraphicsDso());
  }
  GetAngleApi().gl_tex_image_2d(target, level, internal_format, width, height,
                                border, format, type, pixels);
}

void GlTexStorage2dAndroid(std::uint32_t target, std::int32_t levels,
                           std::uint32_t internal_format,
                           std::int32_t width, std::int32_t height) {
  darwin_art::graphics::DetachBoundTextureForStorage(
      target, "glTexStorage2D", width, height, AhbImageBackend(),
      DebugGraphicsDso());
  using Function = void (*)(std::uint32_t, std::int32_t, std::uint32_t,
                            std::int32_t, std::int32_t);
  auto function = reinterpret_cast<Function>(
      GetAngleApi().get_proc_address("glTexStorage2D"));
  if (function != nullptr) function(target, levels, internal_format, width, height);
}

void GlTexStorage2dExtAndroid(std::uint32_t target, std::int32_t levels,
                              std::uint32_t internal_format,
                              std::int32_t width, std::int32_t height) {
  darwin_art::graphics::DetachBoundTextureForStorage(
      target, "glTexStorage2DEXT", width, height, AhbImageBackend(),
      DebugGraphicsDso());
  GetAngleApi().gl_tex_storage_2d_ext(target, levels, internal_format, width,
                                      height);
}

void GlBlitFramebufferAndroid(std::int32_t source_x0,
                              std::int32_t source_y0,
                              std::int32_t source_x1,
                              std::int32_t source_y1,
                              std::int32_t destination_x0,
                              std::int32_t destination_y0,
                              std::int32_t destination_x1,
                              std::int32_t destination_y1,
                              std::uint64_t android_mask_slot,
                              std::uint32_t filter);

void* EglGetNativeClientBufferAndroid(AHardwareBuffer* buffer) {
  if (DebugGraphicsDso()) {
    std::cerr << "ART Android EGL: eglGetNativeClientBufferANDROID buffer="
              << buffer << "\n";
  }
  // bionic's AHardwareBuffer is the owning GraphicBuffer object, while EGL's
  // native client buffer is its embedded ANativeWindowBuffer view.  Android's
  // conversion helper advances by two pointer-sized fields on arm64. Keep
  // that public ABI here; EglCreateImageAndroid resolves the alias back to the
  // owning object before importing its IOSurface.
  return buffer == nullptr ? nullptr
                           : reinterpret_cast<char*>(buffer) + 0x10;
}

EGLImage EglCreateImageAndroid(EGLDisplay display, EGLContext context,
                               EGLenum target, void* client_buffer,
                               const EGLint* attributes) {
  if (target != kEglNativeBufferAndroid) {
    auto& api = GetAngleApi();
    const EGLDisplay current_display =
        api.get_current_display == nullptr ? nullptr : api.get_current_display();
    if (DebugGraphicsDso()) {
      std::cerr << "ART Android EGL: eglCreateImageKHR target=0x" << std::hex
                << target << std::dec << " client=" << client_buffer
                << " display=" << display << " current=" << current_display
                << " image_display="
                << (current_display == nullptr ? display : current_display)
                << "\n";
    }
    using Function = EGLImage (*)(EGLDisplay, EGLContext, EGLenum, void*,
                                  const EGLint*);
    auto function = reinterpret_cast<Function>(
        GetAngleApi().get_proc_address("eglCreateImageKHR"));
    return function == nullptr
               ? nullptr
               : function(display, context, target, client_buffer, attributes);
  }
  return darwin_art::graphics::CreateAndroidImage(
      display, context, target, client_buffer, attributes, AhbImageBackend(),
      DebugGraphicsDso());
}

EGLImage EglCreateImageAndroidCore(EGLDisplay display, EGLContext context,
                                   EGLenum target, void* client_buffer,
                                   const EGLAttrib* attributes) {
  if (DebugGraphicsDso()) {
    std::cerr << "ART Android EGL: eglCreateImage target=0x" << std::hex
              << target << std::dec << " client=" << client_buffer << "\n";
  }
  if (target == kEglNativeBufferAndroid) {
    return EglCreateImageAndroid(display, context, target, client_buffer,
                                 nullptr);
  }
  using Function = EGLImage (*)(EGLDisplay, EGLContext, EGLenum, void*,
                                const EGLAttrib*);
  auto function =
      LoadSymbol<Function>(GetAngleApi().egl_library, "eglCreateImage");
  return function == nullptr
             ? nullptr
             : function(display, context, target, client_buffer, attributes);
}

EGLBoolean EglDestroyImageAndroid(EGLDisplay display, EGLImage image) {
  return darwin_art::graphics::DestroyAndroidImage(
      display, image, AhbImageBackend(), DebugGraphicsDso());
}

EGLBoolean EglDestroyImageAndroidCore(EGLDisplay display, EGLImage image) {
  if (darwin_art::graphics::IsAndroidImage(image)) {
    return EglDestroyImageAndroid(display, image);
  }
  using Function = EGLBoolean (*)(EGLDisplay, EGLImage);
  auto function =
      LoadSymbol<Function>(GetAngleApi().egl_library, "eglDestroyImage");
  return function == nullptr ? 0 : function(display, image);
}

void GlEglImageTargetTexture2dOes(std::uint32_t target, EGLImage image) {
  darwin_art::graphics::BindImageTexture(target, image, AhbImageBackend(),
                                         DebugGraphicsDso());
}

extern "C" void darwin_art_android_egl_bind_direct_image_texture(
    std::uint32_t target, void* image) {
  darwin_art::graphics::BindImageTexture(target, image, AhbImageBackend(),
                                         DebugGraphicsDso(), true);
}

void* EglGetProcAddressMetal(const char* symbol) {
  if (symbol == nullptr) return nullptr;
  if (std::getenv("DARWIN_ART_DEBUG_ANGLE") != nullptr) {
    std::cerr << "ART Android EGL: eglGetProcAddress pid=" << getpid()
              << " symbol=" << symbol << "\n";
  }
  if (std::strcmp(symbol, "eglGetPlatformDisplayEXT") == 0) {
    return reinterpret_cast<void*>(&EglGetPlatformDisplayExtMetal);
  }
  if (std::strcmp(symbol, "eglGetPlatformDisplay") == 0) {
    return reinterpret_cast<void*>(&EglGetPlatformDisplayMetal);
  }
  if (std::strcmp(symbol, "eglQueryString") == 0) {
    return reinterpret_cast<void*>(&EglQueryStringAndroid);
  }
  if (std::strcmp(symbol, "eglBeginFrame") == 0) {
    return reinterpret_cast<void*>(&eglBeginFrame);
  }
  if (std::strcmp(symbol, "eglGetNativeClientBufferANDROID") == 0) {
    return reinterpret_cast<void*>(&EglGetNativeClientBufferAndroid);
  }
  if (std::strcmp(symbol, "eglCreateImageKHR") == 0) {
    return reinterpret_cast<void*>(&EglCreateImageAndroid);
  }
  if (std::strcmp(symbol, "eglCreateImage") == 0) {
    return reinterpret_cast<void*>(&EglCreateImageAndroidCore);
  }
  if (std::strcmp(symbol, "eglDestroyImageKHR") == 0) {
    return reinterpret_cast<void*>(&EglDestroyImageAndroid);
  }
  if (std::strcmp(symbol, "eglDestroyImage") == 0) {
    return reinterpret_cast<void*>(&EglDestroyImageAndroidCore);
  }
  if (std::strcmp(symbol, "eglCreateSyncKHR") == 0) {
    return reinterpret_cast<void*>(&EglCreateSyncKhrAndroid);
  }
  if (std::strcmp(symbol, "eglDestroySyncKHR") == 0) {
    return reinterpret_cast<void*>(&EglDestroySyncKhrAndroid);
  }
  if (std::strcmp(symbol, "eglClientWaitSyncKHR") == 0) {
    return reinterpret_cast<void*>(&EglClientWaitSyncKhrAndroid);
  }
  if (std::strcmp(symbol, "eglWaitSyncKHR") == 0) {
    return reinterpret_cast<void*>(&EglWaitSyncKhrAndroid);
  }
  if (std::strcmp(symbol, "eglGetSyncAttribKHR") == 0) {
    return reinterpret_cast<void*>(&EglGetSyncAttribKhrAndroid);
  }
  if (std::strcmp(symbol, "eglDupNativeFenceFDANDROID") == 0) {
    return reinterpret_cast<void*>(&EglDupNativeFenceFdAndroid);
  }
  if (std::strcmp(symbol, "eglCreateSync") == 0) {
    return reinterpret_cast<void*>(&EglCreateSyncAndroid);
  }
  if (std::strcmp(symbol, "eglDestroySync") == 0) {
    return reinterpret_cast<void*>(&EglDestroySyncKhrAndroid);
  }
  if (std::strcmp(symbol, "eglClientWaitSync") == 0) {
    return reinterpret_cast<void*>(&EglClientWaitSyncKhrAndroid);
  }
  if (std::strcmp(symbol, "eglWaitSync") == 0) {
    return reinterpret_cast<void*>(&EglWaitSyncKhrAndroid);
  }
  if (std::strcmp(symbol, "eglGetSyncAttrib") == 0) {
    return reinterpret_cast<void*>(&EglGetSyncAttribAndroid);
  }
  if (std::strcmp(symbol, "eglGetDisplay") == 0) {
    return reinterpret_cast<void*>(&EglGetDisplayHost);
  }
  if (std::strcmp(symbol, "eglInitialize") == 0) {
    return reinterpret_cast<void*>(&EglInitializeHost);
  }
  if (std::strcmp(symbol, "eglChooseConfig") == 0) {
    return reinterpret_cast<void*>(&EglChooseConfigHost);
  }
  if (std::strcmp(symbol, "eglCreateContext") == 0) {
    return reinterpret_cast<void*>(&EglCreateContextHost);
  }
  if (std::strcmp(symbol, "eglCreatePbufferSurface") == 0) {
    return reinterpret_cast<void*>(&EglCreatePbufferSurfaceHost);
  }
  if (std::strcmp(symbol, "eglCreateWindowSurface") == 0) {
    return reinterpret_cast<void*>(&darwin_art_android_eglCreateWindowSurface);
  }
  if (std::strcmp(symbol, "eglSwapBuffers") == 0) {
    return reinterpret_cast<void*>(&darwin_art_android_eglSwapBuffers);
  }
  if (std::strcmp(symbol, "eglDestroySurface") == 0) {
    return reinterpret_cast<void*>(&darwin_art_android_eglDestroySurface);
  }
  if (std::strcmp(symbol, "eglTerminate") == 0)
    return reinterpret_cast<void*>(&TerminateHostDisplay);
  if (std::strcmp(symbol, "eglReleaseThread") == 0)
    return reinterpret_cast<void*>(&ReleaseHostThread);
  if (std::strcmp(symbol, "eglMakeCurrent") == 0) {
    return reinterpret_cast<void*>(&EglMakeCurrentHost);
  }
  if (std::strcmp(symbol, "glGetString") == 0) {
    return reinterpret_cast<void*>(&GlGetStringHost);
  }
  if (std::strcmp(symbol, "glGetStringi") == 0) {
    return reinterpret_cast<void*>(&GlGetStringiHost);
  }
  if (std::strcmp(symbol, "glGetIntegerv") == 0) {
    return reinterpret_cast<void*>(&GlGetIntegervHost);
  }
  if (std::strcmp(symbol, "glBlitFramebuffer") == 0 ||
      std::strcmp(symbol, "glBlitFramebufferANGLE") == 0 ||
      std::strcmp(symbol, "glBlitFramebufferCHROMIUM") == 0 ||
      std::strcmp(symbol, "glBlitFramebufferNV") == 0) {
    return reinterpret_cast<void*>(&GlBlitFramebufferAndroid);
  }
  if (std::strcmp(symbol, "glBindTexture") == 0) {
    return reinterpret_cast<void*>(&GlBindTextureAndroid);
  }
  if (std::strcmp(symbol, "glEGLImageTargetTexture2DOES") == 0) {
    return reinterpret_cast<void*>(&GlEglImageTargetTexture2dOes);
  }
  if (std::strcmp(symbol, "glFramebufferTexture2D") == 0) {
    return reinterpret_cast<void*>(&GlFramebufferTexture2dAndroid);
  }
  if (std::strcmp(symbol, "glFramebufferTexture2DMultisampleEXT") == 0) {
    return reinterpret_cast<void*>(
        &GlFramebufferTexture2dMultisampleExtAndroid);
  }
  if (std::strcmp(symbol, "glBindFramebuffer") == 0) {
    return reinterpret_cast<void*>(&GlBindFramebufferAndroid);
  }
  if (std::strcmp(symbol, "glGetFramebufferAttachmentParameteriv") == 0) {
    return reinterpret_cast<void*>(
        &GlGetFramebufferAttachmentParameterivAndroid);
  }
  if (std::strcmp(symbol, "glDeleteTextures") == 0) {
    return reinterpret_cast<void*>(&GlDeleteTexturesAndroid);
  }
  if (std::strcmp(symbol, "glTexImage2D") == 0) {
    return reinterpret_cast<void*>(&GlTexImage2dAndroid);
  }
  if (std::strcmp(symbol, "glTexStorage2D") == 0) {
    return reinterpret_cast<void*>(&GlTexStorage2dAndroid);
  }
  if (std::strcmp(symbol, "glTexStorage2DEXT") == 0) {
    return reinterpret_cast<void*>(&GlTexStorage2dExtAndroid);
  }
  return GetAngleApi().get_proc_address(symbol);
}

void GlBlitFramebufferAndroid(std::int32_t source_x0,
                              std::int32_t source_y0,
                              std::int32_t source_x1,
                              std::int32_t source_y1,
                              std::int32_t destination_x0,
                              std::int32_t destination_y0,
                              std::int32_t destination_x1,
                              std::int32_t destination_y1,
                              std::uint64_t android_mask_slot,
                              std::uint32_t filter) {
  auto& api = GetAngleApi();
  // AAPCS64 assigns every stack argument an eight-byte slot. Darwin arm64
  // packs 32-bit stack arguments at four-byte alignment. The first stack
  // argument is deliberately widened so Darwin reads Chromium's tenth
  // argument from sp+8, where Android placed it, rather than from AAPCS
  // padding at sp+4.
  const auto mask = static_cast<std::uint32_t>(android_mask_slot);
  if (DebugGraphicsDso()) {
    std::int32_t read_framebuffer = 0;
    std::int32_t draw_framebuffer = 0;
    api.gl_get_integer_v(0x8CAA, &read_framebuffer);
    api.gl_get_integer_v(0x8CA6, &draw_framebuffer);
    std::cerr << "ART Android GLES: blit read_fbo=" << read_framebuffer
              << " draw_fbo=" << draw_framebuffer << " source=["
              << source_x0 << "," << source_y0 << "," << source_x1 << ","
              << source_y1 << "] destination=[" << destination_x0 << ","
              << destination_y0 << "," << destination_x1 << ","
              << destination_y1 << "] mask=0x" << std::hex << mask
              << " filter=0x" << filter << std::dec << "\n";
  }
  api.gl_blit_framebuffer_angle(
      source_x0, source_y0, source_x1, source_y1, destination_x0,
      destination_y0, destination_x1, destination_y1, mask, filter);
}

std::uint32_t TextureBindingEnum(std::uint32_t target) {
  switch (target) {
    case 0x0DE0:  // GL_TEXTURE_1D
      return 0x8068;  // GL_TEXTURE_BINDING_1D
    case 0x0DE1:  // GL_TEXTURE_2D
      return 0x8069;  // GL_TEXTURE_BINDING_2D
    case 0x806F:  // GL_TEXTURE_3D
      return 0x806A;  // GL_TEXTURE_BINDING_3D
    case 0x8513:  // GL_TEXTURE_CUBE_MAP
      return 0x8514;  // GL_TEXTURE_BINDING_CUBE_MAP
    case 0x8C1A:  // GL_TEXTURE_2D_ARRAY
      return 0x8C1D;  // GL_TEXTURE_BINDING_2D_ARRAY
    default:
      return 0;
  }
}

template <typename StorageCall>
void WithBoundTexture(std::uint32_t texture, std::uint32_t target,
                      StorageCall&& storage_call) {
  auto& api = GetAngleApi();
  const std::uint32_t binding = TextureBindingEnum(target);
  if (!api.ready || binding == 0) return;
  std::int32_t previous = 0;
  api.gl_get_integer_v(binding, &previous);
  api.gl_bind_texture(target, texture);
  storage_call(api);
  api.gl_bind_texture(target, static_cast<std::uint32_t>(previous));
}

void GlTextureStorage1dExt(std::uint32_t texture, std::uint32_t target,
                           std::int32_t levels, std::uint32_t internal_format,
                           std::int32_t width) {
  WithBoundTexture(texture, target, [&](AngleApi& api) {
    api.gl_tex_storage_1d_ext(target, levels, internal_format, width);
  });
}

void GlTextureStorage2dExt(std::uint32_t texture, std::uint32_t target,
                           std::int32_t levels, std::uint32_t internal_format,
                           std::int32_t width, std::int32_t height) {
  WithBoundTexture(texture, target, [&](AngleApi& api) {
    api.gl_tex_storage_2d_ext(target, levels, internal_format, width, height);
  });
}

void GlTextureStorage3dExt(std::uint32_t texture, std::uint32_t target,
                           std::int32_t levels, std::uint32_t internal_format,
                           std::int32_t width, std::int32_t height,
                           std::int32_t depth) {
  WithBoundTexture(texture, target, [&](AngleApi& api) {
    api.gl_tex_storage_3d_ext(target, levels, internal_format, width, height,
                              depth);
  });
}

extern "C" void* darwin_art_angle_dso_symbol(const char* soname,
                                              const char* symbol) {
  if (soname == nullptr || symbol == nullptr) return nullptr;
  auto& api = GetAngleApi();
  if (!api.ready) return nullptr;
  if (std::strcmp(soname, "libEGL.so") == 0) {
    // Direct ELF relocations must resolve through the same Android libEGL
    // dispatch boundary as eglGetProcAddress.  In particular, passing an
    // Android ANativeWindow to ANGLE's Darwin eglCreateWindowSurface would
    // make its Metal backend reinterpret the BufferQueue producer as a
    // CALayer.  Keep all surface lifecycle calls on the Android window bridge
    // instead of exposing ANGLE's raw Darwin entry points.
    if (std::strcmp(symbol, "eglGetDisplay") == 0) {
      return reinterpret_cast<void*>(&EglGetDisplayHost);
    }
    if (std::strcmp(symbol, "eglGetError") == 0) {
      return reinterpret_cast<void*>(&darwin_art_android_eglGetError);
    }
    if (std::strcmp(symbol, "eglInitialize") == 0) {
      return reinterpret_cast<void*>(&EglInitializeHost);
    }
    if (std::strcmp(symbol, "eglChooseConfig") == 0) {
      return reinterpret_cast<void*>(&EglChooseConfigHost);
    }
    if (std::strcmp(symbol, "eglCreateContext") == 0) {
      return reinterpret_cast<void*>(&EglCreateContextHost);
    }
    if (std::strcmp(symbol, "eglCreatePbufferSurface") == 0) {
      return reinterpret_cast<void*>(&EglCreatePbufferSurfaceHost);
    }
    if (std::strcmp(symbol, "eglCreateWindowSurface") == 0) {
      return reinterpret_cast<void*>(
          &darwin_art_android_eglCreateWindowSurface);
    }
    if (std::strcmp(symbol, "eglSwapBuffers") == 0 ||
        std::strcmp(symbol, "eglSwapBuffersWithDamageKHR") == 0 ||
        std::strcmp(symbol, "eglSwapBuffersWithDamageEXT") == 0) {
      return reinterpret_cast<void*>(&darwin_art_android_eglSwapBuffers);
    }
    if (std::strcmp(symbol, "eglDestroySurface") == 0) {
      return reinterpret_cast<void*>(&darwin_art_android_eglDestroySurface);
    }
    if (std::strcmp(symbol, "eglTerminate") == 0)
      return reinterpret_cast<void*>(&TerminateHostDisplay);
    if (std::strcmp(symbol, "eglReleaseThread") == 0)
      return reinterpret_cast<void*>(&ReleaseHostThread);
    if (std::strcmp(symbol, "eglMakeCurrent") == 0) {
      return reinterpret_cast<void*>(&darwin_art_android_eglMakeCurrent);
    }
    if (std::strcmp(symbol, "eglQuerySurface") == 0) {
      return reinterpret_cast<void*>(&darwin_art_android_eglQuerySurface);
    }
    if (std::strcmp(symbol, "eglSurfaceAttrib") == 0) {
      return reinterpret_cast<void*>(&darwin_art_android_eglSurfaceAttrib);
    }
    if (std::strcmp(symbol, "eglSwapInterval") == 0) {
      return reinterpret_cast<void*>(&darwin_art_android_eglSwapInterval);
    }
    if (std::strcmp(symbol, "eglSetDamageRegionKHR") == 0) {
      return reinterpret_cast<void*>(&darwin_art_android_eglSetDamageRegion);
    }
    if (std::strcmp(symbol, "eglBeginFrame") == 0) {
      return reinterpret_cast<void*>(&eglBeginFrame);
    }
    if (std::strcmp(symbol, "eglGetProcAddress") == 0) {
      return reinterpret_cast<void*>(&EglGetProcAddressMetal);
    }
    if (std::strcmp(symbol, "eglGetPlatformDisplayEXT") == 0) {
      return reinterpret_cast<void*>(&EglGetPlatformDisplayExtMetal);
    }
    if (std::strcmp(symbol, "eglGetPlatformDisplay") == 0) {
      return reinterpret_cast<void*>(&EglGetPlatformDisplayMetal);
    }
    if (std::strcmp(symbol, "eglQueryString") == 0) {
      return reinterpret_cast<void*>(&EglQueryStringAndroid);
    }
    if (std::strcmp(symbol, "eglGetNativeClientBufferANDROID") == 0) {
      return reinterpret_cast<void*>(&EglGetNativeClientBufferAndroid);
    }
    if (std::strcmp(symbol, "eglCreateImageKHR") == 0) {
      return reinterpret_cast<void*>(&EglCreateImageAndroid);
    }
    if (std::strcmp(symbol, "eglCreateImage") == 0) {
      return reinterpret_cast<void*>(&EglCreateImageAndroidCore);
    }
    if (std::strcmp(symbol, "eglDestroyImageKHR") == 0) {
      return reinterpret_cast<void*>(&EglDestroyImageAndroid);
    }
    if (std::strcmp(symbol, "eglDestroyImage") == 0) {
      return reinterpret_cast<void*>(&EglDestroyImageAndroidCore);
    }
    if (std::strcmp(symbol, "eglCreateSyncKHR") == 0) {
      return reinterpret_cast<void*>(&EglCreateSyncKhrAndroid);
    }
    if (std::strcmp(symbol, "eglDestroySyncKHR") == 0) {
      return reinterpret_cast<void*>(&EglDestroySyncKhrAndroid);
    }
    if (std::strcmp(symbol, "eglClientWaitSyncKHR") == 0) {
      return reinterpret_cast<void*>(&EglClientWaitSyncKhrAndroid);
    }
    if (std::strcmp(symbol, "eglWaitSyncKHR") == 0) {
      return reinterpret_cast<void*>(&EglWaitSyncKhrAndroid);
    }
    if (std::strcmp(symbol, "eglGetSyncAttribKHR") == 0) {
      return reinterpret_cast<void*>(&EglGetSyncAttribKhrAndroid);
    }
    if (std::strcmp(symbol, "eglDupNativeFenceFDANDROID") == 0) {
      return reinterpret_cast<void*>(&EglDupNativeFenceFdAndroid);
    }
    if (std::strcmp(symbol, "eglCreateSync") == 0) {
      return reinterpret_cast<void*>(&EglCreateSyncAndroid);
    }
    if (std::strcmp(symbol, "eglDestroySync") == 0) {
      return reinterpret_cast<void*>(&EglDestroySyncKhrAndroid);
    }
    if (std::strcmp(symbol, "eglClientWaitSync") == 0) {
      return reinterpret_cast<void*>(&EglClientWaitSyncKhrAndroid);
    }
    if (std::strcmp(symbol, "eglWaitSync") == 0) {
      return reinterpret_cast<void*>(&EglWaitSyncKhrAndroid);
    }
    if (std::strcmp(symbol, "eglGetSyncAttrib") == 0) {
      return reinterpret_cast<void*>(&EglGetSyncAttribAndroid);
    }
    // Chromium's Android GL loader opens libEGL.so and probes GLES extension
    // entry points with dlsym on that same handle before falling back to
    // eglGetProcAddress. Android's libEGL is a dispatch DSO, so preserve the
    // same behavior in the virtual namespace instead of exposing ANGLE's raw
    // host entry point and bypassing the AHardwareBuffer metadata bridge.
    if (std::strcmp(symbol, "glEGLImageTargetTexture2DOES") == 0) {
      return reinterpret_cast<void*>(&GlEglImageTargetTexture2dOes);
    }
    if (std::strcmp(symbol, "glFramebufferTexture2D") == 0) {
      return reinterpret_cast<void*>(&GlFramebufferTexture2dAndroid);
    }
    if (std::strcmp(symbol, "glFramebufferTexture2DMultisampleEXT") == 0) {
      return reinterpret_cast<void*>(
          &GlFramebufferTexture2dMultisampleExtAndroid);
    }
    if (std::strcmp(symbol, "glBindFramebuffer") == 0) {
      return reinterpret_cast<void*>(&GlBindFramebufferAndroid);
    }
    if (std::strcmp(symbol, "glGetFramebufferAttachmentParameteriv") == 0) {
      return reinterpret_cast<void*>(
          &GlGetFramebufferAttachmentParameterivAndroid);
    }
    if (std::strcmp(symbol, "glBlitFramebuffer") == 0 ||
        std::strcmp(symbol, "glBlitFramebufferANGLE") == 0 ||
        std::strcmp(symbol, "glBlitFramebufferCHROMIUM") == 0 ||
        std::strcmp(symbol, "glBlitFramebufferNV") == 0) {
      return reinterpret_cast<void*>(&GlBlitFramebufferAndroid);
    }
    void* result = dlsym(api.egl_library, symbol);
    if (result != nullptr) return result;
    if (std::strcmp(symbol, "glTextureStorage1DEXT") == 0) {
      return reinterpret_cast<void*>(&GlTextureStorage1dExt);
    }
    if (std::strcmp(symbol, "glTextureStorage2DEXT") == 0) {
      return reinterpret_cast<void*>(&GlTextureStorage2dExt);
    }
    if (std::strcmp(symbol, "glTextureStorage3DEXT") == 0) {
      return reinterpret_cast<void*>(&GlTextureStorage3dExt);
    }
    return nullptr;
  }
  if (std::strcmp(soname, "libGLESv2.so") == 0) {
    if (std::strcmp(symbol, "glGetString") == 0) {
      return reinterpret_cast<void*>(&GlGetStringHost);
    }
    if (std::strcmp(symbol, "glGetStringi") == 0) {
      return reinterpret_cast<void*>(&GlGetStringiHost);
    }
    if (std::strcmp(symbol, "glGetIntegerv") == 0) {
      return reinterpret_cast<void*>(&GlGetIntegervHost);
    }
    if (std::strcmp(symbol, "glBlitFramebuffer") == 0 ||
        std::strcmp(symbol, "glBlitFramebufferANGLE") == 0 ||
        std::strcmp(symbol, "glBlitFramebufferCHROMIUM") == 0 ||
        std::strcmp(symbol, "glBlitFramebufferNV") == 0) {
      return reinterpret_cast<void*>(&GlBlitFramebufferAndroid);
    }
    if (std::strcmp(symbol, "glBindTexture") == 0) {
      return reinterpret_cast<void*>(&GlBindTextureAndroid);
    }
    if (std::strcmp(symbol, "glEGLImageTargetTexture2DOES") == 0) {
      return reinterpret_cast<void*>(&GlEglImageTargetTexture2dOes);
    }
    if (std::strcmp(symbol, "glFramebufferTexture2D") == 0) {
      return reinterpret_cast<void*>(&GlFramebufferTexture2dAndroid);
    }
    if (std::strcmp(symbol, "glFramebufferTexture2DMultisampleEXT") == 0) {
      return reinterpret_cast<void*>(
          &GlFramebufferTexture2dMultisampleExtAndroid);
    }
    if (std::strcmp(symbol, "glBindFramebuffer") == 0) {
      return reinterpret_cast<void*>(&GlBindFramebufferAndroid);
    }
    if (std::strcmp(symbol, "glGetFramebufferAttachmentParameteriv") == 0) {
      return reinterpret_cast<void*>(
          &GlGetFramebufferAttachmentParameterivAndroid);
    }
    if (std::strcmp(symbol, "glDeleteTextures") == 0) {
      return reinterpret_cast<void*>(&GlDeleteTexturesAndroid);
    }
    if (std::strcmp(symbol, "glTexImage2D") == 0) {
      return reinterpret_cast<void*>(&GlTexImage2dAndroid);
    }
    if (std::strcmp(symbol, "glTexStorage2D") == 0) {
      return reinterpret_cast<void*>(&GlTexStorage2dAndroid);
    }
    if (std::strcmp(symbol, "glTexStorage2DEXT") == 0) {
      return reinterpret_cast<void*>(&GlTexStorage2dExtAndroid);
    }
    return dlsym(api.gles_library, symbol);
  }
  return nullptr;
}

}  // namespace darwin_art
