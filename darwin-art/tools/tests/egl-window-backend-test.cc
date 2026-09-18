#include "compat/graphics/egl_window_backend.h"
#include "compat/graphics/egl_error_state.h"

#include <atomic>
#include <cassert>
#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <new>

using Backend = darwin_art::graphics::EglWindowBackend;
using Table = darwin_art::graphics::EglWindowBackendTable;
using Resources = darwin_art::graphics::EglWindowBackendResources;
using Owner = darwin_art::graphics::EglWindowSurfaceOwner;

struct State {
  int next_surface = 0;
  int initialize = 0;
  int terminate = 0;
  int destroy = 0;
  int swap = 0;
  int gen_texture = 0;
  int del_texture = 0;
  int gen_fbo = 0;
  int del_fbo = 0;
  int publish = 0;
  int native_acquire = 0;
  int native_release = 0;
  int native_dequeue = 0;
  int native_queue = 0;
  int native_cancel = 0;
  int release_iosurface = 0;
  bool swap_failure = false;
  bool wait_gl_failure = false;
  bool complete_fbo = true;
  bool reset = true;
  int iosurface_serial = 0;
  bool initialize_failure = false;
  bool native_mode = false;
  bool native_dequeue_failure = false;
  bool client_pbuffer_failure = false;
  bool render_pbuffer_failure = false;
  std::int32_t provider_error = 0x3006;
  int provider_error_reads = 0;
  bool track_provider_cleanup_order = false;
  bool provider_cleanup_raced_error = false;
  std::int32_t active_texture = 0x84C3;
  std::int32_t texture_2d = 31;
  std::int32_t texture_rect = 32;
  std::int32_t read_framebuffer = 41;
  std::int32_t draw_framebuffer = 42;
  std::int32_t scissor[4] = {3, 4, 300, 200};
  bool scissor_enabled = true;
};

State* g_state = nullptr;
bool fail_next_allocation = false;

void* operator new(std::size_t size) {
  if (fail_next_allocation) {
    fail_next_allocation = false;
    throw std::bad_alloc();
  }
  if (void* result = std::malloc(size == 0 ? 1 : size)) return result;
  throw std::bad_alloc();
}
void* operator new[](std::size_t size) { return ::operator new(size); }
void operator delete(void* pointer) noexcept { std::free(pointer); }
void operator delete[](void* pointer) noexcept { std::free(pointer); }
void operator delete(void* pointer, std::size_t) noexcept { std::free(pointer); }
void operator delete[](void* pointer, std::size_t) noexcept { std::free(pointer); }

