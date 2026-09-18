#include "composition_consumer.h"

#include "../darwin_surface_bridge.h"
#include "../surfaceflinger/service_darwin.h"

#include <algorithm>
#include <array>
#include <atomic>
#include <cerrno>
#include <cstdlib>
#include <cstring>
#include <iostream>
#include <memory>
#include <new>
#include <string>
#include <unistd.h>
#include <utility>
#include <vector>

namespace darwin_art::graphics {
namespace {

constexpr EglInt kEglDeviceExt = 0x322C;
constexpr EglInt kEglMetalDeviceAngle = 0x34A6;
constexpr EglInt kGlScissorTest = 0x0C11;
constexpr EglInt kGlDrawFramebufferBinding = 0x8CA6;
constexpr EglInt kGlColorBufferBit = 0x00004000;
constexpr EglInt kEglNoNativeFenceFdAndroid = -1;

struct ReleaseTarget {
  void (*release)(void*) = nullptr;
  void operator()(void* value) const {
    if (value != nullptr && release != nullptr) release(value);
  }
};

struct CompositionTarget {
  std::uint32_t surface_id = 0;
  EglDisplay display = nullptr;
  std::unique_ptr<void, ReleaseTarget> iosurface;
  void* metal_device = nullptr;
  std::uint32_t framebuffer = 0;
  std::uint32_t width = 0;
  std::uint32_t height = 0;
  bool bound = false;
  bool has_content = false;
};

thread_local CompositionTarget g_target;
thread_local std::vector<DarwinArtMetalComposerLayer> g_layers;
thread_local std::vector<CompositionBufferLease> g_leases;
thread_local std::uint64_t g_transaction_id = 0;
thread_local bool g_turn_active = false;
// A layer is only externally visible after its backing lease has been
// retained in g_leases.  Keep preparation separate from the vectors so an
// allocation/lease failure can invalidate the complete turn without making a
// partially prepared batch look submit-ready.
thread_local bool g_submission_prepared = true;

struct ContextScope {
  EglDisplay activated_display = nullptr;
  EglDisplay previous_display = nullptr;
  EglContext previous_context = nullptr;
  EglSurface previous_draw_surface = nullptr;
  EglSurface previous_read_surface = nullptr;
  bool switched = false;
};

thread_local ContextScope g_context_scope;

void ClearSubmission() {
  g_layers.clear();
  g_leases.clear();
  g_submission_prepared = true;
}

void InvalidateSubmission() {
  g_submission_prepared = false;
  g_layers.clear();
  g_leases.clear();
}

bool AppendPreparedLayer(DarwinArtMetalComposerLayer layer,
                         CompositionBufferLease&& lease) {
  try {
    // Reserve both vectors before publishing the layer.  The lease is added
    // first so every published layer has a retained IOSurface for the entire
    // lifetime of the batch.
    g_leases.reserve(g_leases.size() + 1);
    g_layers.reserve(g_layers.size() + 1);
    g_leases.push_back(std::move(lease));
    g_layers.push_back(layer);
    return true;
  } catch (const std::bad_alloc&) {
    InvalidateSubmission();
    return false;
  }
}

bool AppendPreparedState(DarwinArtMetalComposerLayer layer) {
  try {
    g_layers.reserve(g_layers.size() + 1);
    g_layers.push_back(layer);
    return true;
  } catch (const std::bad_alloc&) {
    InvalidateSubmission();
    return false;
  }
}

void TraceTargetFailure(const CompositionConsumerBackend& backend,
                        int kind, EglDisplay display,
                        std::uint32_t surface_id) {
  if (std::getenv("DARWIN_ART_TRACE_COMPOSER_FAILURES") == nullptr) return;
  static std::atomic<std::uint32_t> emitted[3] = {};
  if (kind < 0 || kind >= 3 ||
      emitted[kind].fetch_add(1, std::memory_order_relaxed) >= 4) {
    return;
  }
  const char* reason = kind == 0
                           ? "missing-or-invalid-iosurface-id"
                           : kind == 1 ? "iosurface-lookup-failed"
                                       : "metal-device-query-failed";
  std::cerr << "ART Metal Composer: target setup failed reason=" << reason
            << " display=" << display << " surface_id=" << surface_id
            << "\n";
  (void)backend;
}

bool EnsureTarget(const CompositionConsumerBackend& backend,
                  EglDisplay display) {
  const char* encoded = std::getenv("DARWIN_ART_HOST_IOSURFACE_ID");
  if (encoded == nullptr || encoded[0] == '\0') {
    TraceTargetFailure(backend, 0, display, 0);
    return false;
  }
  char* end = nullptr;
  const unsigned long parsed = std::strtoul(encoded, &end, 10);
  if (end == encoded || *end != '\0' || parsed == 0 || parsed > UINT32_MAX) {
    TraceTargetFailure(backend, 0, display, 0);
    return false;
  }
  if (g_target.bound && g_target.display == display &&
      g_target.surface_id == static_cast<std::uint32_t>(parsed) &&
      g_target.iosurface != nullptr && g_target.metal_device != nullptr) {
    return true;
  }
  g_target = {};
  void* iosurface = nullptr;
  std::uint32_t width = 0;
  std::uint32_t height = 0;
  if (backend.release_iosurface == nullptr || backend.lookup_iosurface == nullptr ||
      !backend.lookup_iosurface(static_cast<std::uint32_t>(parsed), &iosurface,
                                &width, &height)) {
    TraceTargetFailure(backend, 1, display, static_cast<std::uint32_t>(parsed));
    return false;
  }
  using QueryDisplayAttrib = EglBoolean (*)(EglDisplay, EglInt, EglAttrib*);
  using QueryDeviceAttrib = EglBoolean (*)(void*, EglInt, EglAttrib*);
  if (backend.get_proc_address == nullptr) {
    if (backend.release_iosurface != nullptr) backend.release_iosurface(iosurface);
    return false;
  }
  auto query_display_attrib = reinterpret_cast<QueryDisplayAttrib>(
      backend.get_proc_address("eglQueryDisplayAttribEXT"));
  auto query_device_attrib = reinterpret_cast<QueryDeviceAttrib>(
      backend.get_proc_address("eglQueryDeviceAttribEXT"));
  EglAttrib egl_device = 0;
  EglAttrib metal_device = 0;
  if (query_display_attrib == nullptr || query_device_attrib == nullptr ||
      query_display_attrib(display, kEglDeviceExt, &egl_device) == 0 ||
      query_device_attrib(reinterpret_cast<void*>(egl_device),
                          kEglMetalDeviceAngle, &metal_device) == 0 ||
      metal_device == 0) {
    TraceTargetFailure(backend, 2, display, static_cast<std::uint32_t>(parsed));
    if (backend.release_iosurface != nullptr) backend.release_iosurface(iosurface);
    return false;
  }
  g_target = CompositionTarget{
      .surface_id = static_cast<std::uint32_t>(parsed),
      .display = display,
      .iosurface = std::unique_ptr<void, ReleaseTarget>(
          iosurface, ReleaseTarget{backend.release_iosurface}),
      .metal_device = reinterpret_cast<void*>(metal_device),
      .width = width,
      .height = height,
      .bound = true,
  };
  if (backend.debug) {
    std::cerr << "ART Metal Composer: target IOSurface=" << g_target.surface_id
              << " size=" << width << "x" << height
              << " device=" << g_target.metal_device << "\n";
  }
  return true;
}

bool RestoreContext(const CompositionConsumerBackend& backend) {
  if (!g_context_scope.switched) {
    g_context_scope = {};
    return true;
  }
  const EglDisplay restore_display =
      g_context_scope.previous_display == nullptr
          ? g_context_scope.activated_display
          : g_context_scope.previous_display;
  if (backend.make_current == nullptr || DispatchMakeCurrent(
        restore_display, g_context_scope.previous_draw_surface,
        g_context_scope.previous_read_surface, g_context_scope.previous_context,
        backend.make_current, false) == 0) {
    std::cerr << "ART Metal Composer: EGL context restoration failed\n";
    return false;
  }
  g_context_scope = {};
  return true;
}

}  // namespace

bool IsSubmissionPrepared() noexcept { return g_submission_prepared; }

bool BeginComposition(const CompositionConsumerBackend& backend, void* opaque,
                      bool clear, std::uint64_t transaction_id) {
  // Do not discard the first turn's leases or saved EGL binding on nesting.
  if (g_turn_active) return false;
  // A failed prior restoration may still leave our producer context current.
  // Recover it before sampling the caller's binding for this new turn.
  if (!RestoreContext(backend)) return false;
  auto* buffer = static_cast<AHardwareBuffer*>(opaque);
  const EglDisplay display = backend.get_current_display == nullptr
                                 ? nullptr
                                 : backend.get_current_display();
  const EglContext context = backend.get_current_context == nullptr
                                 ? nullptr
                                 : backend.get_current_context();
  ClearSubmission();
  EglDisplay active_display = display;
  EglContext active_context = context;
  if (display == nullptr || context == nullptr) {
    const auto binding = SnapshotProducerBindingForBuffer(buffer);
    if (!binding) {
      if (backend.debug) {
        std::cerr << "ART Android SurfaceControl: no producer context pid="
                  << getpid() << " buffer=" << buffer
                  << " current_display=" << display
                  << " current_context=" << context << "\n";
      }
      g_transaction_id = 0;
      return false;
    }
    g_context_scope.activated_display = binding->display;
    g_context_scope.previous_display = display;
    g_context_scope.previous_context = context;
    g_context_scope.previous_draw_surface =
        backend.get_current_surface == nullptr
            ? nullptr
            : backend.get_current_surface(0x3059);
    g_context_scope.previous_read_surface =
        backend.get_current_surface == nullptr
            ? nullptr
            : backend.get_current_surface(0x305A);
    if (backend.make_current == nullptr ||
        DispatchMakeCurrent(binding->display, binding->draw_surface,
                            binding->read_surface, binding->context,
                            backend.make_current, false) == 0) {
      if (backend.debug) {
        std::cerr << "ART Android SurfaceControl: producer context activation "
                     "failed pid="
                  << getpid() << " context=" << binding->context
                  << " draw=" << binding->draw_surface << " read="
                  << binding->read_surface << " error=0x" << std::hex
                  << (backend.get_error == nullptr ? 0 : backend.get_error())
                  << std::dec << "\n";
      }
      g_transaction_id = 0;
      RestoreContext(backend);
      return false;
    }
    g_context_scope.switched = true;
    active_display = binding->display;
    active_context = binding->context;
  }
  if (backend.debug) {
    std::cerr << "ART Android SurfaceControl: begin pid=" << getpid()
              << " buffer=" << buffer << " display=" << active_display
              << " context=" << active_context << " switched="
              << g_context_scope.switched << " clear=" << clear << "\n";
  }
  if (!EnsureTarget(backend, active_display)) {
    ClearSubmission();
    g_transaction_id = 0;
    g_target.has_content = false;
    RestoreContext(backend);
    return false;
  }
  ClearSubmission();
  g_transaction_id = transaction_id;
  if (!clear && g_target.has_content) {
    g_turn_active = true;
    return true;
  }
  if (backend.gl_is_enabled == nullptr || backend.gl_enable == nullptr ||
      backend.gl_get_integer_v == nullptr ||
      backend.gl_bind_framebuffer == nullptr || backend.gl_disable == nullptr ||
      backend.gl_clear_color == nullptr || backend.gl_clear == nullptr) {
    g_transaction_id = 0;
    RestoreContext(backend);
    return false;
  }
  std::int32_t previous_draw_framebuffer = 0;
  const bool scissor_enabled = backend.gl_is_enabled(kGlScissorTest) != 0;
  backend.gl_get_integer_v(kGlDrawFramebufferBinding, &previous_draw_framebuffer);
  backend.gl_bind_framebuffer(0x8CA9, g_target.framebuffer);
  backend.gl_disable(kGlScissorTest);
  backend.gl_clear_color(0.0f, 0.0f, 0.0f, 0.0f);
  backend.gl_clear(kGlColorBufferBit);
  backend.gl_bind_framebuffer(0x8CA9,
                              static_cast<std::uint32_t>(previous_draw_framebuffer));
  if (scissor_enabled) backend.gl_enable(kGlScissorTest);
  g_target.has_content = true;
  g_turn_active = true;
  return true;
}

CompositionBeginResult BeginCompositionResult(const CompositionConsumerBackend& backend,
    void* opaque, bool clear, std::uint64_t transaction_id) {
  if (g_turn_active) return {};
  const bool started = BeginComposition(backend, opaque, clear, transaction_id);
  // An unsuccessful restore deliberately retains the saved switched scope.
  // A provider must not hide this terminal failure by submitting elsewhere.
  return {started, started || !g_context_scope.switched};
}

CompositionEndResult EndCompositionResult(const CompositionConsumerBackend& backend) {
  if (!g_turn_active) return {};
  if (!IsSubmissionPrepared()) {
    ClearSubmission();
    g_transaction_id = 0;
    g_turn_active = false;
    return {-1, RestoreContext(backend),
            {DARWIN_ART_SF_COMMIT_REJECTED, ENOMEM, -1}};
  }
  DarwinArtSurfaceFlingerReceipt receipt{DARWIN_ART_SF_COMMIT_REJECTED, EIO, -1};
  const EglDisplay display = backend.get_current_display == nullptr
                                 ? nullptr
                                 : backend.get_current_display();
  CompositionProducerFence producer =
      backend.export_producer_fence == nullptr
          ? CompositionProducerFence{}
          : backend.export_producer_fence(display);
  if (producer.shared_event != nullptr && backend.get_proc_address != nullptr) {
    using Flush = void (*)();
    auto flush = reinterpret_cast<Flush>(backend.get_proc_address("glFlush"));
    if (flush != nullptr) flush();
  }
  const char* service_socket =
      std::getenv("DARWIN_ART_SURFACEFLINGER_SOCKET");
  const bool remote = service_socket != nullptr && service_socket[0] != '\0';
  const char* app_package = std::getenv("DARWIN_ART_APK_APP_PACKAGE");
  const bool application_runtime = app_package != nullptr && app_package[0] != '\0';
  bool composed = false;
  int completion_fence = kEglNoNativeFenceFdAndroid;
  void* completion_event = nullptr;
  std::uint64_t completion_value = 0;
  const std::int32_t configured_width =
      backend.host_surface_width == nullptr ? 0 : backend.host_surface_width();
  const std::int32_t configured_height =
      backend.host_surface_height == nullptr ? 0 : backend.host_surface_height();
  const std::uint32_t logical_width = configured_width > 0
                                          ? static_cast<std::uint32_t>(configured_width)
                                          : g_target.width;
  const std::uint32_t logical_height = configured_height > 0
                                           ? static_cast<std::uint32_t>(configured_height)
                                           : g_target.height;
  if (remote && producer.shared_event != nullptr && g_target.bound &&
      (backend.present_remote_receipt != nullptr || backend.present_remote != nullptr)) {
    if (backend.present_remote_receipt != nullptr) {
      receipt = backend.present_remote_receipt(
          g_target.surface_id, logical_width, logical_height, g_transaction_id,
          g_layers.data(), g_layers.size(), producer.shared_event,
          producer.signal_value);
      completion_fence = receipt.completion_fd;
    } else {
      completion_fence = backend.present_remote(
        g_target.surface_id, logical_width, logical_height, g_transaction_id,
        g_layers.data(), g_layers.size(), producer.shared_event,
        producer.signal_value);
      receipt = {completion_fence >= 0 ? DARWIN_ART_SF_COMMIT_COMMITTED :
                                            DARWIN_ART_SF_COMMIT_UNKNOWN,
                 completion_fence >= 0 ? 0 : EIO, completion_fence};
    }
    composed = receipt.disposition == DARWIN_ART_SF_COMMIT_COMMITTED &&
               receipt.error == 0 && completion_fence >= 0;
  } else if (!application_runtime && !remote && !g_layers.empty() &&
             producer.shared_event != nullptr && g_target.bound &&
             g_target.iosurface != nullptr && g_target.metal_device != nullptr &&
             backend.compose_local != nullptr) {
    composed = backend.compose_local(
        g_target.metal_device, g_target.iosurface.get(), g_target.width,
        g_target.height, g_layers.data(), g_layers.size(),
        producer.shared_event, producer.signal_value, &completion_event,
        &completion_value);
    completion_fence =
        !composed || completion_event == nullptr ||
                backend.completion_fence_fd == nullptr
            ? kEglNoNativeFenceFdAndroid
            : backend.completion_fence_fd(completion_event, completion_value);
    receipt = {composed ? DARWIN_ART_SF_COMMIT_COMMITTED : DARWIN_ART_SF_COMMIT_UNKNOWN,
               completion_fence >= 0 ? 0 : EIO, completion_fence};
  }
  if (completion_event != nullptr && backend.release_shared_event != nullptr)
    backend.release_shared_event(completion_event);
  if (composed && completion_fence >= 0 && backend.track_completion_fence != nullptr)
    (void)backend.track_completion_fence(completion_fence);
  if (producer.token != nullptr && backend.release_producer_fence != nullptr)
    backend.release_producer_fence(display, producer.token);
  if (backend.debug) {
    std::cerr << "ART Metal Composer: submit layers=" << g_layers.size()
              << " composed=" << composed << " remote=" << remote
              << " producer_value=" << producer.signal_value
              << " completion_value=" << completion_value
              << " fence=" << completion_fence << "\n";
  }
  ClearSubmission();
  g_transaction_id = 0;
  g_turn_active = false;
  return {completion_fence, RestoreContext(backend), receipt};
}

int EndComposition(const CompositionConsumerBackend& backend) {
  const auto result = EndCompositionResult(backend);
  if (!result.context_restored) {
    if (result.present_fence >= 0 && backend.close_completion_fence != nullptr)
      backend.close_completion_fence(result.present_fence);
    return -1;
  }
  return result.present_fence;
}

void SetCompositionActive(const CompositionConsumerBackend& backend, bool active) {
  if (g_target.iosurface != nullptr && backend.set_composition_active != nullptr)
    backend.set_composition_active(g_target.iosurface.get(), active);
}

bool ResetComposition(const CompositionConsumerBackend& backend,
                      EglDisplay display) {
  // Display/session teardown is thread-affine, like EGL context activation.
  if (display != nullptr && g_target.display != display &&
      g_context_scope.activated_display != display) return true;
  if (!RestoreContext(backend)) return false;
  ClearSubmission();
  g_transaction_id = 0;
  g_turn_active = false;
  g_target = {};
  return true;
}

void PresentHardwareBuffer(
    const CompositionConsumerBackend& backend, void* queue,
    std::uint32_t owner_process_id, std::uint32_t layer_id,
    std::uint32_t parent_owner_process_id, std::uint32_t parent_id,
    std::uint64_t what, std::uint32_t relative_parent_owner_process_id,
    std::uint32_t relative_parent_id, std::int32_t z, void* opaque,
    std::uint32_t transform, std::int32_t source_left, std::int32_t source_top,
    std::int32_t source_right, std::int32_t source_bottom,
    std::int32_t destination_left, std::int32_t destination_top,
    std::int32_t destination_right, std::int32_t destination_bottom,
    bool has_damage, std::int32_t damage_left, std::int32_t damage_top,
    std::int32_t damage_right, std::int32_t damage_bottom, float alpha) {
  if (!g_turn_active || !g_submission_prepared) return;
  auto* buffer = static_cast<AHardwareBuffer*>(opaque);
  if (buffer == nullptr) {
    InvalidateSubmission();
    return;
  }
  const char* socket = std::getenv("DARWIN_ART_SURFACEFLINGER_SOCKET");
  const bool remote = socket != nullptr && socket[0] != '\0';
  std::uint32_t width = 0;
  std::uint32_t height = 0;
  std::uint64_t usage = 0;
  void* iosurface = nullptr;
  if (remote) {
    auto lease = CompositionBufferLease::Acquire(buffer, backend.lease_ops);
    if (!lease) {
      InvalidateSubmission();
      return;
    }
    const auto& description = lease->description();
    width = description.width;
    height = description.height;
    usage = description.usage;
    iosurface = lease->iosurface();
    source_left = std::clamp(source_left, 0, static_cast<std::int32_t>(width));
    source_right = std::clamp(source_right, 0, static_cast<std::int32_t>(width));
    source_top = std::clamp(source_top, 0, static_cast<std::int32_t>(height));
    source_bottom = std::clamp(source_bottom, 0, static_cast<std::int32_t>(height));
    destination_left = std::clamp(
        destination_left, 0, static_cast<std::int32_t>(g_target.width));
    destination_right = std::clamp(
        destination_right, 0, static_cast<std::int32_t>(g_target.width));
    destination_top = std::clamp(
        destination_top, 0, static_cast<std::int32_t>(g_target.height));
    destination_bottom = std::clamp(
        destination_bottom, 0, static_cast<std::int32_t>(g_target.height));
    DarwinArtMetalComposerLayer layer{
        .owner_process_id = owner_process_id,
        .layer_id = layer_id,
        .parent_owner_process_id = parent_owner_process_id,
        .parent_id = parent_id,
        .relative_parent_owner_process_id = relative_parent_owner_process_id,
        .relative_parent_id = relative_parent_id,
        .what = what,
        .transform = transform,
        .producer_bottom_left =
            (usage & AHARDWAREBUFFER_USAGE_COMPOSER_OVERLAY) == 0,
        .iosurface = iosurface,
        .width = width,
        .height = height,
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
    };
    (void)AppendPreparedLayer(std::move(layer), std::move(*lease));
  } else {
    const auto presentation = PrepareLocalPresentation(buffer, queue);
    if (!presentation) {
      InvalidateSubmission();
      return;
    }
    auto lease = CompositionBufferLease::Acquire(presentation->buffer,
                                                 backend.lease_ops);
    if (!lease) {
      InvalidateSubmission();
      return;
    }
    width = presentation->width;
    height = presentation->height;
    usage = presentation->usage;
    iosurface = lease->iosurface();
    source_left = std::clamp(source_left, 0, static_cast<std::int32_t>(width));
    source_right = std::clamp(source_right, 0, static_cast<std::int32_t>(width));
    source_top = std::clamp(source_top, 0, static_cast<std::int32_t>(height));
    source_bottom = std::clamp(source_bottom, 0, static_cast<std::int32_t>(height));
    destination_left = std::clamp(
        destination_left, 0, static_cast<std::int32_t>(g_target.width));
    destination_right = std::clamp(
        destination_right, 0, static_cast<std::int32_t>(g_target.width));
    destination_top = std::clamp(
        destination_top, 0, static_cast<std::int32_t>(g_target.height));
    destination_bottom = std::clamp(
        destination_bottom, 0, static_cast<std::int32_t>(g_target.height));
    DarwinArtMetalComposerLayer layer{
        .owner_process_id = owner_process_id,
        .layer_id = layer_id,
        .parent_owner_process_id = parent_owner_process_id,
        .parent_id = parent_id,
        .what = what,
        .transform = transform,
        .producer_bottom_left =
            (usage & AHARDWAREBUFFER_USAGE_COMPOSER_OVERLAY) == 0,
        .iosurface = iosurface,
        .width = width,
        .height = height,
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
    };
    (void)AppendPreparedLayer(std::move(layer), std::move(*lease));
  }
  (void)has_damage;
  (void)damage_left;
  (void)damage_top;
  (void)damage_right;
  (void)damage_bottom;
}

void PresentSurfaceControlState(
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
  if (!g_turn_active || !g_submission_prepared) return;
  DarwinArtMetalComposerLayer state{
      .owner_process_id = owner_process_id,
      .layer_id = layer_id,
      .parent_owner_process_id = parent_owner_process_id,
      .parent_id = parent_id,
      .relative_parent_owner_process_id = relative_parent_owner_process_id,
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
  };
  state.transparent_region_count = std::min(
      transparent_region_rects == nullptr ? 0u : transparent_region_count,
      static_cast<std::uint32_t>(kDarwinArtMaxTransparentRegionRects));
  for (std::uint32_t index = 0; index < state.transparent_region_count; ++index) {
    const std::int32_t* source = transparent_region_rects + index * 4;
    state.transparent_region[index] = {source[0], source[1], source[2],
                                       source[3]};
  }
  (void)AppendPreparedState(std::move(state));
}

}  // namespace darwin_art::graphics
