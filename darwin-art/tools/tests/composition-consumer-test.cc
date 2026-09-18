#include "compat/graphics/composition_consumer.h"
#include "compat/surfaceflinger/commit_receipt.h"

#include <android/hardware_buffer.h>

#include <cassert>
#include <atomic>
#include <cerrno>
#include <cstdlib>
#include <cstdint>
#include <cstdio>
#include <cstring>
#include <new>
#include <optional>

namespace {

std::atomic<bool> g_fail_allocations{false};

void* AllocateOrThrow(std::size_t size) {
  if (g_fail_allocations.load(std::memory_order_relaxed))
    throw std::bad_alloc();
  if (void* result = std::malloc(size == 0 ? 1 : size)) return result;
  throw std::bad_alloc();
}

}  // namespace

void* operator new(std::size_t size) { return AllocateOrThrow(size); }
void* operator new[](std::size_t size) { return AllocateOrThrow(size); }
void operator delete(void* value) noexcept { std::free(value); }
void operator delete[](void* value) noexcept { std::free(value); }
void operator delete(void* value, std::size_t) noexcept { std::free(value); }
void operator delete[](void* value, std::size_t) noexcept { std::free(value); }

namespace {

using darwin_art::graphics::CompositionConsumerBackend;
using darwin_art::graphics::CompositionProducerFence;
using darwin_art::graphics::EglAttrib;
using darwin_art::graphics::EglBoolean;
using darwin_art::graphics::EglContext;
using darwin_art::graphics::EglDisplay;
using darwin_art::graphics::EglInt;
using darwin_art::graphics::EglSurface;
using darwin_art::graphics::LocalPresentation;
using darwin_art::graphics::SnapshotProducerBinding;

AHardwareBuffer* const kBuffer =
    reinterpret_cast<AHardwareBuffer*>(static_cast<uintptr_t>(0x101));
void* const kTargetIosurface = reinterpret_cast<void*>(0x202);
void* const kBufferIosurface = reinterpret_cast<void*>(0x303);
void* const kDisplay = reinterpret_cast<void*>(0x404);
void* const kContext = reinterpret_cast<void*>(0x505);
void* const kDrawSurface = reinterpret_cast<void*>(0x606);
void* const kReadSurface = reinterpret_cast<void*>(0x707);
void* const kEglDevice = reinterpret_cast<void*>(0x808);
void* const kMetalDevice = reinterpret_cast<void*>(0x909);
void* const kProducerToken = reinterpret_cast<void*>(0xa0a);
void* const kProducerEvent = reinterpret_cast<void*>(0xb0b);

struct MakeCurrentCall {
  EglDisplay display = nullptr;
  EglSurface draw = nullptr;
  EglSurface read = nullptr;
  EglContext context = nullptr;
};

bool g_lookup_fail = false;
bool g_lease_iosurface_fail = false;
bool g_get_proc_fail = false;
bool g_scissor_enabled = false;
int g_lookup_calls = 0;
int g_target_release_calls = 0;
int g_buffer_retain_calls = 0;
int g_buffer_release_calls = 0;
int g_producer_export_calls = 0;
int g_producer_release_calls = 0;
int g_present_calls = 0;
int g_flush_calls = 0;
int g_track_fence_calls = 0;
int g_close_completion_fence_calls = 0;
bool g_restore_fail_once = false;
int g_last_completion_fence = -1;
int g_present_result = -1;
DarwinArtSurfaceFlingerReceipt g_test_receipt{DARWIN_ART_SF_COMMIT_UNKNOWN, EIO, -1};
std::uint32_t g_last_surface_id = 0;
std::uint32_t g_last_width = 0;
std::uint32_t g_last_height = 0;
std::uint64_t g_last_transaction_id = 0;
std::size_t g_last_layer_count = 0;
void* g_current_display = kDisplay;
void* g_current_context = kContext;
EglBoolean g_make_current_result = 1;
MakeCurrentCall g_make_current_calls[8]{};
int g_make_current_call_count = 0;
std::optional<SnapshotProducerBinding> g_binding;

void ResetCallbacks() {
  g_lookup_fail = false;
  g_lease_iosurface_fail = false;
  g_get_proc_fail = false;
  g_scissor_enabled = false;
  g_lookup_calls = 0;
  g_target_release_calls = 0;
  g_buffer_retain_calls = 0;
  g_buffer_release_calls = 0;
  g_producer_export_calls = 0;
  g_producer_release_calls = 0;
  g_present_calls = 0;
  g_flush_calls = 0;
  g_track_fence_calls = 0;
  g_close_completion_fence_calls = 0;
  g_restore_fail_once = false;
  g_last_completion_fence = -1;
  g_present_result = -1;
  g_last_surface_id = 0;
  g_last_width = 0;
  g_last_height = 0;
  g_last_transaction_id = 0;
  g_last_layer_count = 0;
  g_current_display = kDisplay;
  g_current_context = kContext;
  g_make_current_result = 1;
  g_make_current_call_count = 0;
  g_binding = SnapshotProducerBinding{
      .display = kDisplay,
      .context = kContext,
      .draw_surface = kDrawSurface,
      .read_surface = kReadSurface,
  };
}

void FakeReleaseIosurface(void* iosurface) {
  assert(iosurface == kTargetIosurface);
  ++g_target_release_calls;
}

bool FakeLookupIosurface(std::uint32_t id, void** iosurface,
                         std::uint32_t* width, std::uint32_t* height) {
  assert(id != 0 && iosurface != nullptr && width != nullptr && height != nullptr);
  ++g_lookup_calls;
  if (g_lookup_fail) return false;
  *iosurface = kTargetIosurface;
  *width = 640;
  *height = 480;
  return true;
}

EglBoolean FakeQueryDisplayAttrib(EglDisplay display, EglInt attribute,
                                  EglAttrib* value) {
  assert(display == kDisplay && attribute == 0x322C && value != nullptr);
  *value = reinterpret_cast<EglAttrib>(kEglDevice);
  return 1;
}

EglBoolean FakeQueryDeviceAttrib(void* device, EglInt attribute,
                                 EglAttrib* value) {
  assert(device == kEglDevice && attribute == 0x34A6 && value != nullptr);
  *value = reinterpret_cast<EglAttrib>(kMetalDevice);
  return 1;
}

void FakeFlush() { ++g_flush_calls; }

void* FunctionPointerAsVoid(void (*function)()) {
  return reinterpret_cast<void*>(reinterpret_cast<uintptr_t>(function));
}

void* FakeGetProcAddress(const char* name) {
  assert(name != nullptr);
  if (g_get_proc_fail) return nullptr;
  if (std::strcmp(name, "eglQueryDisplayAttribEXT") == 0)
    return FunctionPointerAsVoid(reinterpret_cast<void (*)()>(
        &FakeQueryDisplayAttrib));
  if (std::strcmp(name, "eglQueryDeviceAttribEXT") == 0)
    return FunctionPointerAsVoid(reinterpret_cast<void (*)()>(
        &FakeQueryDeviceAttrib));
  if (std::strcmp(name, "glFlush") == 0) return FunctionPointerAsVoid(&FakeFlush);
  return nullptr;
}

EglDisplay FakeCurrentDisplay() { return g_current_display; }
EglContext FakeCurrentContext() { return g_current_context; }
EglSurface FakeCurrentSurface(EglInt attribute) {
  assert(attribute == 0x3059 || attribute == 0x305A);
  return attribute == 0x3059 ? kDrawSurface : kReadSurface;
}

EglBoolean FakeMakeCurrent(EglDisplay display, EglSurface draw,
                           EglSurface read, EglContext context) {
  assert(g_make_current_call_count < 8);
  g_make_current_calls[g_make_current_call_count++] = {
      .display = display, .draw = draw, .read = read, .context = context};
  const bool restore_failed = g_restore_fail_once &&
                              g_make_current_call_count > 1;
  if (restore_failed) g_restore_fail_once = false;
  const EglBoolean result = restore_failed ? 0 : g_make_current_result;
  if (result != 0) {
    g_current_context = context;
    g_current_display = context == nullptr ? nullptr : display;
  }
  return result;
}

std::uint8_t FakeGlIsEnabled(std::uint32_t capability) {
  assert(capability == 0x0C11);
  return g_scissor_enabled ? 1 : 0;
}

void FakeGlEnable(std::uint32_t capability) {
  assert(capability == 0x0C11);
  g_scissor_enabled = true;
}

void FakeGlDisable(std::uint32_t capability) {
  assert(capability == 0x0C11);
  g_scissor_enabled = false;
}

void FakeGlGetInteger(std::uint32_t value, EglInt* result) {
  assert(value == 0x8CA6 && result != nullptr);
  *result = 17;
}

void FakeGlBindFramebuffer(std::uint32_t target, std::uint32_t framebuffer) {
  assert(target == 0x8CA9);
  (void)framebuffer;
}

void FakeGlClearColor(float red, float green, float blue, float alpha) {
  assert(red == 0.0f && green == 0.0f && blue == 0.0f && alpha == 0.0f);
}

void FakeGlClear(std::uint32_t mask) { assert(mask == 0x00004000); }

CompositionProducerFence FakeExportFence(EglDisplay display) {
  assert(display == g_current_display);
  ++g_producer_export_calls;
  return {.token = kProducerToken,
          .shared_event = kProducerEvent,
          .signal_value = 19};
}

void FakeReleaseFence(EglDisplay display, void* token) {
  assert(display == g_current_display && token == kProducerToken);
  ++g_producer_release_calls;
}

int FakePresentRemote(std::uint32_t surface_id, std::uint32_t width,
                      std::uint32_t height, std::uint64_t transaction_id,
                      const DarwinArtMetalComposerLayer* layers,
                      std::size_t layer_count, void* producer_event,
                      std::uint64_t producer_value) {
  assert(surface_id != 0 && width == 640 && height == 480);
  assert(layer_count == 0 || layers != nullptr);
  assert(producer_event == kProducerEvent && producer_value == 19);
  ++g_present_calls;
  g_last_surface_id = surface_id;
  g_last_width = width;
  g_last_height = height;
  g_last_transaction_id = transaction_id;
  g_last_layer_count = layer_count;
  return g_present_result;
}

bool FakeTrackFence(int fence) {
  assert(fence == g_last_completion_fence);
  ++g_track_fence_calls;
  return true;
}

DarwinArtSurfaceFlingerReceipt FakePresentReceipt(
    std::uint32_t surface_id, std::uint32_t width, std::uint32_t height,
    std::uint64_t transaction_id, const DarwinArtMetalComposerLayer* layers,
    std::size_t layer_count, void* event, std::uint64_t value) {
  (void)FakePresentRemote(surface_id, width, height, transaction_id,
                          layers, layer_count, event, value);
  return g_test_receipt;
}

void FakeCloseCompletionFence(int fence) {
  assert(fence == g_last_completion_fence);
  ++g_close_completion_fence_calls;
}

void FakeRetainBuffer(AHardwareBuffer* buffer) {
  assert(buffer == kBuffer);
  ++g_buffer_retain_calls;
}

void FakeReleaseBuffer(AHardwareBuffer* buffer) {
  assert(buffer == kBuffer);
  ++g_buffer_release_calls;
}

void FakeDescribeBuffer(const AHardwareBuffer* buffer, AHardwareBuffer_Desc* desc) {
  assert(buffer == kBuffer && desc != nullptr);
  desc->width = 64;
  desc->height = 32;
  desc->layers = 1;
  desc->usage = 0;
  desc->format = AHARDWAREBUFFER_FORMAT_R8G8B8A8_UNORM;
}

void* FakeBufferIosurface(AHardwareBuffer* buffer) {
  assert(buffer == kBuffer);
  return g_lease_iosurface_fail ? nullptr : kBufferIosurface;
}

CompositionConsumerBackend FakeBackend() {
  CompositionConsumerBackend backend{};
  backend.get_current_display = FakeCurrentDisplay;
  backend.get_current_context = FakeCurrentContext;
  backend.get_current_surface = FakeCurrentSurface;
  backend.make_current = FakeMakeCurrent;
  backend.get_proc_address = FakeGetProcAddress;
  backend.gl_is_enabled = FakeGlIsEnabled;
  backend.gl_enable = FakeGlEnable;
  backend.gl_get_integer_v = FakeGlGetInteger;
  backend.gl_bind_framebuffer = FakeGlBindFramebuffer;
  backend.gl_disable = FakeGlDisable;
  backend.gl_clear_color = FakeGlClearColor;
  backend.gl_clear = FakeGlClear;
  backend.lookup_iosurface = FakeLookupIosurface;
  backend.release_iosurface = FakeReleaseIosurface;
  backend.export_producer_fence = FakeExportFence;
  backend.release_producer_fence = FakeReleaseFence;
  backend.present_remote = FakePresentRemote;
  backend.track_completion_fence = FakeTrackFence;
  backend.close_completion_fence = FakeCloseCompletionFence;
  backend.lease_ops = {
      .retain = FakeRetainBuffer,
      .release = FakeReleaseBuffer,
      .describe = FakeDescribeBuffer,
      .iosurface = FakeBufferIosurface,
  };
  return backend;
}

void SetSurfaceId(const char* id) {
  assert(setenv("DARWIN_ART_HOST_IOSURFACE_ID", id, 1) == 0);
}

void AddBufferLayer(const CompositionConsumerBackend& backend,
                    std::uint64_t transaction_id) {
  darwin_art::graphics::PresentSurfaceControlState(
      1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 0, 0, 640, 480, 0, 0, 1.0f, 1.0f,
      false, 0, 0, 0, 0, 1, 1.0f, nullptr, 0);
  darwin_art::graphics::PresentHardwareBuffer(
      backend, nullptr, 11, 12, 13, 14, 15, 16, 17, 2, kBuffer, 0, 0, 0,
      64, 32, -20, -20, 700, 500, false, 0, 0, 0, 0, 1.0f);
  (void)transaction_id;
}

}  // namespace