std::uint32_t Initialize(void*, std::int32_t* major, std::int32_t* minor) {
  ++g_state->initialize;
  if (g_state->initialize_failure) return 0;
  *major = 1;
  *minor = 5;
  return 1;
}
std::uint32_t Terminate(void*) { return ++g_state->terminate, 1; }
void* Pbuffer(void*, void*, const std::int32_t*) {
  if (g_state->render_pbuffer_failure) return nullptr;
  return reinterpret_cast<void*>(static_cast<std::uintptr_t>(
      0x1000 + ++g_state->next_surface));
}
void* ClientPbuffer(void*, std::uint32_t, void*, void*, const std::int32_t*) {
  if (g_state->client_pbuffer_failure) return nullptr;
  return reinterpret_cast<void*>(static_cast<std::uintptr_t>(
      0x2000 + ++g_state->next_surface));
}
std::uint32_t Destroy(void*, void*) {
  if (g_state->track_provider_cleanup_order &&
      g_state->provider_error_reads == 0)
    g_state->provider_cleanup_raced_error = true;
  return ++g_state->destroy, 1;
}
std::uint32_t Swap(void*, void*) {
  ++g_state->swap;
  return g_state->swap_failure ? 0 : 1;
}
std::uint32_t Bind(void*, void*, std::int32_t) { return 1; }
std::uint32_t Release(void*, void*, std::int32_t) { return 1; }
std::uint32_t WaitGl() { return g_state->wait_gl_failure ? 0 : 1; }
std::uint32_t ReleaseThread() { return 1; }
std::uint32_t ConfigAttrib(void*, void*, std::int32_t, std::int32_t* value) {
  *value = 0x305F;
  return 1;
}
std::int32_t GetError() {
  ++g_state->provider_error_reads;
  return g_state->provider_error;
}
void GetInteger(std::uint32_t name, std::int32_t* value) {
  switch (name) {
    case 0x84E0:
      *value = g_state->active_texture;
      break;
    case 0x8069:
      *value = g_state->texture_2d;
      break;
    case 0x84F6:
      *value = g_state->texture_rect;
      break;
    case 0x8CAA:
      *value = g_state->read_framebuffer;
      break;
    case 0x8CA6:
      *value = g_state->draw_framebuffer;
      break;
    case 0x0C10:
      value[0] = g_state->scissor[0];
      value[1] = g_state->scissor[1];
      value[2] = g_state->scissor[2];
      value[3] = g_state->scissor[3];
      break;
    default:
      *value = 0;
      break;
  }
}
std::uint8_t IsEnabled(std::uint32_t name) {
  return name == 0x0C11 && g_state->scissor_enabled;
}
void Disable(std::uint32_t name) {
  if (name == 0x0C11) g_state->scissor_enabled = false;
}
void Enable(std::uint32_t name) {
  if (name == 0x0C11) g_state->scissor_enabled = true;
}
void Scissor(std::int32_t x, std::int32_t y, std::int32_t width,
             std::int32_t height) {
  g_state->scissor[0] = x;
  g_state->scissor[1] = y;
  g_state->scissor[2] = width;
  g_state->scissor[3] = height;
}
void BindTexture(std::uint32_t target, std::uint32_t texture) {
  if (target == 0x0DE1) g_state->texture_2d = static_cast<std::int32_t>(texture);
  if (target == 0x84F5) g_state->texture_rect = static_cast<std::int32_t>(texture);
}
void ActiveTexture(std::uint32_t texture) {
  g_state->active_texture = static_cast<std::int32_t>(texture);
}
void TexParameter(std::uint32_t, std::uint32_t, std::int32_t) {}
void GenTexture(std::int32_t, std::uint32_t* value) {
  ++g_state->gen_texture;
  *value = 7;
}
void DeleteTexture(std::int32_t, const std::uint32_t*) { ++g_state->del_texture; }
void GenFbo(std::int32_t, std::uint32_t* value) {
  ++g_state->gen_fbo;
  *value = 9;
}
void BindFbo(std::uint32_t target, std::uint32_t fbo) {
  if (target == 0x8CA8) g_state->read_framebuffer = static_cast<std::int32_t>(fbo);
  if (target == 0x8CA9) g_state->draw_framebuffer = static_cast<std::int32_t>(fbo);
  if (target == 0x8D40) {
    g_state->read_framebuffer = static_cast<std::int32_t>(fbo);
    g_state->draw_framebuffer = static_cast<std::int32_t>(fbo);
  }
}
void Attach(std::uint32_t, std::uint32_t, std::uint32_t, std::uint32_t,
            std::int32_t) {}
std::uint32_t CheckFbo(std::uint32_t) {
  return g_state->complete_fbo ? 0x8CD5u : 0;
}
void DeleteFbo(std::int32_t, const std::uint32_t*) { ++g_state->del_fbo; }
void Blit(std::int32_t, std::int32_t, std::int32_t, std::int32_t,
          std::int32_t, std::int32_t, std::int32_t, std::int32_t,
          std::uint32_t, std::uint32_t) {}
std::uint32_t GlError() { return 0; }

void* CurrentHost(void*) { return reinterpret_cast<void*>(0x44); }
bool AcquireIosurface(void*, void*, void** iosurface, std::uint32_t* width,
                      std::uint32_t* height) {
  *iosurface = reinterpret_cast<void*>(0x55 + g_state->iosurface_serial);
  *width = 320 + static_cast<std::uint32_t>(g_state->iosurface_serial) * 10;
  *height = 240 + static_cast<std::uint32_t>(g_state->iosurface_serial) * 10;
  return true;
}
void ReleaseIosurface(void*, void*) { ++g_state->release_iosurface; }
bool Reset(void*, void*) { return g_state->reset; }
void SetExtent(void*, void*, std::uint32_t, std::uint32_t) {}
void Publish(void*, void*) { ++g_state->publish; }

