#include "darwin_angle_egl.h"

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
#include <vector>

namespace darwin_art {
const char* EglQueryStringAndroid(void* display, std::int32_t name);
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
constexpr EGLint kEglSurfaceType = 0x3033;
constexpr EGLint kEglPbufferBit = 0x0001;
constexpr EGLint kEglBindToTextureRgba = 0x303B;
constexpr EGLint kEglTrue = 1;
constexpr EGLint kEglNotInitialized = 0x3001;
constexpr EGLenum kEglNativeBufferAndroid = 0x3140;
constexpr EGLenum kEglSyncNativeFenceAndroid = 0x3144;
constexpr EGLint kEglSyncNativeFenceFdAndroid = 0x3145;
constexpr EGLint kEglNoNativeFenceFdAndroid = -1;
constexpr EGLenum kEglSyncMetalSharedEventAngle = 0x34D8;
constexpr EGLint kEglSyncMetalSharedEventObjectAngle = 0x34D9;
constexpr EGLint kEglSyncMetalSharedEventSignalValueLoAngle = 0x34DA;
constexpr EGLint kEglSyncMetalSharedEventSignalValueHiAngle = 0x34DB;
constexpr EGLenum kEglPlatformAngle = 0x3202;
constexpr EGLint kEglPlatformAngleType = 0x3203;
constexpr EGLint kEglPlatformAngleTypeOpenGles = 0x320E;
constexpr EGLint kEglPlatformAngleTypeMetal = 0x3489;
constexpr EGLenum kEglIosurfaceAngle = 0x3454;
constexpr EGLint kEglDeviceExt = 0x322C;
constexpr EGLint kEglMetalDeviceAngle = 0x34A6;
constexpr EGLenum kEglMetalTextureAngle = 0x34A7;
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
  EGLContext context = api.create_context(
      HandleAs<EGLDisplay>(env, display, "mEGLDisplay"),
      HandleAs<EGLConfig>(env, config, "mEGLConfig"),
      HandleAs<EGLContext>(env, share, "mEGLContext"),
      values.empty() ? nullptr : values.data());
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

std::array<EGLint, 5> HostPbufferAttributes() {
  const bool retina =
      std::getenv("DARWIN_ART_WINDOW_SCALE") != nullptr &&
      std::string(std::getenv("DARWIN_ART_WINDOW_SCALE")) == "2";
  return {kEglWidth, retina ? 720 : 360, kEglHeight,
          retina ? 1280 : 640, kEglNone};
}

struct HostWindowSurface {
  DarwinArtSurface* host = nullptr;
  void* native_window = nullptr;
  void* native_buffer = nullptr;
  EGLConfig config = nullptr;
  EGLint bind_target = 0;
  void* iosurface = nullptr;
  EGLSurface iosurface_target = nullptr;
  std::uint32_t texture_target = 0;
  std::uint32_t texture = 0;
  std::uint32_t framebuffer = 0;
  std::uint32_t render_width = 0;
  std::uint32_t render_height = 0;
  std::uint32_t width = 0;
  std::uint32_t height = 0;
  bool target_bound = false;
  bool owns_iosurface_ref = false;
};

std::mutex& HostWindowSurfaceMutex() {
  static std::mutex mutex;
  return mutex;
}

std::unordered_map<EGLSurface, HostWindowSurface>& HostWindowSurfaces() {
  static std::unordered_map<EGLSurface, HostWindowSurface> surfaces;
  return surfaces;
}

EGLSurface CreateHostWindowSurface(EGLDisplay native_display,
                                   EGLConfig native_config,
                                   void* native_window = nullptr) {
  auto& api = GetAngleApi();
  if (!api.ready) return nullptr;
  DarwinArtSurface* host = darwin_art_surface_active_gpu();
  void* iosurface = nullptr;
  uint32_t backing_width = 0;
  uint32_t backing_height = 0;
  EGLSurface surface = nullptr;
  void* native_buffer = nullptr;
  int acquire_fence = -1;
  AHardwareBuffer* hardware_buffer = nullptr;
  bool owns_iosurface_ref = false;
  bool has_iosurface = false;
  if (native_window != nullptr &&
      darwin_art_android_ANativeWindow_dequeue_hardware_buffer(
          native_window, &hardware_buffer, &native_buffer, &acquire_fence) ==
          0) {
    if (acquire_fence >= 0) {
      (void)sync_wait(acquire_fence, -1);
      (void)darwin_art_bionic_socket_broker_close(acquire_fence);
      acquire_fence = -1;
    }
    AHardwareBuffer_Desc description{};
    AHardwareBuffer_describe(hardware_buffer, &description);
    iosurface = darwin_art_android_hardware_buffer_iosurface(hardware_buffer);
    backing_width = description.width;
    backing_height = description.height;
    has_iosurface = iosurface != nullptr && backing_width != 0 &&
                    backing_height != 0;
    if (!has_iosurface) {
      (void)darwin_art_android_ANativeWindow_cancel_hardware_buffer(
          native_window, native_buffer, -1);
      native_buffer = nullptr;
    }
  }
  if (!has_iosurface) {
    has_iosurface =
        host != nullptr && darwin_art_surface_gpu_acquire_iosurface(
                               host, &iosurface, &backing_width,
                               &backing_height);
    owns_iosurface_ref = has_iosurface;
  }
  if (!has_iosurface) {
    const char* inherited_id = std::getenv("DARWIN_ART_HOST_IOSURFACE_ID");
    if (inherited_id != nullptr) {
      char* end = nullptr;
      const unsigned long parsed = std::strtoul(inherited_id, &end, 10);
      if (end != inherited_id && *end == '\0' && parsed <= UINT32_MAX) {
        has_iosurface = darwin_art_surface_gpu_lookup_iosurface(
            static_cast<uint32_t>(parsed), &iosurface, &backing_width,
            &backing_height);
        if (has_iosurface) {
          std::cerr << "ART Android EGL: imported host IOSurface id="
                    << parsed << " " << backing_width << "x"
                    << backing_height << "\n";
        }
      }
    }
  }
  if (has_iosurface) {
    EGLint bind_target = 0;
    int32_t embedded_x = 0;
    int32_t embedded_y = 0;
    uint32_t embedded_width = backing_width;
    uint32_t embedded_height = backing_height;
    if (native_window == nullptr) {
      darwin_art_surface_gpu_get_embedded_geometry(
          host, &embedded_x, &embedded_y, &embedded_width, &embedded_height);
    }
    (void)embedded_x;
    (void)embedded_y;
    const uint32_t requested_width = std::min(
        backing_width, std::max<uint32_t>(1, embedded_width));
    const uint32_t requested_height = std::min(
        backing_height, std::max<uint32_t>(1, embedded_height));
    if (api.get_config_attrib(native_display, native_config,
                              kEglBindToTextureTargetAngle, &bind_target)) {
      const EGLint attributes[] = {
          kEglWidth,
          static_cast<EGLint>(requested_width),
          kEglHeight,
          static_cast<EGLint>(requested_height),
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
      EGLSurface iosurface_target = api.create_pbuffer_from_client_buffer(
          native_display, kEglIosurfaceAngle, iosurface, native_config,
          attributes);
      const EGLint render_attributes[] = {
          kEglWidth, static_cast<EGLint>(requested_width), kEglHeight,
          static_cast<EGLint>(requested_height), kEglNone};
      surface = iosurface_target == nullptr
                    ? nullptr
                    : api.create_pbuffer_surface(native_display, native_config,
                                                 render_attributes);
      if (surface != nullptr) {
        const std::uint32_t gl_texture_target =
            bind_target == kEglTextureRectangleAngle
                ? kGlTextureRectangleAngle
                : (bind_target == kEglTexture2d ? kGlTexture2d : 0);
        if (gl_texture_target != 0) {
          std::lock_guard<std::mutex> lock(HostWindowSurfaceMutex());
          HostWindowSurfaces().emplace(
              surface,
              HostWindowSurface{.host = host,
                                .native_window = native_window,
                                .native_buffer = native_buffer,
                                .config = native_config,
                                .bind_target = bind_target,
                                .iosurface = iosurface,
                                .iosurface_target = iosurface_target,
                                .texture_target = gl_texture_target,
                                .render_width = requested_width,
                                .render_height = requested_height,
                                .width = requested_width,
                                .height = requested_height,
                                .owns_iosurface_ref = owns_iosurface_ref});
          if (native_window != nullptr) {
            darwin_art_android_ANativeWindow_acquire(native_window);
          }
          std::cerr << "ART Android EGL: SurfaceView uses GPU blit into shared "
                       "IOSurface "
                    << requested_width << "x" << requested_height << "\n";
          return surface;
        }
        api.destroy_surface(native_display, surface);
        surface = nullptr;
      }
      if (iosurface_target != nullptr) {
        api.destroy_surface(native_display, iosurface_target);
      }
    }
    if (surface == nullptr) {
      if (owns_iosurface_ref) {
        darwin_art_surface_gpu_release_iosurface(iosurface);
      }
      if (native_buffer != nullptr) {
        (void)darwin_art_android_ANativeWindow_cancel_hardware_buffer(
            native_window, native_buffer, -1);
        native_buffer = nullptr;
      }
      iosurface = nullptr;
      std::cerr << "ART Android EGL: IOSurface EGL target unavailable error=0x"
                << std::hex << api.get_error() << std::dec
                << "; retaining GPU pbuffer fallback\n";
    }
  }
  if (surface == nullptr) {
    const auto attributes = HostPbufferAttributes();
    surface = api.create_pbuffer_surface(native_display, native_config,
                                         attributes.data());
  }
  return surface;
}

jlong EglCreateWindowSurface(JNIEnv* env, jobject, jobject display,
                             jobject config, jobject, jintArray) {
  return static_cast<jlong>(reinterpret_cast<std::uintptr_t>(
      CreateHostWindowSurface(
          HandleAs<EGLDisplay>(env, display, "mEGLDisplay"),
          HandleAs<EGLConfig>(env, config, "mEGLConfig"), nullptr)));
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
  const EGLBoolean ok = api.initialize(
      HandleAs<EGLDisplay>(env, display, "mEGLDisplay"), &major, &minor);
  if (version != nullptr && env->GetArrayLength(version) >= 2) {
    const jint values[] = {major, minor};
    env->SetIntArrayRegion(version, 0, 2, values);
  }
  return ok ? JNI_TRUE : JNI_FALSE;
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
  return api.ready &&
                 api.make_current(
                     HandleAs<EGLDisplay>(env, display, "mEGLDisplay"),
                     HandleAs<EGLSurface>(env, draw, "mEGLSurface"),
                     HandleAs<EGLSurface>(env, read, "mEGLSurface"),
                     HandleAs<EGLContext>(env, context, "mEGLContext"))
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
  auto& api = GetAngleApi();
  HostWindowSurface window;
  {
    std::lock_guard<std::mutex> lock(HostWindowSurfaceMutex());
    auto found = HostWindowSurfaces().find(native_surface);
    if (found != HostWindowSurfaces().end()) {
      window = found->second;
      HostWindowSurfaces().erase(found);
    }
  }
  if (window.target_bound) {
    std::int32_t previous_texture = 0;
    api.gl_get_integer_v(window.texture_target == kGlTexture2d ? 0x8069
                                                               : 0x84F6,
                         &previous_texture);
    api.gl_bind_texture(window.texture_target, window.texture);
    api.release_tex_image(native_display, window.iosurface_target,
                          kEglBackBuffer);
    api.gl_delete_framebuffers(1, &window.framebuffer);
    api.gl_delete_textures(1, &window.texture);
    api.gl_bind_texture(window.texture_target,
                        static_cast<std::uint32_t>(previous_texture));
  }
  if (window.iosurface_target != nullptr) {
    api.destroy_surface(native_display, window.iosurface_target);
  }
  if (window.owns_iosurface_ref) {
    darwin_art_surface_gpu_release_iosurface(window.iosurface);
  }
  if (window.native_buffer != nullptr) {
    (void)darwin_art_android_ANativeWindow_cancel_hardware_buffer(
        window.native_window, window.native_buffer, -1);
  }
  if (window.native_window != nullptr) {
    darwin_art_android_ANativeWindow_release(window.native_window);
  }
  return api.ready && api.destroy_surface(native_display, native_surface);
}
jboolean EglDestroySurface(JNIEnv* env, jobject, jobject display,
                           jobject surface) {
  return DestroyHostWindowSurface(
             HandleAs<EGLDisplay>(env, display, "mEGLDisplay"),
             HandleAs<EGLSurface>(env, surface, "mEGLSurface"))
             ? JNI_TRUE
             : JNI_FALSE;
}
jboolean EglTerminate(JNIEnv* env, jobject, jobject display) {
  auto& api = GetAngleApi();
  return api.ready && api.terminate(
                          HandleAs<EGLDisplay>(env, display, "mEGLDisplay"))
             ? JNI_TRUE
             : JNI_FALSE;
}
bool SwapHostWindowSurface(EGLDisplay native_display,
                           EGLSurface native_surface) {
  auto& api = GetAngleApi();
  if (!api.ready) return false;
  DarwinArtSurface* host = nullptr;
  void* native_window = nullptr;
  EGLConfig native_config = nullptr;
  EGLint native_bind_target = 0;
  bool transferred = false;
  uint32_t transferred_width = 0;
  uint32_t transferred_height = 0;
  {
    std::lock_guard<std::mutex> lock(HostWindowSurfaceMutex());
    auto found = HostWindowSurfaces().find(native_surface);
    if (found != HostWindowSurfaces().end()) {
      HostWindowSurface& window = found->second;
      host = window.host;
      native_window = window.native_window;
      native_config = window.config;
      native_bind_target = window.bind_target;
      std::int32_t previous_active_texture = 0;
      std::int32_t previous_texture = 0;
      std::int32_t previous_read_framebuffer = 0;
      std::int32_t previous_draw_framebuffer = 0;
      std::int32_t previous_scissor[4] = {};
      const bool scissor_enabled = api.gl_is_enabled(0x0C11) != 0;
      api.gl_get_integer_v(0x84E0, &previous_active_texture);  // GL_ACTIVE_TEXTURE
      api.gl_get_integer_v(window.texture_target == kGlTexture2d ? 0x8069
                                                                 : 0x84F6,
                           &previous_texture);
      api.gl_get_integer_v(0x8CAA,
                           &previous_read_framebuffer);  // GL_READ_FRAMEBUFFER_BINDING
      api.gl_get_integer_v(0x8CA6,
                           &previous_draw_framebuffer);  // GL_DRAW_FRAMEBUFFER_BINDING
      api.gl_get_integer_v(0x0C10, previous_scissor);  // GL_SCISSOR_BOX
      // AppKit resizing replaces the host IOSurface atomically. Android keeps
      // the same Surface/ANativeWindow identity across surfaceChanged(), so
      // refresh only the EGL client-buffer target while preserving the app's
      // current context and render pbuffer. The subsequent framebuffer blit
      // is still GPU-only and scales the old swapchain extent into the new
      // Android backing extent.
      void* current_iosurface = nullptr;
      uint32_t current_width = 0;
      uint32_t current_height = 0;
      if (window.native_window == nullptr &&
          darwin_art_surface_gpu_acquire_iosurface(
              host, &current_iosurface, &current_width, &current_height)) {
        // A BufferQueue producer keeps its negotiated buffer dimensions when
        // only the consumer's window grows; SurfaceFlinger scales that buffer
        // into the new layer bounds. Keep the existing EGL producer extent
        // for the same Android Surface identity and let the Metal compositor
        // perform the equivalent scaling. Forcing the client-buffer target to
        // the AppKit window extent makes ANGLE reject the scaled framebuffer
        // blit and freezes the last pre-resize picture.
        const uint32_t requested_width =
            std::min(current_width, std::max<uint32_t>(1, window.render_width));
        const uint32_t requested_height = std::min(
            current_height, std::max<uint32_t>(1, window.render_height));
        if (current_iosurface != window.iosurface ||
            requested_width != window.width ||
            requested_height != window.height) {
          const EGLint attributes[] = {
              kEglWidth,
              static_cast<EGLint>(requested_width),
              kEglHeight,
              static_cast<EGLint>(requested_height),
              kEglIosurfacePlaneAngle,
              0,
              kEglTextureTarget,
              window.bind_target,
              kEglTextureInternalFormatAngle,
              kGlBgraExt,
              kEglTextureFormat,
              kEglTextureRgba,
              kEglTextureTypeAngle,
              kGlUnsignedByte,
              kEglNone,
          };
          EGLSurface replacement = api.create_pbuffer_from_client_buffer(
              native_display, kEglIosurfaceAngle, current_iosurface,
              window.config, attributes);
          if (replacement != nullptr) {
            if (window.target_bound) {
              api.gl_bind_texture(window.texture_target, window.texture);
              api.release_tex_image(native_display, window.iosurface_target,
                                    kEglBackBuffer);
              api.gl_delete_framebuffers(1, &window.framebuffer);
              api.gl_delete_textures(1, &window.texture);
            }
            api.destroy_surface(native_display, window.iosurface_target);
            darwin_art_surface_gpu_release_iosurface(window.iosurface);
            window.iosurface = current_iosurface;
            window.iosurface_target = replacement;
            window.texture = 0;
            window.framebuffer = 0;
            window.width = requested_width;
            window.height = requested_height;
            window.target_bound = false;
            window.owns_iosurface_ref = true;
            current_iosurface = nullptr;
            std::cerr << "ART Android EGL: refreshed resized IOSurface target "
                      << requested_width << "x" << requested_height << "\n";
          }
        }
        if (current_iosurface != nullptr) {
          darwin_art_surface_gpu_release_iosurface(current_iosurface);
        }
      }
      if (!window.target_bound) {
        api.gl_gen_textures(1, &window.texture);
        api.gl_bind_texture(window.texture_target, window.texture);
        api.gl_tex_parameter_i(window.texture_target, 0x2801,
                               0x2601);  // MIN_FILTER / LINEAR
        api.gl_tex_parameter_i(window.texture_target, 0x2800,
                               0x2601);  // MAG_FILTER / LINEAR
        if (api.bind_tex_image(native_display, window.iosurface_target,
                               kEglBackBuffer)) {
          api.gl_gen_framebuffers(1, &window.framebuffer);
          api.gl_bind_framebuffer(kGlFramebuffer, window.framebuffer);
          api.gl_framebuffer_texture_2d(
              kGlFramebuffer, kGlColorAttachment0, window.texture_target,
              window.texture, 0);
          window.target_bound =
              api.gl_check_framebuffer_status(kGlFramebuffer) ==
              kGlFramebufferComplete;
        }
        if (!window.target_bound) {
          std::cerr << "ART Android EGL: IOSurface transfer framebuffer "
                       "incomplete error=0x"
                    << std::hex << api.gl_get_error() << std::dec << "\n";
        }
      }
      if (window.target_bound) {
        api.gl_bind_framebuffer(kGlFramebuffer, 0);
        api.gl_bind_framebuffer(kGlDrawFramebuffer, window.framebuffer);
        // glBlitFramebuffer honors GL_SCISSOR_TEST. Native renderers commonly
        // leave a swapchain-sized scissor enabled; after growth that would
        // update only the old extent and leave a black strip in the resized
        // IOSurface. Preserve the app's state around our transfer.
        api.gl_disable(0x0C11);  // GL_SCISSOR_TEST
        api.gl_blit_framebuffer_angle(
            0, 0, static_cast<int32_t>(window.render_width),
            static_cast<int32_t>(window.render_height), 0, 0,
            static_cast<int32_t>(window.width),
            static_cast<int32_t>(window.height), 0x00004000,
            0x2600);  // GL_COLOR_BUFFER_BIT / GL_NEAREST
        if (std::getenv("DARWIN_ART_DEBUG_GRAPHICS_DSO") != nullptr) {
          const std::uint32_t transfer_error = api.gl_get_error();
          if (transfer_error != 0) {
            std::cerr << "ART Android EGL: IOSurface blit error=0x" << std::hex
                      << transfer_error << std::dec
                      << " source_fbo=0 target_fbo=" << window.framebuffer
                      << " source=" << window.render_width << "x"
                      << window.render_height << " target=" << window.width
                      << "x" << window.height << "\n";
          }
        }
        // GL error state belongs to the guest context. Reading it here both
        // steals an app-visible error and can misattribute an earlier guest
        // error to this transfer, which used to suppress every publication
        // after a resize. A complete destination FBO plus the queued blit is
        // the host-side success contract; wait_gl() below establishes GPU
        // completion before the IOSurface generation is published.
        transferred = true;
        if (transferred) {
          transferred_width = std::min(window.render_width, window.width);
          transferred_height = std::min(window.render_height, window.height);
        }
      }
      api.gl_bind_framebuffer(0x8CA8, previous_read_framebuffer);  // GL_READ_FRAMEBUFFER
      api.gl_bind_framebuffer(kGlDrawFramebuffer, previous_draw_framebuffer);
      api.gl_scissor(previous_scissor[0], previous_scissor[1],
                     previous_scissor[2], previous_scissor[3]);
      if (scissor_enabled) api.gl_enable(0x0C11);  // GL_SCISSOR_TEST
      api.gl_bind_texture(window.texture_target,
                          static_cast<uint32_t>(previous_texture));
      api.gl_active_texture(static_cast<uint32_t>(previous_active_texture));
    }
  }
  static std::atomic<uint32_t> debug_clear_frames{0};
  if (std::getenv("DARWIN_ART_DEBUG_ANGLE_CLEAR") != nullptr &&
      debug_clear_frames.fetch_add(1, std::memory_order_relaxed) < 120) {
    using ColorMaskFunction = void (*)(uint8_t, uint8_t, uint8_t, uint8_t);
    static ColorMaskFunction color_mask = LoadSymbol<ColorMaskFunction>(
        api.gles_library, "glColorMask");
    if (color_mask != nullptr) color_mask(1, 1, 1, 1);
    api.gl_disable(0x0C11);  // GL_SCISSOR_TEST
    api.gl_clear_color(1.0f, 0.0f, 0.0f, 1.0f);
    api.gl_clear(0x00004000);  // GL_COLOR_BUFFER_BIT
  }
  if (std::getenv("DARWIN_ART_DEBUG_ANGLE") != nullptr) {
    static std::atomic<uint32_t> sampled_frames{0};
    const uint32_t sample =
        sampled_frames.fetch_add(1, std::memory_order_relaxed);
    if (sample < 4 || sample % 60 == 59) {
      std::int32_t viewport[4] = {};
      std::int32_t framebuffer = 0;
      std::int32_t color_bits[4] = {};
      std::int32_t scissor[4] = {};
      std::int32_t surface_width = 0;
      std::int32_t surface_height = 0;
      std::uint8_t pixel[4] = {};
      api.gl_get_integer_v(0x0BA2, viewport);  // GL_VIEWPORT
      api.gl_get_integer_v(0x8CA6, &framebuffer);  // GL_FRAMEBUFFER_BINDING
      api.gl_get_integer_v(0x0D52, &color_bits[0]);  // GL_RED_BITS
      api.gl_get_integer_v(0x0D53, &color_bits[1]);  // GL_GREEN_BITS
      api.gl_get_integer_v(0x0D54, &color_bits[2]);  // GL_BLUE_BITS
      api.gl_get_integer_v(0x0D55, &color_bits[3]);  // GL_ALPHA_BITS
      api.gl_get_integer_v(0x0C10, scissor);  // GL_SCISSOR_BOX
      api.query_surface(native_display, native_surface, kEglWidth,
                        &surface_width);
      api.query_surface(native_display, native_surface, kEglHeight,
                        &surface_height);
      const EGLSurface current_draw = api.get_current_surface(0x3059);
      api.gl_read_pixels(viewport[0] + viewport[2] / 2,
                         viewport[1] + viewport[3] / 2, 1, 1, 0x1908,
                         0x1401, pixel);  // GL_RGBA / GL_UNSIGNED_BYTE
      std::cerr << "ART Android EGL: pre-swap frame=" << sample + 1
                << " viewport=" << viewport[0] << "," << viewport[1] << ","
                << viewport[2] << "x" << viewport[3] << " center_rgba="
                << static_cast<int>(pixel[0]) << ","
                << static_cast<int>(pixel[1]) << ","
                << static_cast<int>(pixel[2]) << ","
                << static_cast<int>(pixel[3]) << " fbo=" << framebuffer
                << " surface=" << surface_width << "x" << surface_height
                << " bits=" << color_bits[0] << "," << color_bits[1] << ","
                << color_bits[2] << "," << color_bits[3] << " scissor="
                << scissor[0] << "," << scissor[1] << "," << scissor[2]
                << "x" << scissor[3]
                << " current_match=" << (current_draw == native_surface)
                << " error=0x" << std::hex
                << api.gl_get_error() << std::dec << "\n";
    }
  }
  if (!api.swap_buffers(native_display, native_surface)) return false;
  // Establish completion and visibility between ANGLE's Metal command queue
  // and the independent Skia/Metal queue before publishing this texture.
  const bool wait_complete = transferred && api.wait_gl();
  const bool gpu_complete = transferred && wait_complete;
  if (std::getenv("DARWIN_ART_DEBUG_GRAPHICS_DSO") != nullptr) {
    std::cerr << "ART Android EGL: swap publication transferred="
              << (transferred ? 1 : 0)
              << " wait_complete=" << (wait_complete ? 1 : 0)
              << " native_window=" << native_window << "\n";
  }
  if (host != nullptr && native_window == nullptr && gpu_complete) {
    darwin_art_surface_gpu_set_embedded_buffer_extent(
        host, transferred_width, transferred_height);
    darwin_art_surface_gpu_publish_embedded(host);
    if (std::getenv("DARWIN_ART_DEBUG_ANGLE") != nullptr) {
      static std::atomic<uint32_t> reported_frames{0};
      const uint32_t frame =
          reported_frames.fetch_add(1, std::memory_order_relaxed) + 1;
      if (frame <= 4 || frame % 60 == 0) {
        std::cerr << "ART Android EGL: published IOSurface frame=" << frame
                  << " gl_error=0x" << std::hex << api.gl_get_error()
                  << std::dec << "\n";
      }
    }
  }
  if (native_window != nullptr && gpu_complete) {
    void* queued_native_buffer = nullptr;
    {
      std::lock_guard<std::mutex> lock(HostWindowSurfaceMutex());
      auto found = HostWindowSurfaces().find(native_surface);
      if (found != HostWindowSurfaces().end()) {
        queued_native_buffer = found->second.native_buffer;
        found->second.native_buffer = nullptr;
      }
    }
    if (queued_native_buffer == nullptr ||
        darwin_art_android_ANativeWindow_queue_hardware_buffer(
            native_window, queued_native_buffer, -1) != 0) {
      return false;
    }

    AHardwareBuffer* next_buffer = nullptr;
    void* next_native_buffer = nullptr;
    int acquire_fence = -1;
    if (darwin_art_android_ANativeWindow_dequeue_hardware_buffer(
            native_window, &next_buffer, &next_native_buffer,
            &acquire_fence) != 0) {
      return false;
    }
    if (acquire_fence >= 0) {
      (void)sync_wait(acquire_fence, -1);
      (void)darwin_art_bionic_socket_broker_close(acquire_fence);
    }
    AHardwareBuffer_Desc description{};
    AHardwareBuffer_describe(next_buffer, &description);
    void* next_iosurface =
        darwin_art_android_hardware_buffer_iosurface(next_buffer);
    EGLint bind_target = native_bind_target;
    const EGLint attributes[] = {
        kEglWidth,
        static_cast<EGLint>(description.width),
        kEglHeight,
        static_cast<EGLint>(description.height),
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
    EGLSurface next_target =
        next_iosurface == nullptr
            ? nullptr
            : api.create_pbuffer_from_client_buffer(
                  native_display, kEglIosurfaceAngle, next_iosurface,
                  native_config, attributes);
    if (next_target == nullptr) {
      (void)darwin_art_android_ANativeWindow_cancel_hardware_buffer(
          native_window, next_native_buffer, -1);
      return false;
    }
    {
      std::lock_guard<std::mutex> lock(HostWindowSurfaceMutex());
      auto found = HostWindowSurfaces().find(native_surface);
      if (found == HostWindowSurfaces().end()) {
        api.destroy_surface(native_display, next_target);
        (void)darwin_art_android_ANativeWindow_cancel_hardware_buffer(
            native_window, next_native_buffer, -1);
        return false;
      }
      HostWindowSurface& current = found->second;
      if (current.target_bound) {
        std::int32_t previous_active_texture = 0;
        std::int32_t previous_texture = 0;
        std::int32_t previous_read_framebuffer = 0;
        std::int32_t previous_draw_framebuffer = 0;
        api.gl_get_integer_v(0x84E0, &previous_active_texture);  // GL_ACTIVE_TEXTURE
        api.gl_get_integer_v(current.texture_target == kGlTexture2d ? 0x8069
                                                                    : 0x84F6,
                             &previous_texture);
        api.gl_get_integer_v(0x8CAA,
                             &previous_read_framebuffer);  // GL_READ_FRAMEBUFFER_BINDING
        api.gl_get_integer_v(0x8CA6,
                             &previous_draw_framebuffer);  // GL_DRAW_FRAMEBUFFER_BINDING
        api.gl_bind_texture(current.texture_target, current.texture);
        api.release_tex_image(native_display, current.iosurface_target,
                              kEglBackBuffer);
        api.gl_delete_framebuffers(1, &current.framebuffer);
        api.gl_delete_textures(1, &current.texture);
        api.gl_bind_framebuffer(0x8CA8,
                                previous_read_framebuffer);  // GL_READ_FRAMEBUFFER
        api.gl_bind_framebuffer(kGlDrawFramebuffer, previous_draw_framebuffer);
        api.gl_bind_texture(current.texture_target,
                            static_cast<std::uint32_t>(previous_texture));
        api.gl_active_texture(static_cast<std::uint32_t>(previous_active_texture));
      }
      api.destroy_surface(native_display, current.iosurface_target);
      current.native_buffer = next_native_buffer;
      current.iosurface = next_iosurface;
      current.iosurface_target = next_target;
      current.texture = 0;
      current.framebuffer = 0;
      current.width = description.width;
      current.height = description.height;
      current.render_width = description.width;
      current.render_height = description.height;
      current.target_bound = false;
      current.owns_iosurface_ref = false;
    }
  }
  return true;
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
  return api.ready ? api.get_error() : kEglNotInitialized;
}
jboolean EglReleaseThread(JNIEnv*, jobject) {
  auto& api = GetAngleApi();
  return api.ready && api.release_thread() ? JNI_TRUE : JNI_FALSE;
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

extern "C" void* darwin_art_android_eglCreateWindowSurface(
    void* display, void* config, void* native_window, const int32_t*) {
  void* surface = CreateHostWindowSurface(display, config, native_window);
  if (std::getenv("DARWIN_ART_DEBUG_GRAPHICS_DSO") != nullptr) {
    std::cerr << "ART Android EGL: eglCreateWindowSurface window="
              << native_window << " result=" << surface << " error=0x"
              << std::hex << (surface == nullptr ? GetAngleApi().get_error() : 0)
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
      api.ready && api.make_current(display, draw, read, context) ? 1u : 0u;
  if (std::getenv("DARWIN_ART_DEBUG_GRAPHICS_DSO") != nullptr) {
    std::cerr << "ART Android EGL: eglMakeCurrent draw=" << draw
              << " read=" << read << " context=" << context
              << " result=" << result << "\n";
  }
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
  g_host_surface_width.store(width, std::memory_order_release);
  g_host_surface_height.store(height, std::memory_order_release);
  DarwinArtSurface* surface = darwin_art_surface_active_gpu();
  if (surface == nullptr) return;
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
  auto& api = GetAngleApi();
  EGLBoolean result = api.initialize(display, major, minor);
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
  EGLContext result = api.create_context(display, config, share, attributes);
  if (DebugGraphicsDso() || result == nullptr) {
    std::cerr << "ART Android EGL: eglCreateContext result=" << result
              << " error=0x" << std::hex
              << (result == nullptr && api.get_error != nullptr
                      ? api.get_error()
                      : 0)
              << std::dec
              << " attributes=";
    if (attributes == nullptr) {
      std::cerr << "<null>";
    } else {
      for (std::size_t index = 0; index < 64; index += 2) {
        const EGLint name = attributes[index];
        std::cerr << (index == 0 ? "[" : ",") << "0x" << std::hex
                  << name << std::dec;
        if (name == kEglNone) {
          std::cerr << "]";
          break;
        }
        std::cerr << "=" << attributes[index + 1];
      }
    }
    std::cerr << "\n";
  }
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
  EGLBoolean result = api.make_current(display, draw, read, context);
  if (DebugGraphicsDso()) {
    std::cerr << "ART Android EGL: eglMakeCurrent result=" << result << "\n";
  }
  return result;
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

std::uint32_t GuestTextureForHostTexture(std::uint32_t host_texture);

void GlGetIntegervHost(std::uint32_t name, std::int32_t* value) {
  auto& api = GetAngleApi();
  api.gl_get_integer_v(name, value);
  if (value != nullptr &&
      (name == 0x8069 ||  // GL_TEXTURE_BINDING_2D
       name == 0x8C1D ||  // GL_TEXTURE_BINDING_2D_ARRAY
       name == 0x8D67)) { // GL_TEXTURE_BINDING_EXTERNAL_OES
    *value = static_cast<std::int32_t>(
        GuestTextureForHostTexture(static_cast<std::uint32_t>(*value)));
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

std::mutex& NativeFenceSyncMutex() {
  static std::mutex mutex;
  return mutex;
}

struct DarwinNativeFenceSync {
  EGLDisplay display = nullptr;
  EGLSync angle_sync = nullptr;
  void* metal_shared_event = nullptr;
  std::uint64_t signal_value = 0;
};

std::unordered_map<EGLSync, std::unique_ptr<DarwinNativeFenceSync>>&
NativeFenceSyncs() {
  static std::unordered_map<EGLSync, std::unique_ptr<DarwinNativeFenceSync>>
      syncs;
  return syncs;
}

void SynchronizeAhbImagesToIosurface();
void SynchronizeIosurfaceToAhbClientTextures();

EGLint NativeFenceAttributeFd(const EGLint* attributes) {
  if (attributes == nullptr) return kEglNoNativeFenceFdAndroid;
  for (const EGLint* attribute = attributes; attribute[0] != kEglNone;
       attribute += 2) {
    if (attribute[0] == kEglSyncNativeFenceFdAndroid) return attribute[1];
  }
  return kEglNoNativeFenceFdAndroid;
}

EGLSync EglCreateSyncKhrAndroid(EGLDisplay display, EGLenum type,
                                const EGLint* attributes) {
  auto& api = GetAngleApi();
  using Function = EGLSync (*)(EGLDisplay, EGLenum, const EGLint*);
  auto function = LoadSymbol<Function>(GetAngleApi().egl_library,
                                       "eglCreateSyncKHR");
  const bool native_fence = type == kEglSyncNativeFenceAndroid;
  const EGLint imported_fence_fd =
      native_fence ? NativeFenceAttributeFd(attributes)
                   : kEglNoNativeFenceFdAndroid;
  const bool acquire_fence = imported_fence_fd >= 0;
  EGLSync sync = nullptr;
  if (native_fence) {
    if (acquire_fence) {
      // Ownership of EGL_SYNC_NATIVE_FENCE_FD_ANDROID transfers to EGL. The
      // broker descriptor is signaled only after the remote Metal producer
      // completed, so refresh consumer compatibility textures from IOSurface.
      (void)sync_wait(imported_fence_fd, -1);
      SynchronizeIosurfaceToAhbClientTextures();
      (void)darwin_art_bionic_socket_broker_close(imported_fence_fd);
    } else {
      // A producer fence publishes Android GL_TEXTURE_2D staging storage into
      // the AHardwareBuffer's IOSurface before exporting the completion FD.
      SynchronizeAhbImagesToIosurface();
    }
    auto owned = std::make_unique<DarwinNativeFenceSync>();
    owned->display = display;
    if (acquire_fence) {
      // The imported broker fence has already established the cross-process
      // completion boundary. Preserve EGLSync query/wait behavior with a
      // native ANGLE fence for subsequent commands in this context.
      owned->angle_sync =
          function == nullptr ? nullptr : function(display, 0x30F9, nullptr);
    } else {
      using QueryDisplayAttrib = EGLBoolean (*)(EGLDisplay, EGLint,
                                                 EGLAttrib*);
      using QueryDeviceAttrib = EGLBoolean (*)(void*, EGLint, EGLAttrib*);
      using CreateSync = EGLSync (*)(EGLDisplay, EGLenum, const EGLAttrib*);
      auto query_display_attrib = reinterpret_cast<QueryDisplayAttrib>(
          api.get_proc_address("eglQueryDisplayAttribEXT"));
      auto query_device_attrib = reinterpret_cast<QueryDeviceAttrib>(
          api.get_proc_address("eglQueryDeviceAttribEXT"));
      auto create_sync = reinterpret_cast<CreateSync>(
          api.get_proc_address("eglCreateSync"));
      EGLAttrib egl_device = 0;
      EGLAttrib metal_device = 0;
      if (query_display_attrib != nullptr && query_device_attrib != nullptr &&
          create_sync != nullptr &&
          query_display_attrib(display, kEglDeviceExt, &egl_device) != 0 &&
          query_device_attrib(reinterpret_cast<void*>(egl_device),
                              kEglMetalDeviceAngle, &metal_device) != 0) {
        owned->metal_shared_event =
            darwin_art_android_metal_shared_event_create(
                reinterpret_cast<void*>(metal_device), &owned->signal_value);
        if (owned->metal_shared_event != nullptr) {
          const EGLAttrib event_value = reinterpret_cast<EGLAttrib>(
              owned->metal_shared_event);
          const EGLAttrib attributes[] = {
              kEglSyncMetalSharedEventObjectAngle, event_value,
              kEglSyncMetalSharedEventSignalValueLoAngle,
              static_cast<EGLAttrib>(owned->signal_value & UINT32_MAX),
              kEglSyncMetalSharedEventSignalValueHiAngle,
              static_cast<EGLAttrib>(owned->signal_value >> 32), kEglNone};
          owned->angle_sync = create_sync(
              display, kEglSyncMetalSharedEventAngle, attributes);
        }
      }
    }
    if (owned->angle_sync == nullptr) {
      if (owned->metal_shared_event != nullptr) {
        darwin_art_android_metal_shared_event_release(
            owned->metal_shared_event);
      }
      return nullptr;
    }
    sync = owned.get();
    std::lock_guard<std::mutex> lock(NativeFenceSyncMutex());
    NativeFenceSyncs().emplace(sync, std::move(owned));
  } else if (function != nullptr) {
    sync = function(display, type, attributes);
  }
  if (DebugGraphicsDso()) {
    std::cerr << "ART Android EGL: eglCreateSyncKHR pid=" << getpid()
              << " type=0x" << std::hex << type << std::dec
              << " translated=" << native_fence
              << " acquire=" << acquire_fence
              << " imported_fd=" << imported_fence_fd << " sync=" << sync
              << " angle_sync="
              << (native_fence && sync != nullptr
                      ? static_cast<DarwinNativeFenceSync*>(sync)->angle_sync
                      : nullptr)
              << " shared_event="
              << (native_fence && sync != nullptr
                      ? static_cast<DarwinNativeFenceSync*>(sync)
                            ->metal_shared_event
                      : nullptr)
              << "\n";
  }
  return sync;
}

EGLBoolean EglDestroySyncKhrAndroid(EGLDisplay display, EGLSync sync) {
  std::unique_ptr<DarwinNativeFenceSync> owned;
  {
    std::lock_guard<std::mutex> lock(NativeFenceSyncMutex());
    auto found = NativeFenceSyncs().find(sync);
    if (found != NativeFenceSyncs().end()) {
      owned = std::move(found->second);
      NativeFenceSyncs().erase(found);
    }
  }
  if (owned != nullptr) {
    using Destroy = EGLBoolean (*)(EGLDisplay, EGLSync);
    auto destroy = reinterpret_cast<Destroy>(
        GetAngleApi().get_proc_address("eglDestroySync"));
    if (destroy != nullptr && owned->angle_sync != nullptr)
      (void)destroy(display, owned->angle_sync);
    if (owned->metal_shared_event != nullptr)
      darwin_art_android_metal_shared_event_release(
          owned->metal_shared_event);
    if (DebugGraphicsDso()) {
      std::cerr << "ART Android EGL: eglDestroySyncKHR pid=" << getpid()
                << " sync=" << sync << " translated=1\n";
    }
    return 1;
  }
  using Function = EGLBoolean (*)(EGLDisplay, EGLSync);
  auto function = LoadSymbol<Function>(GetAngleApi().egl_library,
                                       "eglDestroySyncKHR");
  const EGLBoolean result =
      function == nullptr ? 0 : function(display, sync);
  if (DebugGraphicsDso()) {
    std::cerr << "ART Android EGL: eglDestroySyncKHR pid=" << getpid()
              << " sync=" << sync << " translated=0 result=" << result
              << "\n";
  }
  return result;
}

EGLint EglClientWaitSyncKhrAndroid(EGLDisplay display, EGLSync sync,
                                   EGLint flags, std::uint64_t timeout) {
  {
    std::lock_guard<std::mutex> lock(NativeFenceSyncMutex());
    auto found = NativeFenceSyncs().find(sync);
    if (found != NativeFenceSyncs().end()) {
      using Wait = EGLint (*)(EGLDisplay, EGLSync, EGLint, std::uint64_t);
      auto wait = reinterpret_cast<Wait>(
          GetAngleApi().get_proc_address("eglClientWaitSync"));
      return wait == nullptr
                 ? 0
                 : wait(display, found->second->angle_sync, flags, timeout);
    }
  }
  using Function = EGLint (*)(EGLDisplay, EGLSync, EGLint, std::uint64_t);
  auto function = LoadSymbol<Function>(GetAngleApi().egl_library,
                                       "eglClientWaitSyncKHR");
  return function == nullptr ? 0 : function(display, sync, flags, timeout);
}

EGLBoolean EglWaitSyncKhrAndroid(EGLDisplay display, EGLSync sync,
                                 EGLint flags) {
  {
    std::lock_guard<std::mutex> lock(NativeFenceSyncMutex());
    auto found = NativeFenceSyncs().find(sync);
    if (found != NativeFenceSyncs().end()) {
      using Wait = EGLBoolean (*)(EGLDisplay, EGLSync, EGLint);
      auto wait = reinterpret_cast<Wait>(
          GetAngleApi().get_proc_address("eglWaitSync"));
      return wait == nullptr
                 ? 0
                 : wait(display, found->second->angle_sync, flags);
    }
  }
  using Function = EGLBoolean (*)(EGLDisplay, EGLSync, EGLint);
  auto function = LoadSymbol<Function>(GetAngleApi().egl_library,
                                       "eglWaitSyncKHR");
  return function == nullptr ? 0 : function(display, sync, flags);
}

EGLBoolean EglGetSyncAttribKhrAndroid(EGLDisplay display, EGLSync sync,
                                      EGLint attribute, EGLint* value) {
  {
    std::lock_guard<std::mutex> lock(NativeFenceSyncMutex());
    auto found = NativeFenceSyncs().find(sync);
    if (found != NativeFenceSyncs().end()) {
      using Get = EGLBoolean (*)(EGLDisplay, EGLSync, EGLint, EGLAttrib*);
      auto get = reinterpret_cast<Get>(
          GetAngleApi().get_proc_address("eglGetSyncAttrib"));
      EGLAttrib wide = 0;
      if (value == nullptr || get == nullptr ||
          get(display, found->second->angle_sync, attribute, &wide) == 0)
        return 0;
      *value = static_cast<EGLint>(wide);
      return 1;
    }
  }
  using Function = EGLBoolean (*)(EGLDisplay, EGLSync, EGLint, EGLint*);
  auto function = LoadSymbol<Function>(GetAngleApi().egl_library,
                                       "eglGetSyncAttribKHR");
  return function == nullptr ? 0 : function(display, sync, attribute, value);
}

EGLSync EglCreateSyncAndroid(EGLDisplay display, EGLenum type,
                             const EGLAttrib* attributes) {
  if (attributes != nullptr &&
      attributes[0] == kEglSyncNativeFenceFdAndroid) {
    const EGLint narrow[] = {
        kEglSyncNativeFenceFdAndroid, static_cast<EGLint>(attributes[1]),
        kEglNone};
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
  {
    std::lock_guard<std::mutex> lock(NativeFenceSyncMutex());
    auto found = NativeFenceSyncs().find(sync);
    if (found != NativeFenceSyncs().end()) {
      DarwinNativeFenceSync& translated = *found->second;
      const EGLint result =
          translated.metal_shared_event == nullptr
              ? kEglNoNativeFenceFdAndroid
              : darwin_art_android_metal_shared_event_fence_fd(
                    translated.metal_shared_event, translated.signal_value);
      if (DebugGraphicsDso()) {
        const bool initially_signaled = result >= 0 && sync_wait(result, 0) == 0;
        std::cerr << "ART Android EGL: eglDupNativeFenceFDANDROID pid="
                  << getpid() << " sync=" << sync
                  << " translated=1 shared_event="
                  << translated.metal_shared_event << " value="
                  << translated.signal_value << " result=" << result
                  << " initially_signaled=" << initially_signaled
                  << "\n";
      }
      return result;
    }
  }
  using Function = EGLint (*)(EGLDisplay, EGLSync);
  auto function = LoadSymbol<Function>(GetAngleApi().egl_library,
                                       "eglDupNativeFenceFDANDROID");
  const EGLint result = function == nullptr
                            ? kEglNoNativeFenceFdAndroid
                            : function(display, sync);
  if (DebugGraphicsDso()) {
    std::cerr << "ART Android EGL: eglDupNativeFenceFDANDROID pid=" << getpid()
              << " sync=" << sync << " translated=0 result=" << result
              << "\n";
  }
  return result;
}

struct DarwinAhbEglImage {
  EGLDisplay display = nullptr;
  EGLContext owner_context = nullptr;
  EGLSurface owner_draw_surface = nullptr;
  EGLSurface owner_read_surface = nullptr;
  EGLSurface pbuffer = nullptr;
  AHardwareBuffer* buffer = nullptr;
  // Newer ANGLE versions can expose a Metal texture as an EGLImage. The Metal
  // texture and AHardwareBuffer wrap the same IOSurface, matching Android's
  // EGL_NATIVE_BUFFER_ANDROID storage identity without an intermediate copy.
  EGLImage metal_image = nullptr;
  void* metal_texture = nullptr;
  EGLint bind_target = 0;
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
  // binds the EGLImage, so track the contents independently from the texture
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

std::atomic<std::uint64_t>& NextAhbTextureAssociation() {
  static std::atomic<std::uint64_t> generation{1};
  return generation;
}

struct DarwinSurfaceControlTarget {
  uint32_t surface_id = 0;
  EGLDisplay display = nullptr;
  EGLSurface pbuffer = nullptr;
  EGLImage metal_image = nullptr;
  void* metal_texture = nullptr;
  void* iosurface = nullptr;
  void* metal_device = nullptr;
  EGLint bind_target = 0;
  std::uint32_t texture_target = 0;
  std::uint32_t texture = 0;
  std::uint32_t framebuffer = 0;
  std::uint32_t composite_program = 0;
  std::int32_t composite_source_uniform = -1;
  std::int32_t composite_texture_uniform = -1;
  std::int32_t composite_alpha_uniform = -1;
  bool composite_program_attempted = false;
  std::uint32_t composite_2d_program = 0;
  std::int32_t composite_2d_source_uniform = -1;
  std::int32_t composite_2d_texture_uniform = -1;
  std::int32_t composite_2d_alpha_uniform = -1;
  bool composite_2d_program_attempted = false;
  std::uint32_t width = 0;
  std::uint32_t height = 0;
  bool bound = false;
  bool has_content = false;
};

thread_local DarwinSurfaceControlTarget g_surface_control_target;
thread_local std::vector<DarwinArtMetalComposerLayer>
    g_metal_composer_layers;
thread_local std::uint64_t g_metal_composer_transaction_id = 0;

struct DarwinSurfaceControlContextScope {
  EGLDisplay activated_display = nullptr;
  EGLDisplay previous_display = nullptr;
  EGLContext previous_context = nullptr;
  EGLSurface previous_draw_surface = nullptr;
  EGLSurface previous_read_surface = nullptr;
  bool switched = false;
};

thread_local DarwinSurfaceControlContextScope g_surface_control_context_scope;

// Restoring the producer context is deliberately separate from composition
// completion.  In particular, begin_hardware_buffer_composition holds
// AhbEglImageMutex while it imports the target; calling the normal end path on
// an import failure would create a fence and take that mutex recursively.
void RestoreSurfaceControlContextScope() {
  auto& scope = g_surface_control_context_scope;
  if (!scope.switched) {
    scope = {};
    return;
  }
  const EGLDisplay restore_display =
      scope.previous_display == nullptr ? scope.activated_display
                                        : scope.previous_display;
  auto& api = GetAngleApi();
  if (api.make_current != nullptr) {
    (void)api.make_current(restore_display, scope.previous_draw_surface,
                           scope.previous_read_surface,
                           scope.previous_context);
  }
  scope = {};
}

std::mutex& AhbEglImageMutex() {
  static std::mutex mutex;
  return mutex;
}

std::unordered_map<EGLImage, std::unique_ptr<DarwinAhbEglImage>>&
AhbEglImages() {
  static std::unordered_map<EGLImage, std::unique_ptr<DarwinAhbEglImage>>
      images;
  return images;
}

std::uint32_t GuestTextureForHostTextureLocked(EGLContext context,
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

std::uint32_t HostTextureForGuestTextureLocked(EGLContext context,
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

std::uint32_t GuestTextureForHostTexture(std::uint32_t host_texture) {
  auto& api = GetAngleApi();
  const EGLContext current =
      api.get_current_context == nullptr ? nullptr : api.get_current_context();
  std::lock_guard<std::mutex> lock(AhbEglImageMutex());
  return GuestTextureForHostTextureLocked(current, host_texture);
}

std::unordered_map<EGLContext, AHardwareBuffer*>&
LastPresentedAhbByContext() {
  static std::unordered_map<EGLContext, AHardwareBuffer*> buffers;
  return buffers;
}

std::unordered_map<EGLContext, std::uint64_t>&
PresentedGenerationByContext() {
  static std::unordered_map<EGLContext, std::uint64_t> generations;
  return generations;
}

std::unordered_map<AHardwareBuffer*, void*>& QueueByAhb() {
  static std::unordered_map<AHardwareBuffer*, void*> queues;
  return queues;
}

std::unordered_map<void*, AHardwareBuffer*>& LastPresentedAhbByQueue() {
  static std::unordered_map<void*, AHardwareBuffer*> buffers;
  return buffers;
}

std::unordered_map<void*, std::uint64_t>& PresentedGenerationByQueue() {
  static std::unordered_map<void*, std::uint64_t> generations;
  return generations;
}

EGLConfig ChooseIosurfaceTextureConfig(EGLDisplay display,
                                        EGLint* bind_target) {
  auto& api = GetAngleApi();
  const EGLint attributes[] = {kEglSurfaceType, kEglPbufferBit,
                               kEglBindToTextureRgba, kEglTrue, kEglNone};
  std::array<EGLConfig, 64> configs{};
  EGLint count = 0;
  if (!api.choose_config(display, attributes, configs.data(), configs.size(),
                         &count)) {
    return nullptr;
  }
  for (EGLint index = 0;
       index < std::min<EGLint>(count, static_cast<EGLint>(configs.size()));
       ++index) {
    EGLint target = 0;
    if (api.get_config_attrib(display, configs[index],
                              kEglBindToTextureTargetAngle, &target) &&
        target == kEglTexture2d) {
      *bind_target = target;
      return configs[index];
    }
  }
  for (EGLint index = 0;
       index < std::min<EGLint>(count, static_cast<EGLint>(configs.size()));
       ++index) {
    EGLint target = 0;
    if (api.get_config_attrib(display, configs[index],
                              kEglBindToTextureTargetAngle, &target) &&
        target == kEglTextureRectangleAngle) {
      *bind_target = target;
      return configs[index];
    }
  }
  return nullptr;
}

bool EnsureSurfaceControlTarget(EGLDisplay display) {
  auto& api = GetAngleApi();
  auto& target = g_surface_control_target;
  if (target.bound && target.display == display) return true;
  const char* encoded = std::getenv("DARWIN_ART_HOST_IOSURFACE_ID");
  if (encoded == nullptr || encoded[0] == '\0') {
    if (DebugGraphicsDso()) {
      std::cerr << "ART Android SurfaceControl: missing host IOSurface id pid="
                << getpid() << "\n";
    }
    return false;
  }
  char* end = nullptr;
  const unsigned long parsed = std::strtoul(encoded, &end, 10);
  if (end == encoded || *end != '\0' || parsed == 0 || parsed > UINT32_MAX) {
    if (DebugGraphicsDso()) {
      std::cerr << "ART Android SurfaceControl: invalid host IOSurface id pid="
                << getpid() << " value=" << encoded << "\n";
    }
    return false;
  }
  void* iosurface = nullptr;
  std::uint32_t width = 0;
  std::uint32_t height = 0;
  if (!darwin_art_surface_gpu_lookup_iosurface(
          static_cast<std::uint32_t>(parsed), &iosurface, &width, &height)) {
    if (DebugGraphicsDso()) {
      std::cerr << "ART Android SurfaceControl: host IOSurface lookup failed "
                   "pid="
                << getpid() << " id=" << parsed << "\n";
    }
    return false;
  }

  // ANGLE's Metal display intentionally does not expose the legacy
  // EGL_IOSURFACE_ANGLE pbuffer configs. Import the host compositor surface
  // through the native Metal device, exactly like Android HardwareBuffers,
  // and attach that EGLImage to an FBO. Both the Chromium source and the host
  // destination then stay IOSurface-backed GPU resources throughout.
  using QueryDisplayAttrib = EGLBoolean (*)(EGLDisplay, EGLint, EGLAttrib*);
  using QueryDeviceAttrib = EGLBoolean (*)(void*, EGLint, EGLAttrib*);
  using CreateImage = EGLImage (*)(EGLDisplay, EGLContext, EGLenum, void*,
                                   const EGLint*);
  using DestroyImage = EGLBoolean (*)(EGLDisplay, EGLImage);
  using ImageTargetTexture2d = void (*)(std::uint32_t, void*);
  auto query_display_attrib = reinterpret_cast<QueryDisplayAttrib>(
      api.get_proc_address("eglQueryDisplayAttribEXT"));
  auto query_device_attrib = reinterpret_cast<QueryDeviceAttrib>(
      api.get_proc_address("eglQueryDeviceAttribEXT"));
  auto create_image =
      reinterpret_cast<CreateImage>(api.get_proc_address("eglCreateImageKHR"));
  auto destroy_image = reinterpret_cast<DestroyImage>(
      api.get_proc_address("eglDestroyImageKHR"));
  auto image_target_texture = reinterpret_cast<ImageTargetTexture2d>(
      api.get_proc_address("glEGLImageTargetTexture2DOES"));
  EGLAttrib egl_device = 0;
  EGLAttrib metal_device = 0;
  void* metal_texture = nullptr;
  EGLImage metal_image = nullptr;
  if (query_display_attrib != nullptr && query_device_attrib != nullptr &&
      create_image != nullptr && image_target_texture != nullptr &&
      query_display_attrib(display, kEglDeviceExt, &egl_device) != 0 &&
      query_device_attrib(reinterpret_cast<void*>(egl_device),
                          kEglMetalDeviceAngle, &metal_device) != 0) {
    metal_texture = darwin_art_android_iosurface_metal_texture(
        iosurface, width, height, reinterpret_cast<void*>(metal_device));
    if (metal_texture != nullptr) {
      const EGLint image_attributes[] = {kEglNone};
      metal_image = create_image(display, nullptr, kEglMetalTextureAngle,
                                 metal_texture, image_attributes);
    }
  }
  if (metal_image != nullptr) {
    std::int32_t previous_texture = 0;
    std::int32_t previous_framebuffer = 0;
    api.gl_get_integer_v(0x8069, &previous_texture);  // GL_TEXTURE_BINDING_2D
    api.gl_get_integer_v(0x8CA6,
                         &previous_framebuffer);  // GL_DRAW_FRAMEBUFFER_BINDING
    std::uint32_t texture = 0;
    std::uint32_t framebuffer = 0;
    api.gl_gen_textures(1, &texture);
    api.gl_bind_texture(kGlTexture2d, texture);
    image_target_texture(kGlTexture2d, metal_image);
    api.gl_gen_framebuffers(1, &framebuffer);
    api.gl_bind_framebuffer(kGlFramebuffer, framebuffer);
    api.gl_framebuffer_texture_2d(kGlFramebuffer, kGlColorAttachment0,
                                  kGlTexture2d, texture, 0);
    const std::uint32_t framebuffer_status =
        api.gl_check_framebuffer_status(kGlFramebuffer);
    const std::uint32_t gl_error = api.gl_get_error();
    api.gl_bind_framebuffer(
        kGlFramebuffer, static_cast<std::uint32_t>(previous_framebuffer));
    api.gl_bind_texture(kGlTexture2d,
                        static_cast<std::uint32_t>(previous_texture));
    if (framebuffer_status == kGlFramebufferComplete && gl_error == 0) {
      target = DarwinSurfaceControlTarget{
          .surface_id = static_cast<std::uint32_t>(parsed),
          .display = display,
          .metal_image = metal_image,
          .metal_texture = metal_texture,
          .iosurface = iosurface,
          .texture_target = kGlTexture2d,
          .texture = texture,
          .framebuffer = framebuffer,
          .width = width,
          .height = height,
          .bound = true,
      };
      if (DebugGraphicsDso()) {
        std::cerr
            << "ART Android SurfaceControl: imported compositor IOSurface="
            << target.surface_id << " through Metal size=" << width << "x"
            << height << "\n";
      }
      return true;
    }
    if (DebugGraphicsDso()) {
      std::cerr << "ART Android SurfaceControl: host Metal target failed pid="
                << getpid() << " framebuffer_status=0x" << std::hex
                << framebuffer_status << " gl_error=0x" << gl_error
                << std::dec << "\n";
    }
    if (framebuffer != 0) api.gl_delete_framebuffers(1, &framebuffer);
    if (texture != 0) api.gl_delete_textures(1, &texture);
    if (destroy_image != nullptr) destroy_image(display, metal_image);
    darwin_art_android_metal_texture_release(metal_texture);
    metal_image = nullptr;
    metal_texture = nullptr;
  } else if (metal_texture != nullptr) {
    darwin_art_android_metal_texture_release(metal_texture);
    metal_texture = nullptr;
  }

  EGLint bind_target = 0;
  EGLConfig config = ChooseIosurfaceTextureConfig(display, &bind_target);
  const std::uint32_t texture_target =
      bind_target == kEglTextureRectangleAngle
          ? kGlTextureRectangleAngle
          : (bind_target == kEglTexture2d ? kGlTexture2d : 0);
  if (config == nullptr || texture_target == 0) {
    if (DebugGraphicsDso()) {
      std::cerr << "ART Android SurfaceControl: no IOSurface texture config "
                   "pid="
                << getpid() << " display=" << display
                << " bind_target=0x" << std::hex << bind_target << std::dec
                << "\n";
    }
    darwin_art_surface_gpu_release_iosurface(iosurface);
    return false;
  }
  const EGLint attributes[] = {
      kEglWidth, static_cast<EGLint>(width),
      kEglHeight, static_cast<EGLint>(height),
      kEglIosurfacePlaneAngle, 0,
      kEglTextureTarget, bind_target,
      kEglTextureInternalFormatAngle, kGlBgraExt,
      kEglTextureFormat, kEglTextureRgba,
      kEglTextureTypeAngle, kGlUnsignedByte,
      kEglNone,
  };
  EGLSurface pbuffer = api.create_pbuffer_from_client_buffer(
      display, kEglIosurfaceAngle, iosurface, config, attributes);
  if (pbuffer == nullptr) {
    if (DebugGraphicsDso()) {
      std::cerr << "ART Android SurfaceControl: host IOSurface pbuffer failed "
                   "pid="
                << getpid() << " size=" << width << "x" << height
                << " bind_target=0x" << std::hex << bind_target
                << " error=0x" << api.get_error() << std::dec << "\n";
    }
    darwin_art_surface_gpu_release_iosurface(iosurface);
    return false;
  }
  std::int32_t previous_texture = 0;
  api.gl_get_integer_v(texture_target == kGlTextureRectangleAngle ? 0x84F6
                                                                  : 0x8069,
                       &previous_texture);
  std::uint32_t texture = 0;
  std::uint32_t framebuffer = 0;
  api.gl_gen_textures(1, &texture);
  api.gl_bind_texture(texture_target, texture);
  const bool bound =
      api.bind_tex_image(display, pbuffer, kEglBackBuffer) != 0;
  if (bound) {
    api.gl_gen_framebuffers(1, &framebuffer);
    api.gl_bind_framebuffer(kGlFramebuffer, framebuffer);
    api.gl_framebuffer_texture_2d(kGlFramebuffer, kGlColorAttachment0,
                                  texture_target, texture, 0);
  }
  const bool complete =
      bound && api.gl_check_framebuffer_status(kGlFramebuffer) ==
                   kGlFramebufferComplete;
  if (!complete && DebugGraphicsDso()) {
    const std::uint32_t framebuffer_status =
        bound ? api.gl_check_framebuffer_status(kGlFramebuffer) : 0;
    std::cerr << "ART Android SurfaceControl: host IOSurface target failed "
                 "pid="
              << getpid() << " bind_tex_image=" << bound
              << " texture_target=0x" << std::hex << texture_target
              << " framebuffer_status=0x" << framebuffer_status
              << " egl_error=0x" << api.get_error() << std::dec << "\n";
  }
  api.gl_bind_texture(texture_target,
                      static_cast<std::uint32_t>(previous_texture));
  if (!complete) {
    if (framebuffer != 0) api.gl_delete_framebuffers(1, &framebuffer);
    if (texture != 0) api.gl_delete_textures(1, &texture);
    if (bound) api.release_tex_image(display, pbuffer, kEglBackBuffer);
    api.destroy_surface(display, pbuffer);
    darwin_art_surface_gpu_release_iosurface(iosurface);
    return false;
  }
  target = DarwinSurfaceControlTarget{
      .surface_id = static_cast<std::uint32_t>(parsed),
      .display = display,
      .pbuffer = pbuffer,
      .iosurface = iosurface,
      .bind_target = bind_target,
      .texture_target = texture_target,
      .texture = texture,
      .framebuffer = framebuffer,
      .width = width,
      .height = height,
      .bound = true,
  };
  if (DebugGraphicsDso()) {
    std::cerr << "ART Android SurfaceControl: imported compositor IOSurface="
              << target.surface_id << " size=" << width << "x" << height
              << "\n";
  }
  return true;
}

enum class ComposerTargetFailureKind : std::uint8_t {
  kMissingOrInvalidSurfaceId = 0,
  kSurfaceLookup = 1,
  kMetalDeviceQuery = 2,
};

void TraceComposerTargetFailure(ComposerTargetFailureKind kind,
                                EGLDisplay display, std::uint32_t surface_id) {
  // Target setup can be retried for every frame. Keep diagnostics opt-in and
  // bounded independently for each failure class so one bad environment
  // value cannot hide a later lookup or device-query failure.
  if (std::getenv("DARWIN_ART_TRACE_COMPOSER_FAILURES") == nullptr) return;
  static std::atomic<std::uint32_t> emitted[3] = {};
  const std::size_t index = static_cast<std::size_t>(kind);
  if (index >= std::size(emitted) ||
      emitted[index].fetch_add(1, std::memory_order_relaxed) >= 4) {
    return;
  }
  const char* reason = "unknown";
  switch (kind) {
    case ComposerTargetFailureKind::kMissingOrInvalidSurfaceId:
      reason = "missing-or-invalid-iosurface-id";
      break;
    case ComposerTargetFailureKind::kSurfaceLookup:
      reason = "iosurface-lookup-failed";
      break;
    case ComposerTargetFailureKind::kMetalDeviceQuery:
      reason = "metal-device-query-failed";
      break;
  }
  std::cerr << "ART Metal Composer: target setup failed reason=" << reason
            << " display=" << display << " surface_id=" << surface_id
            << "\n";
}

bool EnsureMetalComposerTarget(EGLDisplay display) {
  const char* encoded = std::getenv("DARWIN_ART_HOST_IOSURFACE_ID");
  if (encoded == nullptr || encoded[0] == '\0') {
    TraceComposerTargetFailure(
        ComposerTargetFailureKind::kMissingOrInvalidSurfaceId, display, 0);
    return false;
  }
  char* end = nullptr;
  const unsigned long parsed = std::strtoul(encoded, &end, 10);
  if (end == encoded || *end != '\0' || parsed == 0 || parsed > UINT32_MAX) {
    TraceComposerTargetFailure(
        ComposerTargetFailureKind::kMissingOrInvalidSurfaceId, display, 0);
    return false;
  }
  auto& target = g_surface_control_target;
  if (target.bound && target.display == display &&
      target.surface_id == static_cast<std::uint32_t>(parsed) &&
      target.iosurface != nullptr && target.metal_device != nullptr) {
    return true;
  }
  if (target.iosurface != nullptr)
    darwin_art_surface_gpu_release_iosurface(target.iosurface);
  target = {};
  void* iosurface = nullptr;
  std::uint32_t width = 0;
  std::uint32_t height = 0;
  if (!darwin_art_surface_gpu_lookup_iosurface(
          static_cast<std::uint32_t>(parsed), &iosurface, &width, &height)) {
    TraceComposerTargetFailure(ComposerTargetFailureKind::kSurfaceLookup,
                               display, static_cast<std::uint32_t>(parsed));
    return false;
  }
  using QueryDisplayAttrib = EGLBoolean (*)(EGLDisplay, EGLint, EGLAttrib*);
  using QueryDeviceAttrib = EGLBoolean (*)(void*, EGLint, EGLAttrib*);
  auto& api = GetAngleApi();
  auto query_display_attrib = reinterpret_cast<QueryDisplayAttrib>(
      api.get_proc_address("eglQueryDisplayAttribEXT"));
  auto query_device_attrib = reinterpret_cast<QueryDeviceAttrib>(
      api.get_proc_address("eglQueryDeviceAttribEXT"));
  EGLAttrib egl_device = 0;
  EGLAttrib metal_device = 0;
  if (query_display_attrib == nullptr || query_device_attrib == nullptr ||
      query_display_attrib(display, kEglDeviceExt, &egl_device) == 0 ||
      query_device_attrib(reinterpret_cast<void*>(egl_device),
                          kEglMetalDeviceAngle, &metal_device) == 0 ||
      metal_device == 0) {
    TraceComposerTargetFailure(ComposerTargetFailureKind::kMetalDeviceQuery,
                               display, static_cast<std::uint32_t>(parsed));
    darwin_art_surface_gpu_release_iosurface(iosurface);
    return false;
  }
  target = DarwinSurfaceControlTarget{
      .surface_id = static_cast<std::uint32_t>(parsed),
      .display = display,
      .iosurface = iosurface,
      .metal_device = reinterpret_cast<void*>(metal_device),
      .width = width,
      .height = height,
      .bound = true,
  };
  if (DebugGraphicsDso()) {
    std::cerr << "ART Metal Composer: target IOSurface=" << target.surface_id
              << " size=" << width << "x" << height
              << " device=" << target.metal_device << "\n";
  }
  return true;
}

bool EnsureSurfaceControlCompositeProgram(DarwinSurfaceControlTarget& target,
                                          bool rectangle_source) {
  auto& program = rectangle_source ? target.composite_program
                                   : target.composite_2d_program;
  auto& source_uniform = rectangle_source
                             ? target.composite_source_uniform
                             : target.composite_2d_source_uniform;
  auto& texture_uniform = rectangle_source
                              ? target.composite_texture_uniform
                              : target.composite_2d_texture_uniform;
  auto& alpha_uniform = rectangle_source ? target.composite_alpha_uniform
                                         : target.composite_2d_alpha_uniform;
  auto& attempted = rectangle_source ? target.composite_program_attempted
                                     : target.composite_2d_program_attempted;
  if (program != 0) return true;
  if (attempted) return false;
  attempted = true;
  auto& api = GetAngleApi();
  if (api.gl_create_shader == nullptr || api.gl_shader_source == nullptr ||
      api.gl_compile_shader == nullptr || api.gl_get_shader_iv == nullptr ||
      api.gl_delete_shader == nullptr || api.gl_create_program == nullptr ||
      api.gl_attach_shader == nullptr || api.gl_link_program == nullptr ||
      api.gl_get_program_iv == nullptr || api.gl_delete_program == nullptr ||
      api.gl_get_uniform_location == nullptr) {
    return false;
  }
  constexpr char kVertexShader[] = R"(#version 300 es
precision highp float;
uniform vec4 u_source;
out vec2 v_tex_coord;
void main() {
  vec2 position = gl_VertexID == 0 ? vec2(-1.0, -1.0) :
                  (gl_VertexID == 1 ? vec2(3.0, -1.0) : vec2(-1.0, 3.0));
  vec2 unit_position = position * 0.5 + 0.5;
  v_tex_coord = mix(u_source.xy, u_source.zw, unit_position);
  gl_Position = vec4(position, 0.0, 1.0);
}
)";
  constexpr char kFragmentShader[] = R"(#version 300 es
#extension GL_ARB_texture_rectangle : require
precision highp float;
uniform sampler2DRect u_texture;
uniform float u_alpha;
in vec2 v_tex_coord;
out vec4 o_color;
void main() {
  o_color = texture(u_texture, v_tex_coord) * u_alpha;
}
)";
  constexpr char kFragmentShader2d[] = R"(#version 300 es
precision highp float;
uniform sampler2D u_texture;
uniform float u_alpha;
in vec2 v_tex_coord;
out vec4 o_color;
void main() {
  o_color = texture(u_texture, v_tex_coord) * u_alpha;
}
)";
  auto compile = [&](std::uint32_t type, const char* source) {
    const std::uint32_t shader = api.gl_create_shader(type);
    api.gl_shader_source(shader, 1, &source, nullptr);
    api.gl_compile_shader(shader);
    std::int32_t compiled = 0;
    api.gl_get_shader_iv(shader, 0x8B81, &compiled);  // GL_COMPILE_STATUS
    if (compiled != 0) return shader;
    if (api.gl_get_shader_info_log != nullptr) {
      std::array<char, 2048> log{};
      std::int32_t length = 0;
      api.gl_get_shader_info_log(shader, log.size(), &length, log.data());
      std::cerr << "ART Android SurfaceControl: composite shader failed: "
                << log.data() << "\n";
    }
    api.gl_delete_shader(shader);
    return std::uint32_t{0};
  };
  const std::uint32_t vertex = compile(0x8B31, kVertexShader);
  const std::uint32_t fragment = compile(
      0x8B30, rectangle_source ? kFragmentShader : kFragmentShader2d);
  if (vertex == 0 || fragment == 0) {
    if (vertex != 0) api.gl_delete_shader(vertex);
    if (fragment != 0) api.gl_delete_shader(fragment);
    return false;
  }
  program = api.gl_create_program();
  api.gl_attach_shader(program, vertex);
  api.gl_attach_shader(program, fragment);
  api.gl_link_program(program);
  api.gl_delete_shader(vertex);
  api.gl_delete_shader(fragment);
  std::int32_t linked = 0;
  api.gl_get_program_iv(program, 0x8B82, &linked);  // GL_LINK_STATUS
  if (linked == 0) {
    if (api.gl_get_program_info_log != nullptr) {
      std::array<char, 2048> log{};
      std::int32_t length = 0;
      api.gl_get_program_info_log(program, log.size(), &length, log.data());
      std::cerr << "ART Android SurfaceControl: composite link failed: "
                << log.data() << "\n";
    }
    api.gl_delete_program(program);
    return false;
  }
  source_uniform = api.gl_get_uniform_location(program, "u_source");
  texture_uniform = api.gl_get_uniform_location(program, "u_texture");
  alpha_uniform = api.gl_get_uniform_location(program, "u_alpha");
  return source_uniform >= 0 && texture_uniform >= 0 && alpha_uniform >= 0;
}

bool CompositeSurfaceControlImageLocked(
    DarwinAhbEglImage& image, DarwinSurfaceControlTarget& target,
    std::int32_t source_left, std::int32_t source_top,
    std::int32_t source_right, std::int32_t source_bottom,
    std::int32_t destination_left, std::int32_t destination_top,
    std::int32_t destination_right, std::int32_t destination_bottom,
    float alpha) {
  auto& api = GetAngleApi();
  const bool rectangle_source = image.metal_image == nullptr;
  const std::uint32_t source_target =
      rectangle_source ? kGlTextureRectangleAngle : kGlTexture2d;
  const std::uint32_t source_texture =
      rectangle_source ? image.iosurface_texture
                       : image.client_staging_texture;
  if (source_texture == 0 ||
      !EnsureSurfaceControlCompositeProgram(target, rectangle_source) ||
      api.gl_use_program == nullptr || api.gl_uniform_1i == nullptr ||
      api.gl_uniform_1f == nullptr || api.gl_uniform_4f == nullptr ||
      api.gl_blend_func_separate == nullptr ||
      api.gl_blend_equation_separate == nullptr ||
      api.gl_draw_arrays == nullptr) {
    return false;
  }
  std::int32_t previous_program = 0;
  std::int32_t previous_active_texture = 0;
  std::int32_t previous_source_texture = 0;
  std::int32_t previous_viewport[4]{};
  std::int32_t previous_blend_src_rgb = 0;
  std::int32_t previous_blend_dst_rgb = 0;
  std::int32_t previous_blend_src_alpha = 0;
  std::int32_t previous_blend_dst_alpha = 0;
  std::int32_t previous_blend_equation_rgb = 0;
  std::int32_t previous_blend_equation_alpha = 0;
  std::uint8_t previous_color_mask[4]{1, 1, 1, 1};
  api.gl_get_integer_v(0x8B8D, &previous_program);       // GL_CURRENT_PROGRAM
  api.gl_get_integer_v(0x84E0, &previous_active_texture);  // GL_ACTIVE_TEXTURE
  api.gl_get_integer_v(0x0BA2, previous_viewport);       // GL_VIEWPORT
  api.gl_get_integer_v(0x80C9, &previous_blend_src_rgb);
  api.gl_get_integer_v(0x80C8, &previous_blend_dst_rgb);
  api.gl_get_integer_v(0x80CB, &previous_blend_src_alpha);
  api.gl_get_integer_v(0x80CA, &previous_blend_dst_alpha);
  api.gl_get_integer_v(0x8009, &previous_blend_equation_rgb);
  api.gl_get_integer_v(0x883D, &previous_blend_equation_alpha);
  if (api.gl_get_boolean_v != nullptr)
    api.gl_get_boolean_v(0x0C23, previous_color_mask);  // GL_COLOR_WRITEMASK
  const bool blend_enabled = api.gl_is_enabled(0x0BE2) != 0;
  const bool depth_enabled = api.gl_is_enabled(0x0B71) != 0;
  const bool stencil_enabled = api.gl_is_enabled(0x0B90) != 0;
  const bool cull_enabled = api.gl_is_enabled(0x0B44) != 0;
  const bool rasterizer_discard_enabled = api.gl_is_enabled(0x8C89) != 0;
  api.gl_active_texture(0x84C0);  // GL_TEXTURE0
  api.gl_get_integer_v(rectangle_source ? 0x84F6 : 0x8069,
                       &previous_source_texture);
  api.gl_bind_texture(source_target, source_texture);
  api.gl_tex_parameter_i(source_target, 0x2801, 0x2601);
  api.gl_tex_parameter_i(source_target, 0x2800, 0x2601);
  api.gl_tex_parameter_i(source_target, 0x2802, 0x812F);
  api.gl_tex_parameter_i(source_target, 0x2803, 0x812F);
  api.gl_viewport(destination_left, destination_top,
                  destination_right - destination_left,
                  destination_bottom - destination_top);
  api.gl_disable(0x0B71);  // GL_DEPTH_TEST
  api.gl_disable(0x0B90);  // GL_STENCIL_TEST
  api.gl_disable(0x0B44);  // GL_CULL_FACE
  api.gl_disable(0x8C89);  // GL_RASTERIZER_DISCARD
  if (api.gl_color_mask != nullptr) api.gl_color_mask(1, 1, 1, 1);
  api.gl_enable(0x0BE2);   // GL_BLEND
  api.gl_blend_equation_separate(0x8006, 0x8006);  // GL_FUNC_ADD
  api.gl_blend_func_separate(1, 0x0303, 1, 0x0303);  // premultiplied source-over
  api.gl_use_program(rectangle_source ? target.composite_program
                                      : target.composite_2d_program);
  api.gl_uniform_1i(rectangle_source ? target.composite_texture_uniform
                                     : target.composite_2d_texture_uniform,
                    0);
  api.gl_uniform_1f(rectangle_source ? target.composite_alpha_uniform
                                     : target.composite_2d_alpha_uniform,
                    std::clamp(alpha, 0.0f, 1.0f));
  const float coordinate_scale_x =
      rectangle_source ? 1.0f : 1.0f / static_cast<float>(image.width);
  const float coordinate_scale_y =
      rectangle_source ? 1.0f : 1.0f / static_cast<float>(image.height);
  api.gl_uniform_4f(
      rectangle_source ? target.composite_source_uniform
                       : target.composite_2d_source_uniform,
      static_cast<float>(source_left) * coordinate_scale_x,
      static_cast<float>(image.height - source_bottom) * coordinate_scale_y,
      static_cast<float>(source_right) * coordinate_scale_x,
      static_cast<float>(image.height - source_top) * coordinate_scale_y);
  (void)api.gl_get_error();
  api.gl_draw_arrays(0x0004, 0, 3);  // GL_TRIANGLES
  const std::uint32_t draw_error = api.gl_get_error();
  if (draw_error != 0) {
    std::cerr << "ART Android SurfaceControl: composite draw error=0x"
              << std::hex << draw_error << std::dec << "\n";
  }
  api.gl_use_program(static_cast<std::uint32_t>(previous_program));
  api.gl_blend_equation_separate(
      static_cast<std::uint32_t>(previous_blend_equation_rgb),
      static_cast<std::uint32_t>(previous_blend_equation_alpha));
  api.gl_blend_func_separate(
      static_cast<std::uint32_t>(previous_blend_src_rgb),
      static_cast<std::uint32_t>(previous_blend_dst_rgb),
      static_cast<std::uint32_t>(previous_blend_src_alpha),
      static_cast<std::uint32_t>(previous_blend_dst_alpha));
  if (!blend_enabled) api.gl_disable(0x0BE2);
  if (depth_enabled) api.gl_enable(0x0B71);
  if (stencil_enabled) api.gl_enable(0x0B90);
  if (cull_enabled) api.gl_enable(0x0B44);
  if (rasterizer_discard_enabled) api.gl_enable(0x8C89);
  if (api.gl_color_mask != nullptr) {
    api.gl_color_mask(previous_color_mask[0], previous_color_mask[1],
                      previous_color_mask[2], previous_color_mask[3]);
  }
  api.gl_viewport(previous_viewport[0], previous_viewport[1],
                  previous_viewport[2], previous_viewport[3]);
  api.gl_bind_texture(source_target,
                      static_cast<std::uint32_t>(previous_source_texture));
  api.gl_active_texture(static_cast<std::uint32_t>(previous_active_texture));
  return draw_error == 0;
}

void* EglGetNativeClientBufferAndroid(AHardwareBuffer* buffer) {
  if (DebugGraphicsDso()) {
    std::cerr << "ART Android EGL: eglGetNativeClientBufferANDROID buffer="
              << buffer << "\n";
  }
  // bionic's AHardwareBuffer is the owning GraphicBuffer object, while EGL's
  // native client buffer is its embedded ANativeWindowBuffer view.  Android's
  // conversion helper advances by two pointer-sized fields on arm64.  Keep
  // that public ABI here; EglCreateImageAndroid resolves the alias back to the
  // owning object before importing its IOSurface.
  return buffer == nullptr ? nullptr
                           : reinterpret_cast<char*>(buffer) + 0x10;
}

EGLImage EglCreateImageAndroid(EGLDisplay display, EGLContext context,
                               EGLenum target, void* client_buffer,
                               const EGLint* attributes) {
  auto& api = GetAngleApi();
  // EGLImage names are display-local in ANGLE. Android's loader can hand this
  // bridge the public EGLDisplay while Chromium's passthrough decoder is
  // current on the corresponding host display. Always create an imported AHB
  // image on that current host display when one exists; otherwise a small
  // image ID can alias an unrelated (often YUV) image in the decoder's display.
  const EGLDisplay current_display =
      api.get_current_display == nullptr ? nullptr : api.get_current_display();
  const EGLDisplay image_display =
      current_display == nullptr ? display : current_display;
  if (DebugGraphicsDso()) {
    std::cerr << "ART Android EGL: eglCreateImageKHR target=0x" << std::hex
              << target << std::dec << " client=" << client_buffer
              << " display=" << display << " current=" << current_display
              << " image_display=" << image_display << "\n";
  }
  if (target != kEglNativeBufferAndroid) {
    using Function = EGLImage (*)(EGLDisplay, EGLContext, EGLenum, void*,
                                  const EGLint*);
    auto function = reinterpret_cast<Function>(
        api.get_proc_address("eglCreateImageKHR"));
    return function == nullptr
               ? nullptr
               : function(display, context, target, client_buffer, attributes);
  }
  auto* buffer =
      darwin_art_android_hardware_buffer_from_client_buffer(client_buffer);
  AHardwareBuffer_Desc description{};
  AHardwareBuffer_describe(buffer, &description);
  void* iosurface = darwin_art_android_hardware_buffer_iosurface(buffer);
  if (iosurface == nullptr || description.width == 0 ||
      description.height == 0) {
    if (DebugGraphicsDso()) {
      std::cerr << "ART Android EGL: invalid AHardwareBuffer client=" << buffer
                << " iosurface=" << iosurface << " size="
                << description.width << "x" << description.height
                << " format=" << description.format << "\n";
    }
    return nullptr;
  }
  using QueryDisplayAttrib = EGLBoolean (*)(EGLDisplay, EGLint, EGLAttrib*);
  using QueryDeviceAttrib = EGLBoolean (*)(void*, EGLint, EGLAttrib*);
  using CreateImage = EGLImage (*)(EGLDisplay, EGLContext, EGLenum, void*,
                                   const EGLint*);
  auto query_display_attrib = reinterpret_cast<QueryDisplayAttrib>(
      api.get_proc_address("eglQueryDisplayAttribEXT"));
  auto query_device_attrib = reinterpret_cast<QueryDeviceAttrib>(
      api.get_proc_address("eglQueryDeviceAttribEXT"));
  auto create_image =
      reinterpret_cast<CreateImage>(api.get_proc_address("eglCreateImageKHR"));
  EGLAttrib egl_device = 0;
  EGLAttrib metal_device = 0;
  void* metal_texture = nullptr;
  EGLImage metal_image = nullptr;
  if (query_display_attrib != nullptr && query_device_attrib != nullptr &&
      create_image != nullptr &&
      query_display_attrib(image_display, kEglDeviceExt, &egl_device) != 0 &&
      query_device_attrib(reinterpret_cast<void*>(egl_device),
                          kEglMetalDeviceAngle, &metal_device) != 0) {
    metal_texture = darwin_art_android_hardware_buffer_metal_texture(
        buffer, reinterpret_cast<void*>(metal_device));
    if (metal_texture != nullptr) {
      const EGLint image_attributes[] = {kEglNone};
      metal_image = create_image(image_display, nullptr, kEglMetalTextureAngle,
                                 metal_texture, image_attributes);
      if (metal_image == nullptr) {
        darwin_art_android_metal_texture_release(metal_texture);
        metal_texture = nullptr;
      }
    }
  }
  if (metal_image != nullptr) {
    AHardwareBuffer_acquire(buffer);
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
    // therefore lets a direct glEGLImageTargetTexture2DOES call hand ANGLE a
    // non-EGL pointer (which ANGLE can misclassify as a YUV image). Key the
    // compatibility metadata by, and return, the actual Metal EGLImage. Calls
    // that pass through our wrapper still recover the Android buffer state,
    // while direct GLES calls now receive a valid native image as required by
    // EGL's opaque-handle contract.
    EGLImage handle = metal_image;
    {
      std::lock_guard<std::mutex> lock(AhbEglImageMutex());
      AhbEglImages().emplace(handle, std::move(image));
    }
    if (DebugGraphicsDso()) {
      std::cerr << "ART Android EGL: AHardwareBuffer Metal EGLImage=" << handle
                << " native=" << metal_image << " size=" << description.width
                << "x" << description.height << "\n";
    }
    return handle;
  }
  EGLint bind_target = 0;
  EGLConfig config = ChooseIosurfaceTextureConfig(image_display, &bind_target);
  if (config == nullptr) {
    if (DebugGraphicsDso())
      std::cerr << "ART Android EGL: no IOSurface texture config\n";
    return nullptr;
  }
  const EGLint iosurface_attributes[] = {
      kEglWidth,
      static_cast<EGLint>(description.width),
      kEglHeight,
      static_cast<EGLint>(description.height),
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
  EGLSurface pbuffer = api.create_pbuffer_from_client_buffer(
      image_display, kEglIosurfaceAngle, iosurface, config,
      iosurface_attributes);
  if (pbuffer == nullptr) {
    if (DebugGraphicsDso()) {
      std::cerr << "ART Android EGL: IOSurface image pbuffer failed size="
                << description.width << "x" << description.height
                << " error=0x" << std::hex << api.get_error() << std::dec
                << "\n";
    }
    return nullptr;
  }
  AHardwareBuffer_acquire(buffer);
  auto image = std::make_unique<DarwinAhbEglImage>();
  image->display = image_display;
  image->pbuffer = pbuffer;
  image->buffer = buffer;
  image->bind_target = bind_target;
  image->width = description.width;
  image->height = description.height;
  image->usage = description.usage;
  EGLImage handle = image.get();
  {
    std::lock_guard<std::mutex> lock(AhbEglImageMutex());
    AhbEglImages().emplace(handle, std::move(image));
  }
  if (DebugGraphicsDso()) {
    std::cerr << "ART Android EGL: AHardwareBuffer IOSurface image=" << handle
              << " size=" << description.width << "x" << description.height
              << "\n";
  }
  return handle;
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
    auto& api = GetAngleApi();
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
      using Function = EGLBoolean (*)(EGLDisplay, EGLImage);
      auto function = reinterpret_cast<Function>(
          api.get_proc_address("eglDestroyImageKHR"));
      if (function != nullptr) function(owned->display, owned->metal_image);
      darwin_art_android_metal_texture_release(owned->metal_texture);
      owned->metal_image = nullptr;
      owned->metal_texture = nullptr;
    }
    if (owned->bound)
      api.release_tex_image(owned->display, owned->pbuffer, kEglBackBuffer);
    if (owned->client_staging_texture != 0)
      api.gl_delete_textures(1, &owned->client_staging_texture);
    if (owned->iosurface_texture != 0)
      api.gl_delete_textures(1, &owned->iosurface_texture);
    const EGLBoolean result =
        owned->pbuffer == nullptr
            ? 1
            : api.destroy_surface(owned->display, owned->pbuffer);
    AHardwareBuffer_release(owned->buffer);
    return result;
  }
  using Function = EGLBoolean (*)(EGLDisplay, EGLImage);
  auto function = reinterpret_cast<Function>(
      GetAngleApi().get_proc_address("eglDestroyImageKHR"));
  return function == nullptr ? 0 : function(display, image);
}

EGLBoolean EglDestroyImageAndroidCore(EGLDisplay display, EGLImage image) {
  bool is_android_image = false;
  {
    std::lock_guard<std::mutex> lock(AhbEglImageMutex());
    is_android_image = AhbEglImages().contains(image);
  }
  if (is_android_image) return EglDestroyImageAndroid(display, image);
  using Function = EGLBoolean (*)(EGLDisplay, EGLImage);
  auto function =
      LoadSymbol<Function>(GetAngleApi().egl_library, "eglDestroyImage");
  return function == nullptr ? 0 : function(display, image);
}

bool CopyIosurfaceToAhbClientTextureLocked(DarwinAhbEglImage& source,
                                           DarwinAhbEglImage& destination);
void RestoreBufferQueueSlotIfNeeded(std::uint32_t texture);

void GlEglImageTargetTexture2dOes(std::uint32_t target, EGLImage image) {
  if (DebugGraphicsDso()) {
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
      auto& api = GetAngleApi();
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
        using Function = void (*)(std::uint32_t, EGLImage);
        auto function = reinterpret_cast<Function>(
            api.get_proc_address("glEGLImageTargetTexture2DOES"));
        const std::uint32_t guest_texture = GuestTextureForHostTextureLocked(
            owned.owner_context, static_cast<std::uint32_t>(client_texture));
        // One Chromium texture name is rebound across rotating BufferQueue
        // slots. Keep a private GL name permanently attached to each slot's
        // Metal EGLImage; otherwise rebinding the guest name destroys access
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
        if (DebugGraphicsDso()) {
          std::cerr << "ART Android EGL: bound Metal AHB image texture="
                    << guest_texture << " staging="
                    << owned.client_staging_texture << " native_image="
                    << owned.metal_image
                    << " success=" << owned.bound << "\n";
        }
        // Chromium commonly attaches the texture name to its draw FBO before
        // replacing that name's storage with the EGLImage.  Consequently the
        // framebuffer wrapper cannot observe the AHB association yet.  Treat
        // the successful image bind as the acquisition boundary as well, and
        // seed a rotating Metal/IOSurface slot before any partial raster.
        const std::uint32_t acquired_texture = guest_texture;
        const bool acquired = owned.bound;
        lock.unlock();
        if (acquired) RestoreBufferQueueSlotIfNeeded(acquired_texture);
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
      if (DebugGraphicsDso()) {
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
  using Function = void (*)(std::uint32_t, EGLImage);
  auto function = reinterpret_cast<Function>(
      GetAngleApi().get_proc_address("glEGLImageTargetTexture2DOES"));
  if (function != nullptr) function(target, image);
}

bool CopyIosurfaceToAhbClientTextureLocked(DarwinAhbEglImage& source,
                                           DarwinAhbEglImage& destination) {
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
  // ANGLE's Metal EGLImage path already gives the guest GL texture the exact
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
  auto& api = GetAngleApi();
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
  } else if (DebugGraphicsDso()) {
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

bool CopyAhbImageToIosurfaceLocked(DarwinAhbEglImage& image) {
  if (!image.bound || image.bind_target != kEglTextureRectangleAngle ||
      image.client_staging_texture == 0 || image.iosurface_texture == 0) {
    return false;
  }
  auto& api = GetAngleApi();
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

void SynchronizeIosurfaceToAhbClientTextures() {
  std::lock_guard<std::mutex> lock(AhbEglImageMutex());
  auto& api = GetAngleApi();
  const EGLContext current =
      api.get_current_context == nullptr ? nullptr : api.get_current_context();
  if (current == nullptr) return;
  std::int32_t bound_client_texture = 0;
  api.gl_get_integer_v(0x8069, &bound_client_texture);  // GL_TEXTURE_BINDING_2D
  if (DebugGraphicsDso()) {
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
    if (DebugGraphicsDso()) {
      std::cerr << " {client=" << image->client_texture << " " << image->width
                << "x" << image->height << " buffer=" << image->buffer
                << " source=" << image->buffer
                << "}";
    }
    if (CopyIosurfaceToAhbClientTextureLocked(*image, *image)) {
      // An imported acquire fence is the authoritative notification that the
      // shared IOSurface contains a newer producer frame, including when this
      // process did not create that frame and has no local generation history.
      ++image->iosurface_content_generation;
      image->staging_content_generation =
          image->iosurface_content_generation;
      image->staging_source_buffer = image->buffer;
    }
  }
  if (DebugGraphicsDso()) std::cerr << "\n";
}

void SynchronizeAhbImagesToIosurface() {
  std::lock_guard<std::mutex> lock(AhbEglImageMutex());
  auto& api = GetAngleApi();
  const EGLContext current =
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
  if (DebugGraphicsDso()) {
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
      } else if (staged_attachment && CopyAhbImageToIosurfaceLocked(*image)) {
        ++image->iosurface_content_generation;
        image->staging_content_generation =
            image->iosurface_content_generation;
        image->staging_source_buffer = image->buffer;
      }
    }
  }
}

extern "C" bool darwin_art_android_begin_hardware_buffer_composition(
    void* opaque, bool clear, std::uint64_t transaction_id) {
  std::lock_guard<std::mutex> lock(AhbEglImageMutex());
  auto& api = GetAngleApi();
  auto* buffer = static_cast<AHardwareBuffer*>(opaque);
  EGLDisplay display = api.get_current_display == nullptr
                           ? nullptr
                           : api.get_current_display();
  EGLContext context = api.get_current_context == nullptr
                           ? nullptr
                           : api.get_current_context();
  g_surface_control_context_scope = {};
  if (display == nullptr || context == nullptr) {
    auto found = std::find_if(
        AhbEglImages().begin(), AhbEglImages().end(),
        [buffer](const auto& entry) { return entry.second->buffer == buffer; });
    if (found == AhbEglImages().end() ||
        found->second->display == nullptr ||
        found->second->owner_context == nullptr) {
      if (DebugGraphicsDso()) {
        std::cerr << "ART Android SurfaceControl: no producer context pid="
                  << getpid() << " buffer=" << buffer
                  << " current_display=" << display
                  << " current_context=" << context << "\n";
      }
      g_metal_composer_layers.clear();
      g_metal_composer_transaction_id = 0;
      return false;
    }
    DarwinAhbEglImage& image = *found->second;
    auto& scope = g_surface_control_context_scope;
    scope.activated_display = image.display;
    scope.previous_display = display;
    scope.previous_context = context;
    scope.previous_draw_surface =
        api.get_current_surface == nullptr ? nullptr
                                           : api.get_current_surface(0x3059);
    scope.previous_read_surface =
        api.get_current_surface == nullptr ? nullptr
                                           : api.get_current_surface(0x305A);
    if (api.make_current(image.display, image.owner_draw_surface,
                         image.owner_read_surface, image.owner_context) == 0) {
      if (DebugGraphicsDso()) {
        std::cerr << "ART Android SurfaceControl: producer context activation "
                     "failed pid="
                  << getpid() << " context=" << image.owner_context
                  << " draw=" << image.owner_draw_surface << " read="
                  << image.owner_read_surface << " error=0x" << std::hex
                  << api.get_error() << std::dec << "\n";
      }
      g_metal_composer_layers.clear();
      g_metal_composer_transaction_id = 0;
      RestoreSurfaceControlContextScope();
      return false;
    }
    scope.switched = true;
    display = image.display;
    context = image.owner_context;
  }
  if (DebugGraphicsDso()) {
    std::cerr << "ART Android SurfaceControl: begin pid=" << getpid()
              << " buffer=" << buffer << " display=" << display
              << " context=" << context << " switched="
              << g_surface_control_context_scope.switched << " clear="
              << clear << "\n";
  }
  if (!EnsureMetalComposerTarget(display)) {
    // Do not call end_hardware_buffer_composition here: begin holds
    // AhbEglImageMutex and end creates a native-fence sync which may acquire
    // it again while publishing AHardwareBuffer contents. This is a setup
    // failure, so no producer or completion fence exists to return.
    g_metal_composer_layers.clear();
    g_metal_composer_transaction_id = 0;
    g_surface_control_target.has_content = false;
    RestoreSurfaceControlContextScope();
    return false;
  }
  g_metal_composer_layers.clear();
  g_metal_composer_transaction_id = transaction_id;

  // The transaction bridge now retains every attached SurfaceControl backing
  // and submits the complete sorted layer tree on each commit. `clear=true`
  // therefore means a full SurfaceFlinger-style recomposition: discard pixels
  // from hidden, detached, or moved layers before drawing the retained tree.
  // Keeping the previous target here left Chromium's old full-screen web
  // surface underneath the tab switcher even after its layer was resized into
  // a thumbnail. Partial producers pass clear=false and retain their target.
  if (!clear && g_surface_control_target.has_content) return true;
  std::int32_t previous_draw_framebuffer = 0;
  const bool scissor_enabled = api.gl_is_enabled(0x0C11) != 0;
  api.gl_get_integer_v(0x8CA6, &previous_draw_framebuffer);
  api.gl_bind_framebuffer(0x8CA9, g_surface_control_target.framebuffer);
  api.gl_disable(0x0C11);  // GL_SCISSOR_TEST
  api.gl_clear_color(0.0f, 0.0f, 0.0f, 0.0f);
  api.gl_clear(0x00004000);  // GL_COLOR_BUFFER_BIT
  api.gl_bind_framebuffer(
      0x8CA9, static_cast<std::uint32_t>(previous_draw_framebuffer));
  if (scissor_enabled) api.gl_enable(0x0C11);
  g_surface_control_target.has_content = true;
  return true;
}

extern "C" int darwin_art_android_end_hardware_buffer_composition() {
  // Publish preceding producer work, then wait for that exact value from the
  // direct Metal HWC command buffer without stalling the CPU.
  auto& api = GetAngleApi();
  const EGLDisplay display = api.get_current_display();
  EGLSync producer_sync =
      display == nullptr
          ? nullptr
          : EglCreateSyncKhrAndroid(display, kEglSyncNativeFenceAndroid,
                                    nullptr);
  void* producer_event = nullptr;
  std::uint64_t producer_value = 0;
  if (producer_sync != nullptr) {
    std::lock_guard<std::mutex> lock(NativeFenceSyncMutex());
    auto found = NativeFenceSyncs().find(producer_sync);
    if (found != NativeFenceSyncs().end()) {
      producer_event = found->second->metal_shared_event;
      producer_value = found->second->signal_value;
    }
  }
  if (producer_event != nullptr) {
    // EGL_ANDROID_native_fence_sync requires the producer to flush after
    // inserting the fence. The central SurfaceFlinger waits on this Metal
    // event before latching the IOSurface; entering its IPC first leaves the
    // signal command buffered in ANGLE and deadlocks the remote composer.
    // glFlush submits GPU work without adding a CPU wait or a pixel copy.
    using Flush = void (*)();
    auto flush = reinterpret_cast<Flush>(api.get_proc_address("glFlush"));
    if (flush != nullptr) flush();
  }
  void* completion_event = nullptr;
  std::uint64_t completion_value = 0;
  const auto& target = g_surface_control_target;
  const char* service_socket =
      std::getenv("DARWIN_ART_SURFACEFLINGER_SOCKET");
  const bool remote = service_socket != nullptr && service_socket[0] != '\0';
  const char* app_package = std::getenv("DARWIN_ART_APK_APP_PACKAGE");
  const bool application_runtime = app_package != nullptr && app_package[0] != '\0';
  bool composed = false;
  int completion_fence = kEglNoNativeFenceFdAndroid;
  if (remote && producer_event != nullptr && target.bound) {
    const jint configured_width = DarwinAngleHostSurfaceWidth();
    const jint configured_height = DarwinAngleHostSurfaceHeight();
    const uint32_t logical_width =
        configured_width > 0 ? static_cast<uint32_t>(configured_width)
                             : target.width;
    const uint32_t logical_height =
        configured_height > 0 ? static_cast<uint32_t>(configured_height)
                              : target.height;
    completion_fence = darwin_art_surfaceflinger_service_present(
        target.surface_id, logical_width, logical_height,
        g_metal_composer_transaction_id, g_metal_composer_layers.data(),
        g_metal_composer_layers.size(), producer_event, producer_value);
    composed = completion_fence >= 0;
  } else if (!application_runtime && !remote &&
             !g_metal_composer_layers.empty() &&
             producer_event != nullptr && target.bound &&
             target.iosurface != nullptr && target.metal_device != nullptr) {
    composed = darwin_art_metal_composer_compose(
        target.metal_device, target.iosurface, target.width, target.height,
        g_metal_composer_layers.data(), g_metal_composer_layers.size(),
        producer_event, producer_value, &completion_event,
        &completion_value);
    completion_fence =
        !composed || completion_event == nullptr
            ? kEglNoNativeFenceFdAndroid
            : darwin_art_android_metal_shared_event_fence_fd(
                  completion_event, completion_value);
  }
  if (completion_event != nullptr)
    darwin_art_android_metal_shared_event_release(completion_event);
  if (composed && completion_fence >= 0) {
    // Keep the Android-facing fence ownership intact while a duplicate feeds
    // the host surface's readiness monitor. AppKit scanout must not sample
    // the target IOSurface until this exact SurfaceFlinger composition has
    // signaled, even when the next display tick arrives earlier.
    DarwinArtSurface* host = darwin_art_surface_active_gpu();
    const int monitor_fence =
        host == nullptr ? -1
                        : darwin_art_bionic_socket_broker_dup(completion_fence);
    if (host != nullptr && monitor_fence >= 0) {
      if (!darwin_art_surface_gpu_track_composition_fence(host,
                                                           monitor_fence) &&
          std::getenv("DARWIN_ART_DEBUG_SURFACE_TRANSACTIONS") != nullptr) {
        std::cerr << "ART SurfaceFlinger: readiness monitor rejected fence="
                  << monitor_fence << "\n";
      }
    }
  }
  if (producer_sync != nullptr)
    (void)EglDestroySyncKhrAndroid(display, producer_sync);
  if (DebugGraphicsDso()) {
    std::cerr << "ART Metal Composer: submit layers="
              << g_metal_composer_layers.size() << " composed=" << composed
              << " remote=" << remote
              << " producer_value=" << producer_value
              << " completion_value=" << completion_value
              << " fence=" << completion_fence << "\n";
  }
  g_metal_composer_layers.clear();
  g_metal_composer_transaction_id = 0;

  RestoreSurfaceControlContextScope();
  return completion_fence;
}

extern "C" void darwin_art_android_set_hardware_buffer_composition_active(
    bool active) {
  if (g_surface_control_target.iosurface == nullptr) return;
  darwin_art_surface_gpu_set_iosurface_composition_active(
      g_surface_control_target.iosurface, active);
}

extern "C" void darwin_art_android_mark_hardware_buffer_released(
    void* opaque) {
  auto* buffer = static_cast<AHardwareBuffer*>(opaque);
  if (buffer == nullptr) return;
  std::lock_guard<std::mutex> lock(AhbEglImageMutex());
  for (auto& [handle, image] : AhbEglImages()) {
    (void)handle;
    if (image->buffer != buffer) continue;
    // The release fence belongs to this exact BufferQueue slot.  Do not copy
    // immediately: Chromium may still have an active Metal render encoder.
    // Invalidating the staging generation makes the next producer FBO bind
    // restore the slot's persistent IOSurface before partial damage is drawn.
    image->staging_content_generation = 0;
    image->staging_source_buffer = nullptr;
    if (DebugGraphicsDso()) {
      std::cerr << "ART Android EGL: released AHB slot buffer=" << buffer
                << " context=" << image->owner_context
                << " texture=" << image->client_texture << " content="
                << image->iosurface_content_generation << "\n";
    }
  }
}

extern "C" void darwin_art_android_present_hardware_buffer(
    void* queue, std::uint32_t owner_process_id, std::uint32_t layer_id,
    std::uint32_t parent_owner_process_id, std::uint32_t parent_id,
    std::uint64_t what, std::uint32_t relative_parent_owner_process_id,
    std::uint32_t relative_parent_id, std::int32_t z, void* opaque,
    std::uint32_t transform,
    std::int32_t source_left,
    std::int32_t source_top,
    std::int32_t source_right, std::int32_t source_bottom,
    std::int32_t destination_left, std::int32_t destination_top,
    std::int32_t destination_right, std::int32_t destination_bottom,
    bool has_damage, std::int32_t damage_left, std::int32_t damage_top,
    std::int32_t damage_right, std::int32_t damage_bottom, float alpha) {
  auto* buffer = static_cast<AHardwareBuffer*>(opaque);
  if (buffer == nullptr) return;
  std::lock_guard<std::mutex> lock(AhbEglImageMutex());
  const char* surfaceflinger_socket =
      std::getenv("DARWIN_ART_SURFACEFLINGER_SOCKET");
  const bool remote_surfaceflinger =
      surfaceflinger_socket != nullptr && surfaceflinger_socket[0] != '\0';
  if (remote_surfaceflinger) {
    // SurfaceFlinger consumes the gralloc contract, not ANGLE's EGLImage
    // bookkeeping. ViewRoot/HWUI buffers are valid AHardwareBuffers backed by
    // IOSurface even when no client EGLImage was ever created for them.
    AHardwareBuffer_Desc description{};
    AHardwareBuffer_describe(buffer, &description);
    void* iosurface = darwin_art_android_hardware_buffer_iosurface(buffer);
    if (iosurface == nullptr || description.width == 0 ||
        description.height == 0) {
      return;
    }
    const auto width = static_cast<std::int32_t>(description.width);
    const auto height = static_cast<std::int32_t>(description.height);
    // The Darwin gralloc contract uses COMPOSER_OVERLAY to identify buffers
    // that the EGL producer has already resolved into display-space. HWUI's
    // Metal render target omits it and retains bottom-left native storage.
    const bool producer_bottom_left =
        (description.usage & AHARDWAREBUFFER_USAGE_COMPOSER_OVERLAY) == 0;
    source_left = std::clamp(source_left, 0, width);
    source_right = std::clamp(source_right, 0, width);
    source_top = std::clamp(source_top, 0, height);
    source_bottom = std::clamp(source_bottom, 0, height);
    const auto& target = g_surface_control_target;
    destination_left = std::clamp(
        destination_left, 0, static_cast<std::int32_t>(target.width));
    destination_right = std::clamp(
        destination_right, 0, static_cast<std::int32_t>(target.width));
    destination_top = std::clamp(
        destination_top, 0, static_cast<std::int32_t>(target.height));
    destination_bottom = std::clamp(
        destination_bottom, 0, static_cast<std::int32_t>(target.height));
    g_metal_composer_layers.push_back({
        .owner_process_id = owner_process_id,
        .layer_id = layer_id,
        .parent_owner_process_id = parent_owner_process_id,
        .parent_id = parent_id,
        .relative_parent_owner_process_id =
            relative_parent_owner_process_id,
        .relative_parent_id = relative_parent_id,
        .what = what,
        .transform = transform,
        .producer_bottom_left = producer_bottom_left,
        .iosurface = iosurface,
        .width = description.width,
        .height = description.height,
        .source_left = source_left,
        .source_top = source_top,
        .source_right = source_right,
        .source_bottom = source_bottom,
        .destination_left = destination_left,
        .destination_top = destination_top,
        .destination_right = destination_right,
        .destination_bottom = destination_bottom,
        .z = z,
        .alpha = alpha,
    });
    (void)queue;
    (void)has_damage;
    (void)damage_left;
    (void)damage_top;
    (void)damage_right;
    (void)damage_bottom;
    return;
  }
  auto found = std::find_if(
      AhbEglImages().begin(), AhbEglImages().end(),
      [buffer](const auto& entry) { return entry.second->buffer == buffer; });
  if (found == AhbEglImages().end()) return;
  DarwinAhbEglImage& image = *found->second;
  const bool direct_metal =
      image.metal_image != nullptr && image.client_staging_texture != 0;
  const bool staged_rectangle =
      image.bind_target == kEglTextureRectangleAngle &&
      image.client_staging_texture != 0 && image.iosurface_texture != 0;
  if (!image.bound || (!direct_metal && !staged_rectangle)) {
    return;
  }
  // BufferQueue preserves the last presented pixels when a producer receives
  // a buffer with a positive age. Remember that source per producer context;
  // the next acquire-fence refresh performs the equivalent preservation as a
  // GPU blit before Chromium applies its damage region.
  LastPresentedAhbByContext()[image.owner_context] = image.buffer;
  ++PresentedGenerationByContext()[image.owner_context];
  if (queue != nullptr) {
    QueueByAhb()[image.buffer] = queue;
    LastPresentedAhbByQueue()[queue] = image.buffer;
    ++PresentedGenerationByQueue()[queue];
  }
  const auto& target = g_surface_control_target;
  const auto image_width = static_cast<std::int32_t>(image.width);
  const auto image_height = static_cast<std::int32_t>(image.height);
  source_left = std::clamp(source_left, 0, image_width);
  source_right = std::clamp(source_right, 0, image_width);
  source_top = std::clamp(source_top, 0, image_height);
  source_bottom = std::clamp(source_bottom, 0, image_height);
  destination_left = std::clamp(
      destination_left, 0, static_cast<std::int32_t>(target.width));
  destination_right = std::clamp(
      destination_right, 0, static_cast<std::int32_t>(target.width));
  destination_top = std::clamp(
      destination_top, 0, static_cast<std::int32_t>(target.height));
  destination_bottom = std::clamp(
      destination_bottom, 0, static_cast<std::int32_t>(target.height));
  g_metal_composer_layers.push_back({
      .owner_process_id = owner_process_id,
      .layer_id = layer_id,
      .parent_owner_process_id = parent_owner_process_id,
      .parent_id = parent_id,
      .what = what,
      .transform = transform,
      .producer_bottom_left =
          (image.usage & AHARDWAREBUFFER_USAGE_COMPOSER_OVERLAY) == 0,
      .iosurface = darwin_art_android_hardware_buffer_iosurface(image.buffer),
      .width = image.width,
      .height = image.height,
      .source_left = source_left,
      .source_top = source_top,
      .source_right = source_right,
      .source_bottom = source_bottom,
      .destination_left = destination_left,
      .destination_top = destination_top,
      .destination_right = destination_right,
      .destination_bottom = destination_bottom,
      .z = z,
      .alpha = alpha,
  });
  // Damage remains scheduling metadata. SurfaceFlinger supplies the complete
  // retained layer tree here, and Metal clips the destination geometry.
  (void)has_damage;
  (void)damage_left;
  (void)damage_top;
  (void)damage_right;
  (void)damage_bottom;
  return;

  auto& api = GetAngleApi();
  std::int32_t previous_read_framebuffer = 0;
  std::int32_t previous_draw_framebuffer = 0;
  std::int32_t previous_scissor[4]{};
  const bool scissor_enabled = api.gl_is_enabled(0x0C11) != 0;
  api.gl_get_integer_v(0x8CAA, &previous_read_framebuffer);
  api.gl_get_integer_v(0x8CA6, &previous_draw_framebuffer);
  api.gl_get_integer_v(0x0C10, previous_scissor);  // GL_SCISSOR_BOX
  std::uint32_t framebuffers[2]{};
  api.gl_gen_framebuffers(2, framebuffers);
  api.gl_bind_framebuffer(0x8CA8, framebuffers[0]);  // GL_READ_FRAMEBUFFER
  // SurfaceTransaction can run after Chromium has released or reused the
  // producer's temporary 2D texture. Read the persistent AHardwareBuffer
  // IOSurface snapshot made at native-fence creation instead.
  api.gl_framebuffer_texture_2d(
      0x8CA8, kGlColorAttachment0,
      direct_metal ? kGlTexture2d : kGlTextureRectangleAngle,
      direct_metal ? image.client_staging_texture : image.iosurface_texture,
      0);
  if (std::getenv("DARWIN_ART_DEBUG_AHB_DUMP") != nullptr) {
    static std::atomic<std::uint32_t> dump_index{0};
    const std::uint32_t index =
        dump_index.fetch_add(1, std::memory_order_relaxed);
    std::vector<std::uint8_t> pixels(
        static_cast<std::size_t>(image.width) * image.height * 4);
    api.gl_read_pixels(0, 0, image.width, image.height, 0x1908,
                       kGlUnsignedByte, pixels.data());
    const std::string path = "/tmp/darwin-art-root-ahb-" +
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
  if (std::getenv("DARWIN_ART_DEBUG_SURFACECONTROL_PIXELS") != nullptr) {
    constexpr std::array<float, 3> positions{0.25f, 0.5f, 0.75f};
    std::cerr << "ART Android SurfaceControl: source samples";
    for (float y : positions) {
      for (float x : positions) {
        std::uint8_t pixel[4]{};
        api.gl_read_pixels(
            static_cast<std::int32_t>(image.width * x),
            static_cast<std::int32_t>(image.height * y), 1, 1, 0x1908,
            kGlUnsignedByte, pixel);
        std::cerr << " [" << x << "," << y << "]="
                  << static_cast<int>(pixel[0]) << ","
                  << static_cast<int>(pixel[1]) << ","
                  << static_cast<int>(pixel[2]) << ","
                  << static_cast<int>(pixel[3]);
      }
    }
    std::cerr << " error=0x" << std::hex << api.gl_get_error() << std::dec
              << "\n";
  }
  if (std::getenv("DARWIN_ART_DEBUG_SURFACECONTROL_CLEAR") != nullptr) {
    api.gl_bind_framebuffer(kGlFramebuffer, framebuffers[0]);
    api.gl_clear_color(1.0f, 0.0f, 0.0f, 1.0f);
    api.gl_clear(0x00004000);  // GL_COLOR_BUFFER_BIT
  }
  bool submitted_to_compositor = false;
  if (EnsureSurfaceControlTarget(image.display)) {
    auto& target = g_surface_control_target;
    api.gl_bind_framebuffer(0x8CA9, target.framebuffer);
    api.gl_bind_framebuffer(0x8CA8, framebuffers[0]);
    const auto image_width = static_cast<std::int32_t>(image.width);
    const auto image_height = static_cast<std::int32_t>(image.height);
    source_left = std::clamp(source_left, 0, image_width);
    source_right = std::clamp(source_right, 0, image_width);
    source_top = std::clamp(source_top, 0, image_height);
    source_bottom = std::clamp(source_bottom, 0, image_height);
    destination_left = std::clamp(
        destination_left, 0, static_cast<std::int32_t>(target.width));
    destination_right = std::clamp(
        destination_right, 0, static_cast<std::int32_t>(target.width));
    destination_top = std::clamp(
        destination_top, 0, static_cast<std::int32_t>(target.height));
    destination_bottom = std::clamp(
        destination_bottom, 0, static_cast<std::int32_t>(target.height));
    // Damage is buffer-coordinate compositor metadata, never layer geometry.
    // Preserve the complete source-to-destination mapping and limit writes
    // with a destination scissor, matching SurfaceFlinger's composition model.
    std::int32_t composite_source_left = source_left;
    std::int32_t composite_source_top = source_top;
    std::int32_t composite_source_right = source_right;
    std::int32_t composite_source_bottom = source_bottom;
    std::int32_t composite_destination_left = destination_left;
    std::int32_t composite_destination_top = destination_top;
    std::int32_t composite_destination_right = destination_right;
    std::int32_t composite_destination_bottom = destination_bottom;
    api.gl_disable(0x0C11);  // Never inherit the producer's damage scissor.
    if (has_damage && source_right > source_left &&
        source_bottom > source_top) {
      damage_left = std::clamp(damage_left, source_left, source_right);
      damage_right = std::clamp(damage_right, source_left, source_right);
      damage_top = std::clamp(damage_top, source_top, source_bottom);
      damage_bottom = std::clamp(damage_bottom, source_top, source_bottom);
      if (damage_left < damage_right && damage_top < damage_bottom) {
        const double scale_x =
            static_cast<double>(destination_right - destination_left) /
            static_cast<double>(source_right - source_left);
        const double scale_y =
            static_cast<double>(destination_bottom - destination_top) /
            static_cast<double>(source_bottom - source_top);
        const std::int32_t clip_left = destination_left +
            static_cast<std::int32_t>((damage_left - source_left) * scale_x);
        const std::int32_t clip_right = destination_left +
            static_cast<std::int32_t>((damage_right - source_left) * scale_x);
        const std::int32_t clip_top = destination_top +
            static_cast<std::int32_t>((damage_top - source_top) * scale_y);
        const std::int32_t clip_bottom = destination_top +
            static_cast<std::int32_t>((damage_bottom - source_top) * scale_y);
        // A SurfaceControl damage rect uses top-left buffer coordinates, while
        // the producer rendered the partial update in GL bottom-left storage.
        // Crop the mirrored source region and place it into the top-left
        // destination region, exactly as SurfaceFlinger does. A destination
        // scissor alone selects the wrong source rows for partial buffers.
        composite_source_left = damage_left;
        composite_source_top = damage_top;
        composite_source_right = damage_right;
        composite_source_bottom = damage_bottom;
        composite_destination_left = clip_left;
        composite_destination_top = clip_top;
        composite_destination_right = clip_right;
        composite_destination_bottom = clip_bottom;
      } else {
        has_damage = false;
      }
    }
    const bool debug_damage_pixels =
        std::getenv("DARWIN_ART_DEBUG_SURFACECONTROL_DAMAGE_PIXELS") != nullptr &&
        has_damage;
    auto sample_damage = [&](const char* phase, std::uint32_t framebuffer,
                             std::int32_t width, std::int32_t height,
                             std::int32_t left, std::int32_t top,
                             std::int32_t right, std::int32_t bottom) {
      if (!debug_damage_pixels) return;
      api.gl_bind_framebuffer(0x8CA8, framebuffer);
      std::cerr << "ART Android SurfaceControl: damage " << phase << " rect=["
                << left << "," << top << "," << right << "," << bottom
                << "]";
      for (float y : std::array<float, 3>{0.1f, 0.5f, 0.9f}) {
        for (float x : std::array<float, 3>{0.1f, 0.5f, 0.9f}) {
          const std::int32_t sample_x =
              std::clamp(left + static_cast<std::int32_t>((right - left) * x),
                         0, width - 1);
          const std::int32_t sample_y_top =
              std::clamp(top + static_cast<std::int32_t>((bottom - top) * y),
                         0, height - 1);
          std::uint8_t pixel[4]{};
          const std::int32_t sample_y =
              std::strcmp(phase, "source") == 0
                  ? height - 1 - sample_y_top
                  : sample_y_top;
          api.gl_read_pixels(sample_x, sample_y, 1, 1, 0x1908, kGlUnsignedByte,
                             pixel);
          std::cerr << " [" << sample_x << "," << sample_y_top << "]="
                    << static_cast<int>(pixel[0]) << ","
                    << static_cast<int>(pixel[1]) << ","
                    << static_cast<int>(pixel[2]) << ","
                    << static_cast<int>(pixel[3]);
        }
      }
      std::cerr << "\n";
    };
    sample_damage("source", framebuffers[0], image_width, image_height,
                  source_left, source_top, source_right, source_bottom);
    if (!CompositeSurfaceControlImageLocked(
            image, target, composite_source_left, composite_source_top,
            composite_source_right, composite_source_bottom,
            composite_destination_left, composite_destination_top,
            composite_destination_right, composite_destination_bottom,
            alpha)) {
      // Keep a visible diagnostic fallback for ANGLE builds which lack the
      // rectangle-texture shader extension. Normal Chromium composition uses
      // the premultiplied source-over path above and never copies transparent
      // RGB into the host surface.
      api.gl_blit_framebuffer_angle(
          composite_source_left, image_height - composite_source_bottom,
          composite_source_right, image_height - composite_source_top,
          composite_destination_left, composite_destination_top,
          composite_destination_right, composite_destination_bottom,
          0x00004000, 0x2600);  // GL_COLOR_BUFFER_BIT / GL_NEAREST
    }
    sample_damage("target", target.framebuffer,
                  static_cast<std::int32_t>(target.width),
                  static_cast<std::int32_t>(target.height), destination_left,
                  destination_top, destination_right, destination_bottom);
    submitted_to_compositor = true;
    if (std::getenv("DARWIN_ART_DEBUG_SURFACECONTROL_PIXELS") != nullptr) {
      constexpr std::array<float, 3> positions{0.25f, 0.5f, 0.75f};
      api.gl_bind_framebuffer(0x8CA8, target.framebuffer);
      std::cerr << "ART Android SurfaceControl: target samples";
      for (float y : positions) {
        for (float x : positions) {
          std::uint8_t pixel[4]{};
          api.gl_read_pixels(static_cast<std::int32_t>(target.width * x),
                             static_cast<std::int32_t>(target.height * y), 1,
                             1, 0x1908, kGlUnsignedByte, pixel);
          std::cerr << " [" << x << "," << y << "]="
                    << static_cast<int>(pixel[0]) << ","
                    << static_cast<int>(pixel[1]) << ","
                    << static_cast<int>(pixel[2]) << ","
                    << static_cast<int>(pixel[3]);
        }
      }
      std::cerr << " error=0x" << std::hex << api.gl_get_error() << std::dec
                << "\n";
    }
    if (std::getenv("DARWIN_ART_DEBUG_SURFACECONTROL_CLEAR") != nullptr) {
      std::uint8_t pixel[4]{};
      api.gl_bind_framebuffer(0x8CA8, target.framebuffer);
      api.gl_read_pixels(static_cast<std::int32_t>(target.width / 2),
                         static_cast<std::int32_t>(target.height / 2), 1, 1,
                         0x1908, kGlUnsignedByte, pixel);
      std::cerr << "ART Android SurfaceControl: compositor center rgba="
                << static_cast<int>(pixel[0]) << ","
                << static_cast<int>(pixel[1]) << ","
                << static_cast<int>(pixel[2]) << ","
                << static_cast<int>(pixel[3]) << " error=0x" << std::hex
                << api.gl_get_error() << std::dec << "\n";
    }
  }
  api.gl_bind_framebuffer(0x8CA8,
                          static_cast<std::uint32_t>(previous_read_framebuffer));
  api.gl_bind_framebuffer(0x8CA9,
                          static_cast<std::uint32_t>(previous_draw_framebuffer));
  api.gl_scissor(previous_scissor[0], previous_scissor[1],
                 previous_scissor[2], previous_scissor[3]);
  if (scissor_enabled)
    api.gl_enable(0x0C11);  // GL_SCISSOR_TEST
  else
    api.gl_disable(0x0C11);
  api.gl_delete_framebuffers(2, framebuffers);
  using Flush = void (*)();
  auto flush = reinterpret_cast<Flush>(api.get_proc_address("glFlush"));
  if (flush != nullptr) flush();
  if (DebugGraphicsDso()) {
    static std::atomic<std::uint32_t> submissions{0};
    const std::uint32_t frame =
        submissions.fetch_add(1, std::memory_order_relaxed) + 1;
    if (frame <= 4 || frame % 60 == 0) {
      std::cerr << "ART Android SurfaceControl: submitted GPU frame=" << frame
                << " compositor=" << submitted_to_compositor << " size="
                << image.width << "x" << image.height
                << " context=" << image.owner_context
                << " buffer=" << image.buffer << "\n";
    }
  }
  (void)alpha;
}

extern "C" void darwin_art_android_present_surface_control_state(
    std::uint32_t owner_process_id, std::uint32_t layer_id,
    std::uint32_t parent_owner_process_id, std::uint32_t parent_id,
    std::uint32_t relative_parent_owner_process_id,
    std::uint32_t relative_parent_id, std::uint64_t what,
    std::uint32_t flags, std::uint32_t mask,
    std::uint32_t transform,
    std::int32_t destination_left, std::int32_t destination_top,
    std::int32_t destination_right, std::int32_t destination_bottom,
    std::int32_t position_x, std::int32_t position_y, float scale_x,
    float scale_y, bool has_crop, std::int32_t crop_left,
    std::int32_t crop_top, std::int32_t crop_right,
    std::int32_t crop_bottom, std::int32_t z, float alpha,
    const std::int32_t* transparent_region_rects,
    std::uint32_t transparent_region_count) {
  g_metal_composer_layers.push_back({
      .owner_process_id = owner_process_id,
      .layer_id = layer_id,
      .parent_owner_process_id = parent_owner_process_id,
      .parent_id = parent_id,
      .relative_parent_owner_process_id =
          relative_parent_owner_process_id,
      .relative_parent_id = relative_parent_id,
      .what = what,
      .flags = flags,
      .mask = mask,
      .transform = transform,
      .iosurface = nullptr,
      .destination_left = destination_left,
      .destination_top = destination_top,
      .destination_right = destination_right,
      .destination_bottom = destination_bottom,
      .position_x = position_x,
      .position_y = position_y,
      .scale_x = scale_x,
      .scale_y = scale_y,
      .has_crop = has_crop,
      .crop_left = crop_left,
      .crop_top = crop_top,
      .crop_right = crop_right,
      .crop_bottom = crop_bottom,
      .z = z,
      .alpha = alpha,
  });
  DarwinArtMetalComposerLayer& state = g_metal_composer_layers.back();
  state.transparent_region_count = std::min(
      transparent_region_rects == nullptr ? 0u : transparent_region_count,
      static_cast<std::uint32_t>(kDarwinArtMaxTransparentRegionRects));
  for (std::uint32_t index = 0; index < state.transparent_region_count;
       ++index) {
    const std::int32_t* source = transparent_region_rects + index * 4;
    state.transparent_region[index] = {
        .left = source[0],
        .top = source[1],
        .right = source[2],
        .bottom = source[3],
    };
  }
}

void RestoreBufferQueueSlotIfNeeded(std::uint32_t texture) {
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
  auto& api = GetAngleApi();
  const EGLContext current = api.get_current_context == nullptr
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
          CopyIosurfaceToAhbClientTextureLocked(*predecessor, *destination)) {
        destination->staging_content_generation =
            predecessor->iosurface_content_generation;
        destination->staging_source_buffer = predecessor->buffer;
        if (DebugGraphicsDso()) {
          std::cerr << "ART Android EGL: seeded new AHB representation context="
                    << current << " texture=" << texture << " previous="
                    << predecessor->buffer << " destination="
                    << destination->buffer << " content="
                    << predecessor->iosurface_content_generation << "\n";
        }
      }
      return;
    }
    // Metal EGLImages already share the slot IOSurface directly. The
    // rectangle fallback performs the equivalent self restore below.
    if (destination->association_generation != 0 &&
        destination->iosurface_content_generation != 0 &&
        destination->staging_content_generation !=
            destination->iosurface_content_generation &&
        CopyIosurfaceToAhbClientTextureLocked(*destination, *destination)) {
      destination->staging_content_generation =
          destination->iosurface_content_generation;
      destination->staging_source_buffer = destination->buffer;
      if (DebugGraphicsDso()) {
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
    RestoreBufferQueueSlotIfNeeded(texture);
  }
  if (texture_target == kGlTexture2d && texture != 0) {
    auto& api = GetAngleApi();
    const EGLContext current = api.get_current_context == nullptr
                                   ? nullptr
                                   : api.get_current_context();
    std::lock_guard<std::mutex> lock(AhbEglImageMutex());
    texture = HostTextureForGuestTextureLocked(current, texture);
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
    RestoreBufferQueueSlotIfNeeded(texture);
  }
  if (texture_target == kGlTexture2d && texture != 0) {
    auto& api = GetAngleApi();
    const EGLContext current = api.get_current_context == nullptr
                                   ? nullptr
                                   : api.get_current_context();
    std::lock_guard<std::mutex> lock(AhbEglImageMutex());
    texture = HostTextureForGuestTextureLocked(current, texture);
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
    std::lock_guard<std::mutex> lock(AhbEglImageMutex());
    texture = HostTextureForGuestTextureLocked(current, texture);
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
    *params = static_cast<std::int32_t>(GuestTextureForHostTexture(
        static_cast<std::uint32_t>(*params)));
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
    RestoreBufferQueueSlotIfNeeded(GuestTextureForHostTexture(
        static_cast<std::uint32_t>(attachment_name)));
  }
}

void GlDeleteTexturesAndroid(std::int32_t count,
                             const std::uint32_t* textures) {
  if (count <= 0 || textures == nullptr) return;
  auto& api = GetAngleApi();
  const EGLContext current = api.get_current_context == nullptr
                                 ? nullptr
                                 : api.get_current_context();
  {
    std::lock_guard<std::mutex> lock(AhbEglImageMutex());
    for (auto& [handle, image] : AhbEglImages()) {
      (void)handle;
      if (image->owner_context != current || image->client_texture == 0)
        continue;
      if (std::find(textures, textures + count, image->client_texture) ==
          textures + count) {
        continue;
      }
      if (DebugGraphicsDso()) {
        std::cerr << "ART Android EGL: detached deleted AHB texture context="
                  << current << " texture=" << image->client_texture
                  << " buffer=" << image->buffer << " size=" << image->width
                  << "x" << image->height << "\n";
      }
      // glDeleteTextures only retires Chromium's guest name.  The private
      // texture is owned by the EGLImage/AHardwareBuffer and must remain
      // alive until eglDestroyImage: SurfaceControl commits the buffer after
      // SharedImage has deleted its temporary client texture.  Dropping the
      // private texture here made the transaction silently skip the web
      // layer, leaving Chrome's Android chrome visible over a black page.
      image->client_texture = 0;
    }
  }
  api.gl_delete_textures(count, textures);
}

void DetachBoundAhbTextureForStorage(std::uint32_t target,
                                     const char* operation,
                                     std::int32_t width,
                                     std::int32_t height) {
  if (target != kGlTexture2d) return;
  auto& api = GetAngleApi();
  std::int32_t host_texture = 0;
  api.gl_get_integer_v(0x8069, &host_texture);  // GL_TEXTURE_BINDING_2D
  if (host_texture == 0) return;
  const EGLContext current = api.get_current_context == nullptr
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
    if (DebugGraphicsDso()) {
      std::cerr << "ART Android EGL: detached redefined AHB texture context="
                << current << " texture=" << image->client_texture
                << " operation=" << operation << " new_size=" << width
                << "x" << height << " buffer=" << image->buffer
                << " old_size=" << image->width << "x" << image->height
                << "\n";
    }
    const std::uint32_t guest_texture = image->client_texture;
    const std::uint32_t staging_texture = image->client_staging_texture;
    // The explicit storage operation supersedes the EGLImage association.
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

void GlTexImage2dAndroid(std::uint32_t target, std::int32_t level,
                         std::int32_t internal_format, std::int32_t width,
                         std::int32_t height, std::int32_t border,
                         std::uint32_t format, std::uint32_t type,
                         const void* pixels) {
  if (level == 0) {
    DetachBoundAhbTextureForStorage(target, "glTexImage2D", width, height);
  }
  GetAngleApi().gl_tex_image_2d(target, level, internal_format, width, height,
                                border, format, type, pixels);
}

void GlTexStorage2dAndroid(std::uint32_t target, std::int32_t levels,
                           std::uint32_t internal_format,
                           std::int32_t width, std::int32_t height) {
  DetachBoundAhbTextureForStorage(target, "glTexStorage2D", width, height);
  using Function = void (*)(std::uint32_t, std::int32_t, std::uint32_t,
                            std::int32_t, std::int32_t);
  auto function = reinterpret_cast<Function>(
      GetAngleApi().get_proc_address("glTexStorage2D"));
  if (function != nullptr) function(target, levels, internal_format, width, height);
}

void GlTexStorage2dExtAndroid(std::uint32_t target, std::int32_t levels,
                              std::uint32_t internal_format,
                              std::int32_t width, std::int32_t height) {
  DetachBoundAhbTextureForStorage(target, "glTexStorage2DEXT", width, height);
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