namespace darwin_art::graphics {

bool IsSubmissionPrepared() noexcept;

std::optional<SnapshotProducerBinding> SnapshotProducerBindingForBuffer(
    AHardwareBuffer* buffer) {
  if (buffer != kBuffer) return std::nullopt;
  return g_binding;
}

std::optional<LocalPresentation> PrepareLocalPresentation(
    AHardwareBuffer* buffer, void* queue) {
  (void)buffer;
  (void)queue;
  return std::nullopt;
}

}  // namespace darwin_art::graphics

int main() {
  using darwin_art::graphics::BeginComposition;
  using darwin_art::graphics::EndComposition;

  ResetCallbacks();
  const CompositionConsumerBackend backend = FakeBackend();
  assert(setenv("DARWIN_ART_SURFACEFLINGER_SOCKET", "test-socket", 1) == 0);

  // A failed lookup with no prior target does not release an input it never
  // acquired. A later target lookup failure releases the retained old target.
  SetSurfaceId("101");
  g_lookup_fail = true;
  assert(!BeginComposition(backend, kBuffer, true, 1));
  assert(g_lookup_calls == 1 && g_target_release_calls == 0);
  g_lookup_fail = false;
  assert(BeginComposition(backend, kBuffer, true, 2));
  assert(g_target_release_calls == 0);
  assert(EndComposition(backend) == -1);
  SetSurfaceId("102");
  g_lookup_fail = true;
  assert(!BeginComposition(backend, kBuffer, true, 3));
  assert(g_target_release_calls == 1);

  // A lookup succeeds before extension resolution fails; that newly acquired
  // IOSurface must be released on this failure path.
  SetSurfaceId("103");
  g_lookup_fail = false;
  g_get_proc_fail = true;
  assert(!BeginComposition(backend, kBuffer, true, 4));
  assert(g_target_release_calls == 2);
  g_get_proc_fail = false;

  // Producer activation is restored when target setup fails after the switch.
  SetSurfaceId("104");
  g_lookup_fail = true;
  g_current_display = nullptr;
  g_current_context = nullptr;
  assert(!BeginComposition(backend, kBuffer, true, 5));
  assert(g_make_current_call_count == 2);
  assert(g_make_current_calls[0].display == kDisplay &&
         g_make_current_calls[0].context == kContext);
  assert(g_make_current_calls[1].display == kDisplay &&
         g_make_current_calls[1].draw == kDrawSurface &&
         g_make_current_calls[1].read == kReadSurface &&
         g_make_current_calls[1].context == nullptr);

  // Remote rejection still releases the producer fence and every retained
  // buffer lease, and EndComposition restores a temporarily activated context.
  SetSurfaceId("105");
  g_lookup_fail = false;
  g_current_display = nullptr;
  g_current_context = nullptr;
  g_make_current_call_count = 0;
  g_present_result = -1;
  assert(BeginComposition(backend, kBuffer, true, 6));
  AddBufferLayer(backend, 6);
  assert(g_buffer_retain_calls == 1);
  assert(EndComposition(backend) == -1);
  assert(g_present_calls == 2 && g_last_transaction_id == 6 &&
         g_last_layer_count == 2);
  assert(g_producer_export_calls == 2 && g_producer_release_calls == 2);
  assert(g_buffer_release_calls == 1 && g_flush_calls == 2);
  assert(g_track_fence_calls == 0);
  assert(g_make_current_call_count == 2);
  assert(g_make_current_calls[1].display == kDisplay &&
         g_make_current_calls[1].context == nullptr);

  // A successful remote submission tracks the returned completion fence and
  // still performs the same lease/fence/context cleanup.
  g_current_display = kDisplay;
  g_current_context = kContext;
  g_present_result = 37;
  g_last_completion_fence = 37;
  assert(BeginComposition(backend, kBuffer, true, 7));
  AddBufferLayer(backend, 7);
  assert(EndComposition(backend) == 37);
  assert(g_present_calls == 3 && g_producer_export_calls == 3 &&
         g_producer_release_calls == 3);
  assert(g_buffer_retain_calls == 2 && g_buffer_release_calls == 2);
  assert(g_track_fence_calls == 1);

  // End closes a returned completion descriptor when context restoration
  // fails, and the next Begin retries restoration before re-activating the
  // producer context rather than discarding the saved binding.
  g_current_display = nullptr;
  g_current_context = nullptr;
  g_make_current_call_count = 0;
  g_restore_fail_once = true;
  g_present_result = 37;
  assert(BeginComposition(backend, kBuffer, true, 8));
  AddBufferLayer(backend, 8);
  assert(EndComposition(backend) == -1);
  assert(g_close_completion_fence_calls == 1);
  assert(g_buffer_retain_calls == 3 && g_buffer_release_calls == 3);
  assert(g_make_current_call_count == 2);
  assert(g_current_context == kContext && g_current_display == kDisplay);
  assert(BeginComposition(backend, kBuffer, true, 9));
  assert(g_make_current_call_count == 4);
  assert(g_make_current_calls[2].display == kDisplay &&
         g_make_current_calls[2].context == nullptr);
  assert(g_make_current_calls[3].display == kDisplay &&
         g_make_current_calls[3].context == kContext);
  AddBufferLayer(backend, 9);
  assert(EndComposition(backend) == 37);
  assert(g_make_current_call_count == 5);
  assert(g_buffer_retain_calls == 4 && g_buffer_release_calls == 4);

  // A reset for another display leaves the target alone. Resetting on the
  // owning display releases an active lease and the target exactly once.
  const EglDisplay other_display = reinterpret_cast<void*>(0xc0c);
  assert(ResetComposition(backend, other_display));
  assert(g_target_release_calls == 2);
  g_current_display = kDisplay;
  g_current_context = kContext;
  assert(BeginComposition(backend, kBuffer, true, 10));
  AddBufferLayer(backend, 10);
  assert(g_buffer_retain_calls == 5 && g_buffer_release_calls == 4);
  assert(ResetComposition(backend, kDisplay));
  assert(g_buffer_release_calls == 5 && g_target_release_calls == 3);

  // Nested Begin is rejected without replacing the first transaction's
  // retained layers; the first transaction is the one submitted by End.
  SetSurfaceId("106");
  assert(BeginComposition(backend, kBuffer, true, 11));
  assert(!BeginComposition(backend, kBuffer, true, 12));
  AddBufferLayer(backend, 11);
  assert(EndComposition(backend) == 37);
  assert(g_last_transaction_id == 11);
  assert(g_buffer_retain_calls == 6 && g_buffer_release_calls == 6);
  assert(ResetComposition(backend, kDisplay));
  assert(g_target_release_calls == 4);

  // A required lease failure is sticky for the whole turn.  Acquire retains
  // before discovering the invalid IOSurface, and the failed turn must not
  // export a producer fence or issue a remote request for a partial batch.
  SetSurfaceId("107");
  assert(BeginComposition(backend, kBuffer, true, 15));
  const int present_before_lease_failure = g_present_calls;
  const int export_before_lease_failure = g_producer_export_calls;
  g_lease_iosurface_fail = true;
  darwin_art::graphics::PresentHardwareBuffer(
      backend, nullptr, 21, 22, 23, 24, 25, 26, 27, 2, kBuffer, 0, 0, 0,
      64, 32, 0, 0, 64, 32, false, 0, 0, 0, 0, 1.0f);
  g_lease_iosurface_fail = false;
  assert(!darwin_art::graphics::IsSubmissionPrepared());
  const auto lease_rejected = darwin_art::graphics::EndCompositionResult(backend);
  assert(lease_rejected.context_restored && lease_rejected.present_fence == -1);
  assert(lease_rejected.receipt.disposition == DARWIN_ART_SF_COMMIT_REJECTED &&
         lease_rejected.receipt.error == ENOMEM);
  assert(g_present_calls == present_before_lease_failure &&
         g_producer_export_calls == export_before_lease_failure);
  assert(g_buffer_retain_calls == 7 && g_buffer_release_calls == 7);

  // A vector allocation failure has the same all-or-nothing contract for a
  // structural state append.  Keep the failure enabled until an append must
  // grow the actual production vector, then clear it before End's cleanup.
  assert(BeginComposition(backend, kBuffer, true, 16));
  darwin_art::graphics::PresentHardwareBuffer(
      backend, nullptr, 41, 42, 43, 44, 45, 46, 47, 2, kBuffer, 0, 0, 0,
      64, 32, 0, 0, 64, 32, false, 0, 0, 0, 0, 1.0f);
  const int retains_before_vector_failure = g_buffer_retain_calls;
  const int present_before_vector_failure = g_present_calls;
  const int export_before_vector_failure = g_producer_export_calls;
  g_fail_allocations.store(true, std::memory_order_relaxed);
  std::size_t attempts = 0;
  while (darwin_art::graphics::IsSubmissionPrepared() && attempts++ < 4096) {
    darwin_art::graphics::PresentSurfaceControlState(
        31, static_cast<std::uint32_t>(attempts), 33, 34, 35, 36, 37, 38,
        39, 40, 0, 0, 640, 480, 0, 0, 1.0f, 1.0f, false, 0, 0, 0, 0,
        1, 1.0f, nullptr, 0);
  }
  g_fail_allocations.store(false, std::memory_order_relaxed);
  assert(!darwin_art::graphics::IsSubmissionPrepared());
  const auto vector_rejected = darwin_art::graphics::EndCompositionResult(backend);
  assert(vector_rejected.context_restored && vector_rejected.present_fence == -1);
  assert(vector_rejected.receipt.disposition == DARWIN_ART_SF_COMMIT_REJECTED &&
         vector_rejected.receipt.error == ENOMEM);
  assert(g_present_calls == present_before_vector_failure &&
         g_producer_export_calls == export_before_vector_failure);
  assert(g_buffer_retain_calls == retains_before_vector_failure &&
         g_buffer_release_calls == retains_before_vector_failure);
  assert(ResetComposition(backend, kDisplay));

  // A pending restore failure is terminal, unlike an ordinary unavailable
  // producer context. The typed begin result must not permit central retry.
  g_current_display = nullptr;
  g_current_context = nullptr;
  g_make_current_call_count = 0;
  g_restore_fail_once = true;
  const auto begun = BeginCompositionResult(backend, kBuffer, true, 13);
  assert(begun.started && begun.retry_safe);
  const auto ended = EndCompositionResult(backend);
  assert(!ended.context_restored && ended.present_fence == 37);
  assert(ended.receipt.disposition == DARWIN_ART_SF_COMMIT_COMMITTED &&
         ended.receipt.completion_fd == 37);
  FakeCloseCompletionFence(ended.present_fence);
  g_restore_fail_once = true;
  const auto retry = BeginCompositionResult(backend, kBuffer, true, 14);
  assert(!retry.started && !retry.retry_safe);
  assert(ResetComposition(backend));
  auto typed_backend = backend;
  typed_backend.present_remote_receipt = FakePresentReceipt;
  g_current_display = kDisplay;
  g_current_context = kContext;
  SetSurfaceId("108");
  for (const DarwinArtSurfaceFlingerReceipt receipt : {
           DarwinArtSurfaceFlingerReceipt{DARWIN_ART_SF_COMMIT_REJECTED, EBUSY, -1},
           DarwinArtSurfaceFlingerReceipt{DARWIN_ART_SF_COMMIT_UNKNOWN, EIO, -1},
           DarwinArtSurfaceFlingerReceipt{DARWIN_ART_SF_COMMIT_COMMITTED, EPROTO, -1}}) {
    g_test_receipt = receipt;
    const int before = g_present_calls;
    assert(BeginComposition(typed_backend, kBuffer, true, 20));
    const auto result = EndCompositionResult(typed_backend);
    assert(result.context_restored && result.present_fence == -1);
    assert(result.receipt.disposition == receipt.disposition &&
           result.receipt.error == receipt.error);
    assert(g_present_calls == before + 1);
    (void)EndCompositionResult(typed_backend);
    assert(g_present_calls == before + 1);
    assert(ResetComposition(typed_backend));
  }
  unsetenv("DARWIN_ART_HOST_IOSURFACE_ID");
  unsetenv("DARWIN_ART_SURFACEFLINGER_SOCKET");
  std::puts("composition consumer actual TU: target/fence/lease/context failure and success cleanup PASS");
}