void NativeAcquire(void*, void*) { ++g_state->native_acquire; }
void NativeRelease(void*, void*) { ++g_state->native_release; }
int NativeDequeue(void*, void*, void** hardware, void** buffer, int* fence) {
  ++g_state->native_dequeue;
  if (g_state->native_dequeue_failure) return -1;
  *hardware = reinterpret_cast<void*>(0x8000 + g_state->native_dequeue);
  *buffer = reinterpret_cast<void*>(0x9000 + g_state->native_dequeue);
  *fence = -1;
  return 0;
}
int NativeQueue(void*, void*, void*, int) { return ++g_state->native_queue, 0; }
int NativeCancel(void*, void*, void*, int) {
  return ++g_state->native_cancel, 0;
}
void DescribeBuffer(void*, void*, std::uint32_t* width, std::uint32_t* height) {
  *width = 320;
  *height = 240;
}
void* BufferIosurface(void*, void* hardware) {
  return reinterpret_cast<void*>(reinterpret_cast<std::uintptr_t>(hardware) + 0x1000);
}

Table MakeTable() {
  Table table;
  table.initialize = Initialize;
  table.terminate = Terminate;
  table.create_pbuffer_surface = Pbuffer;
  table.create_pbuffer_from_client_buffer = ClientPbuffer;
  table.destroy_surface = Destroy;
  table.swap_buffers = Swap;
  table.bind_tex_image = Bind;
  table.release_tex_image = Release;
  table.wait_gl = WaitGl;
  table.release_thread = ReleaseThread;
  table.get_config_attrib = ConfigAttrib;
  table.get_error = GetError;
  table.gl_get_integer_v = GetInteger;
  table.gl_is_enabled = IsEnabled;
  table.gl_disable = Disable;
  table.gl_enable = Enable;
  table.gl_scissor = Scissor;
  table.gl_bind_texture = BindTexture;
  table.gl_active_texture = ActiveTexture;
  table.gl_tex_parameter_i = TexParameter;
  table.gl_gen_textures = GenTexture;
  table.gl_delete_textures = DeleteTexture;
  table.gl_gen_framebuffers = GenFbo;
  table.gl_bind_framebuffer = BindFbo;
  table.gl_framebuffer_texture_2d = Attach;
  table.gl_check_framebuffer_status = CheckFbo;
  table.gl_delete_framebuffers = DeleteFbo;
  table.gl_blit_framebuffer = Blit;
  table.gl_get_error = GlError;
  return table;
}

int main() {
  State state;
  g_state = &state;
  Resources resources;
  resources.current_host = CurrentHost;
  resources.acquire_iosurface = AcquireIosurface;
  resources.release_iosurface = ReleaseIosurface;
  resources.set_embedded_extent = SetExtent;
  resources.publish_embedded = Publish;
  resources.reset_composition = Reset;
  Backend backend(MakeTable(), resources);
  void* display = reinterpret_cast<void*>(0x100);
  void* config = reinterpret_cast<void*>(0x101);
  std::int32_t major = 0;
  std::int32_t minor = 0;
  assert(backend.Initialize(display, &major, &minor));
  assert(major == 1 && minor == 5 && state.initialize == 1);

  const auto uninitialized = backend.CreateWindowWithError(
      reinterpret_cast<void*>(0xdead), config, nullptr);
  assert(uninitialized.surface == nullptr &&
         uninitialized.error ==
             darwin_art::graphics::EglWindowCreationError::kUninitializedDisplay);
  assert(darwin_art::graphics::PeekEglError() ==
         darwin_art::graphics::kEglNotInitialized);
  assert(darwin_art::graphics::ConsumeEglError() ==
         darwin_art::graphics::kEglNotInitialized);
  void* failed_display = reinterpret_cast<void*>(0x110);
  major = 41;
  minor = 42;
  state.initialize_failure = true;
  assert(!backend.Initialize(failed_display, &major, &minor));
  assert(major == 41 && minor == 42);
  state.initialize_failure = false;
  assert(backend.Initialize(failed_display, &major, &minor));

  void* surface = backend.CreateWindow(display, config, nullptr);
  assert(surface != nullptr);
  assert(backend.Swap(display, surface));
  assert(state.swap == 1 && state.gen_texture == 1);
  assert(state.active_texture == 0x84C3 && state.texture_2d == 31);
  assert(state.read_framebuffer == 41 && state.draw_framebuffer == 42);
  assert(state.scissor[0] == 3 && state.scissor[1] == 4 &&
         state.scissor[2] == 300 && state.scissor[3] == 200 &&
         state.scissor_enabled);
  state.iosurface_serial = 1;
  assert(backend.Swap(display, surface));
  assert(state.swap == 2);
  assert(backend.Destroy(display, surface));

  // A failed destination FBO rolls back all intermediate GL objects, while a
  // later swap may retry the same owner entry.
  void* retry_surface = backend.CreateWindow(display, config, nullptr);
  const int publish_before_incomplete = state.publish;
  state.complete_fbo = false;
  assert(backend.Swap(display, retry_surface));
  assert(state.publish == publish_before_incomplete);
  assert(state.del_texture == 3 && state.del_fbo == 3);
  state.complete_fbo = true;
  assert(backend.Swap(display, retry_surface));
  const int publish_before_swap_failure = state.publish;
  state.swap_failure = true;
  assert(!backend.Swap(display, retry_surface));
  assert(state.publish == publish_before_swap_failure);
  assert(state.active_texture == 0x84C3 && state.texture_2d == 31);
  assert(state.read_framebuffer == 41 && state.draw_framebuffer == 42);
  assert(state.scissor[0] == 3 && state.scissor[1] == 4 &&
         state.scissor[2] == 300 && state.scissor[3] == 200 &&
         state.scissor_enabled);
  state.swap_failure = false;
  assert(backend.Destroy(display, retry_surface));

  // Owner publication allocation failure invokes the cleanup callback exactly
  // once. Window creation fails honestly rather than returning an untracked
  // pbuffer whose swaps cannot submit a frame.
  const int destroy_before_oom = state.destroy;
  const int release_before_oom = state.release_iosurface;
  fail_next_allocation = true;
  void* oom_surface = backend.CreateWindow(display, config, nullptr);
  assert(oom_surface == nullptr);
  assert(state.destroy == destroy_before_oom + 2);
  assert(state.release_iosurface == release_before_oom + 1);
  assert(state.destroy == destroy_before_oom + 2);

  // Unknown ordinary pbuffers remain ANGLE-owned and use the facade table.
  void* ordinary = Pbuffer(display, config, nullptr);
  assert(backend.Swap(display, ordinary));
  assert(backend.Destroy(display, ordinary));

  // Completion is mandatory for embedded publication.  A backend lacking a
  // wait hook, or one whose wait reports failure, must leave the frame
  // unpublished even when the raw EGL swap succeeded.
  Table no_wait_table = MakeTable();
  no_wait_table.wait_gl = nullptr;
  Backend no_wait_backend(no_wait_table, resources);
  void* no_wait_display = reinterpret_cast<void*>(0x120);
  assert(no_wait_backend.Initialize(no_wait_display, &major, &minor));
  void* no_wait_surface =
      no_wait_backend.CreateWindow(no_wait_display, config, nullptr);
  const int publish_before_no_wait = state.publish;
  assert(!no_wait_backend.Swap(no_wait_display, no_wait_surface));
  assert(state.publish == publish_before_no_wait);
  assert(no_wait_backend.Destroy(no_wait_display, no_wait_surface));
  assert(no_wait_backend.Terminate(no_wait_display));

  state.wait_gl_failure = true;
  void* failed_wait_surface = backend.CreateWindow(display, config, nullptr);
  const int publish_before_failed_wait = state.publish;
  assert(!backend.Swap(display, failed_wait_surface));
  assert(state.publish == publish_before_failed_wait);
  assert(backend.Destroy(display, failed_wait_surface));
  state.wait_gl_failure = false;

  // Native-window transfers are queued only after a completed GPU transfer;
  // they never publish the same IOSurface through the embedded-host path.
  Resources native_resources = resources;
  native_resources.native_acquire = NativeAcquire;
  native_resources.native_release = NativeRelease;
  native_resources.native_dequeue = NativeDequeue;
  native_resources.native_queue = NativeQueue;
  native_resources.native_cancel = NativeCancel;
  native_resources.describe_buffer = DescribeBuffer;
  native_resources.buffer_iosurface = BufferIosurface;
  Backend native_backend(MakeTable(), native_resources);
  void* native_window = reinterpret_cast<void*>(0x777);
  const int native_destroy_before_oom = state.destroy;
  const int native_cancel_before_oom = state.native_cancel;
  const int native_release_before_oom = state.native_release;
  fail_next_allocation = true;
  const auto native_oom =
      native_backend.CreateWindowWithError(display, config, native_window);
  assert(native_oom.surface == nullptr &&
         native_oom.error ==
             darwin_art::graphics::EglWindowCreationError::kResourceAllocation);
  assert(state.destroy == native_destroy_before_oom + 2);
  assert(state.native_cancel == native_cancel_before_oom + 1);
  assert(state.native_release == native_release_before_oom + 1);
  const int surfaces_before_dequeue_failure = state.next_surface;
  state.native_dequeue_failure = true;
  const auto invalid_native =
      native_backend.CreateWindowWithError(display, config, native_window);
  assert(invalid_native.surface == nullptr &&
         invalid_native.error ==
             darwin_art::graphics::EglWindowCreationError::kInvalidNativeWindow);
  assert(darwin_art::graphics::ConsumeEglError() ==
         darwin_art::graphics::kEglBadNativeWindow);
  assert(state.next_surface == surfaces_before_dequeue_failure);
  state.native_dequeue_failure = false;

  // The same completion rule applies to native-window queueing: neither a
  // null wait hook nor a failed wait may queue the admitted buffer.
  Backend no_wait_native_backend(no_wait_table, native_resources);
  void* no_wait_native_display = reinterpret_cast<void*>(0x130);
  assert(no_wait_native_backend.Initialize(no_wait_native_display, &major,
                                           &minor));
  void* no_wait_native_surface = no_wait_native_backend.CreateWindow(
      no_wait_native_display, config, native_window);
  const int queue_before_no_wait = state.native_queue;
  assert(!no_wait_native_backend.Swap(no_wait_native_display,
                                      no_wait_native_surface));
  assert(state.native_queue == queue_before_no_wait);
  assert(no_wait_native_backend.Destroy(no_wait_native_display,
                                        no_wait_native_surface));
  assert(no_wait_native_backend.Terminate(no_wait_native_display));

  state.wait_gl_failure = true;
  void* failed_native_surface =
      native_backend.CreateWindow(display, config, native_window);
  const int queue_before_failed_wait = state.native_queue;
  assert(!native_backend.Swap(display, failed_native_surface));
  assert(state.native_queue == queue_before_failed_wait);
  assert(native_backend.Destroy(display, failed_native_surface));
  state.wait_gl_failure = false;

  const int publish_before_native = state.publish;
  void* native_surface =
      native_backend.CreateWindow(display, config, native_window);
  assert(native_surface != nullptr);
  assert(native_backend.Swap(display, native_surface));
  assert(state.native_queue == 1);
  assert(state.publish == publish_before_native);
  assert(native_backend.Destroy(display, native_surface));
  assert(state.native_acquire == 4 && state.native_release == 4);

  // Provider errors are captured before cleanup can invoke another provider
  // entry point. Diagnostics peek at the latch and therefore do not consume
  // the caller-visible error.
  state.track_provider_cleanup_order = true;
  state.provider_error_reads = 0;
  state.provider_cleanup_raced_error = false;
  state.render_pbuffer_failure = true;
  const auto provider_failure =
      backend.CreateWindowWithError(display, config, nullptr);
  state.render_pbuffer_failure = false;
  assert(provider_failure.surface == nullptr &&
         provider_failure.error ==
             darwin_art::graphics::EglWindowCreationError::kProviderFailure);
  assert(state.provider_error_reads == 1);
  assert(!state.provider_cleanup_raced_error);
  assert(darwin_art::graphics::PeekEglError() == state.provider_error);
  assert(darwin_art::graphics::PeekEglError() == state.provider_error);
  assert(darwin_art::graphics::ConsumeEglError() == state.provider_error);
  state.track_provider_cleanup_order = false;

  // Retire waits for an already admitted owner lease before terminating the
  // display; termination failure is observable and not converted to success.
  void* deferred_surface = backend.CreateWindow(display, config, nullptr);
  darwin_art::graphics::EglWindowSurfaceAdmission admission;
  auto held = Owner::Instance().Acquire(display, deferred_surface, &admission);
  assert(held && admission ==
                     darwin_art::graphics::EglWindowSurfaceAdmission::kAdmitted);
  const int terminate_before_deferred = state.terminate;
  assert(backend.Terminate(display));
  assert(state.terminate == terminate_before_deferred);
  held = {};
  assert(state.terminate == terminate_before_deferred + 1);
  assert(backend.ReleaseThread());
  std::puts("egl-window-backend: PASS initialization/owner/swap/retire");
  return 0;
}
