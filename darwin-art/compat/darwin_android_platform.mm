#include "darwin_android_platform.h"
#include "graphics/hardware_buffer_owner.h"
#include "graphics/metal_shared_event_provider.h"
#include "window/surface_control_jni.h"
#include "memory/shared_memory.h"
#include "darwin_android_time.h"
#include "darwin_surface_bridge.h"
#include "surfaceflinger/transaction_bridge.h"
#include "surfaceflinger/service_darwin.h"
#include "darwin_angle_egl.h"
#include "darwin_art_bionic_socket_broker.h"
#include "window/surface_transaction_merge.h"
#include "window/surface_transaction_lifetime.h"
#include "window/surface_transaction_builder.h"
#include "window/surface_transaction_submission.h"
#include "window/surface_control_state.h"
#include "window/surface_control_registry.h"
#include "window/surface_control_submit_darwin.h"

#import <IOSurface/IOSurface.h>

#include <android/hardware_buffer.h>
#include <android/hardware_buffer_jni.h>
#include <android/choreographer.h>
#include <android/input.h>
#include <android/looper.h>
#include <android/native_window.h>
#include <android/native_window_jni.h>
#include <android/sensor.h>
#include <android/surface_control.h>

#include <algorithm>
#include <array>
#include <atomic>
#include <chrono>
#include <climits>
#include <cerrno>
#include <cstdlib>
#include <cstdio>
#include <cstring>
#include <condition_variable>
#include <deque>
#include <dlfcn.h>
#include <fcntl.h>
#include <limits>
#include <memory>
#include <mutex>
#include <new>
#include <string>
#include <sys/mman.h>
#include <sys/xattr.h>
#include <time.h>
#include <unistd.h>
#include <thread>
#include <unordered_map>
#include <vector>

extern "C" int darwin_art_bionic_errno_set_from_darwin(int error);
extern "C" int darwin_art_bionic_socket_broker_close(int fd);

struct AInputEvent {
  uint32_t magic = 0x44414945u;
  std::atomic<uint32_t> references{1};
  int32_t type = AINPUT_EVENT_TYPE_MOTION;
  int32_t source = AINPUT_SOURCE_TOUCHSCREEN;
  int32_t action = 0;
  int32_t meta_state = 0;
  int32_t button_state = 0;
  int32_t classification = 0;
  int64_t down_time = 0;
  int64_t event_time = 0;
  int32_t pointer_id = 0;
  int32_t tool_type = AMOTION_EVENT_TOOL_TYPE_FINGER;
  float x = 0;
  float y = 0;
  float raw_x = 0;
  float raw_y = 0;
  float pressure = 1;
  float touch_major = 1;
  float touch_minor = 1;
  float orientation = 0;
};
namespace {

using SurfaceTransaction = darwin_art::window::SurfaceTransaction;
using SurfaceTransactionStats = darwin_art::window::SurfaceTransactionStats;
using SurfaceTransactionBuilder =
    darwin_art::window::SurfaceTransactionBuilder;

extern "C" void* darwin_art_android_surface_control_create_root(
    const char* name) {
  return darwin_art::window::SurfaceControlRegistry::Instance().Create(
      nullptr, name, true);
}

extern "C" void* darwin_art_android_surface_control_create_imported(
    uint32_t owner_process_id, uint32_t layer_id, const char* name) {
  if (owner_process_id == 0 || layer_id == 0) return nullptr;
  return darwin_art::window::SurfaceControlRegistry::Instance().Create(
      nullptr, name, true, owner_process_id, layer_id);
}

extern "C" bool darwin_art_android_surface_control_get_identity(
    void* opaque, uint32_t* owner_process_id, uint32_t* layer_id) {
  return darwin_art::window::SurfaceControlRegistry::Instance().GetIdentity(
      static_cast<const ASurfaceControl*>(opaque), owner_process_id, layer_id);
}

bool DebugSurfaceTransactions() {
  static const bool enabled =
      std::getenv("DARWIN_ART_DEBUG_SURFACE_TRANSACTIONS") != nullptr;
  return enabled;
}

bool TraceReparentToNull() {
  static const bool enabled =
      std::getenv("DARWIN_ART_TRACE_REPARENT_NULL") != nullptr;
  return enabled;
}

uint32_t HostTargetSurfaceIdForTrace() {
  const char* encoded_target = std::getenv("DARWIN_ART_HOST_IOSURFACE_ID");
  if (encoded_target == nullptr || encoded_target[0] == '\0') return 0;
  char* end = nullptr;
  const unsigned long parsed = std::strtoul(encoded_target, &end, 10);
  return end != encoded_target && *end == '\0' && parsed > 0 &&
          parsed <= UINT32_MAX
      ? static_cast<uint32_t>(parsed)
      : 0;
}

void NoopServiceCallback(void*) {}

// The public NDK setter is void, but a valid transaction/control pair cannot
// silently lose an ownership mutation.  Null opaque handles preserve the
// compatibility layer's legacy no-op contract; allocation/resource failure
// on a valid operation is a provider bug and follows the existing fatal
// policy.  Native-window submission uses the checked entry point below so it
// can reject the apply before installing completion callbacks.
bool RequireSurfaceTransactionBuilderResult(
    bool accepted, SurfaceTransaction* transaction, ASurfaceControl* control) {
  if (accepted || transaction == nullptr || control == nullptr) return accepted;
  std::fprintf(stderr,
               "ART Android SurfaceTransaction: builder resource failure\n");
  std::abort();
}

}  // namespace

namespace {
const AInputEvent* Input(const AInputEvent* event) {
  return event != nullptr && event->magic == 0x44414945u ? event : nullptr;
}
float MotionAxis(const AInputEvent* event, int32_t axis) {
  event = Input(event);
  if (event == nullptr) return 0;
  switch (axis) {
    case AMOTION_EVENT_AXIS_X: return event->x;
    case AMOTION_EVENT_AXIS_Y: return event->y;
    case AMOTION_EVENT_AXIS_PRESSURE: return event->pressure;
    case AMOTION_EVENT_AXIS_TOUCH_MAJOR: return event->touch_major;
    case AMOTION_EVENT_AXIS_TOUCH_MINOR: return event->touch_minor;
    case AMOTION_EVENT_AXIS_ORIENTATION: return event->orientation;
    default: return 0;
  }
}

}  // namespace

extern "C" int32_t AInputEvent_getType(const AInputEvent* event) {
  event = Input(event);
  return event == nullptr ? 0 : event->type;
}
extern "C" int32_t AInputEvent_getSource(const AInputEvent* event) {
  event = Input(event);
  return event == nullptr ? 0 : event->source;
}
extern "C" void AInputEvent_release(const AInputEvent* event) {
  auto* owned = const_cast<AInputEvent*>(Input(event));
  if (owned != nullptr &&
      owned->references.fetch_sub(1, std::memory_order_acq_rel) == 1)
    delete owned;
}
extern "C" int32_t AMotionEvent_getAction(const AInputEvent* event) {
  event = Input(event);
  return event == nullptr ? 0 : event->action;
}
extern "C" int32_t AMotionEvent_getMetaState(const AInputEvent* event) {
  event = Input(event);
  return event == nullptr ? 0 : event->meta_state;
}
extern "C" int32_t AMotionEvent_getButtonState(const AInputEvent* event) {
  event = Input(event);
  return event == nullptr ? 0 : event->button_state;
}
extern "C" int32_t AMotionEvent_getClassification(const AInputEvent* event) {
  event = Input(event);
  return event == nullptr ? 0 : event->classification;
}
extern "C" int64_t AMotionEvent_getDownTime(const AInputEvent* event) {
  event = Input(event);
  return event == nullptr ? 0 : event->down_time;
}
extern "C" int64_t AMotionEvent_getEventTime(const AInputEvent* event) {
  event = Input(event);
  return event == nullptr ? 0 : event->event_time;
}
extern "C" size_t AMotionEvent_getPointerCount(const AInputEvent* event) {
  return Input(event) == nullptr ? 0 : 1;
}
extern "C" int32_t AMotionEvent_getPointerId(const AInputEvent* event, size_t index) {
  event = Input(event);
  return event != nullptr && index == 0 ? event->pointer_id : -1;
}
extern "C" int32_t AMotionEvent_getToolType(const AInputEvent* event, size_t index) {
  event = Input(event);
  return event != nullptr && index == 0 ? event->tool_type
                                       : AMOTION_EVENT_TOOL_TYPE_UNKNOWN;
}
extern "C" float AMotionEvent_getRawX(const AInputEvent* event, size_t index) {
  event = Input(event);
  return event != nullptr && index == 0 ? event->raw_x : 0;
}
extern "C" float AMotionEvent_getRawY(const AInputEvent* event, size_t index) {
  event = Input(event);
  return event != nullptr && index == 0 ? event->raw_y : 0;
}
extern "C" float AMotionEvent_getX(const AInputEvent* event, size_t index) {
  event = Input(event);
  return event != nullptr && index == 0 ? event->x : 0;
}
extern "C" float AMotionEvent_getY(const AInputEvent* event, size_t index) {
  event = Input(event);
  return event != nullptr && index == 0 ? event->y : 0;
}
extern "C" float AMotionEvent_getPressure(const AInputEvent* event, size_t index) {
  event = Input(event);
  return event != nullptr && index == 0 ? event->pressure : 0;
}
extern "C" float AMotionEvent_getTouchMajor(const AInputEvent* event, size_t index) {
  event = Input(event);
  return event != nullptr && index == 0 ? event->touch_major : 0;
}
extern "C" float AMotionEvent_getTouchMinor(const AInputEvent* event, size_t index) {
  event = Input(event);
  return event != nullptr && index == 0 ? event->touch_minor : 0;
}
extern "C" float AMotionEvent_getOrientation(const AInputEvent* event, size_t index) {
  event = Input(event);
  return event != nullptr && index == 0 ? event->orientation : 0;
}
extern "C" float AMotionEvent_getAxisValue(const AInputEvent* event, int32_t axis,
                                            size_t index) {
  return index == 0 ? MotionAxis(event, axis) : 0;
}
extern "C" size_t AMotionEvent_getHistorySize(const AInputEvent*) { return 0; }
extern "C" int64_t AMotionEvent_getHistoricalEventTime(const AInputEvent*, size_t) {
  return 0;
}
extern "C" float AMotionEvent_getHistoricalX(const AInputEvent*, size_t, size_t) {
  return 0;
}
extern "C" float AMotionEvent_getHistoricalY(const AInputEvent*, size_t, size_t) {
  return 0;
}
extern "C" float AMotionEvent_getHistoricalTouchMajor(const AInputEvent*, size_t,
                                                        size_t) {
  return 0;
}

extern "C" ASurfaceControl* ASurfaceControl_createFromWindow(
    ANativeWindow* window, const char* name) {
  uint32_t owner_process_id = 0;
  uint32_t layer_id = 0;
  if (darwin_art_android_ANativeWindow_get_imported_surface_identity(
          window, &owner_process_id, &layer_id)) {
    auto* child = darwin_art::window::SurfaceControlRegistry::Instance().Create(
        nullptr, name, false, 0, 0, owner_process_id, layer_id);
    if (child != nullptr) {
      if (child != nullptr && DebugSurfaceTransactions()) {
        uint32_t child_owner_id = 0;
        uint32_t child_layer_id = 0;
        (void)darwin_art::window::SurfaceControlRegistry::Instance()
            .GetIdentity(child, &child_owner_id, &child_layer_id);
        std::fprintf(stderr,
                     "ART Android SurfaceControl: create-from-window "
                     "layer=%u imported-parent=%u:%u name=%s\n",
                     child_layer_id, owner_process_id, layer_id,
                     name == nullptr ? "" : name);
      }
    }
    return child;
  }
  return darwin_art::window::SurfaceControlRegistry::Instance().Create(
      nullptr, name, true);
}

extern "C" ASurfaceControl* ASurfaceControl_create(ASurfaceControl* parent,
                                                     const char* name) {
  return darwin_art::window::SurfaceControlRegistry::Instance().Create(
      parent, name, false);
}

extern "C" void ASurfaceControl_acquire(ASurfaceControl* opaque) {
  darwin_art::window::SurfaceControlRegistry::Instance().Acquire(opaque);
}

extern "C" void ASurfaceControl_release(ASurfaceControl* opaque) {
  darwin_art::window::SurfaceControlRegistry::Instance().Release(opaque);
}

extern "C" void darwin_art_android_surface_transaction_merge(
    void* opaque_destination, void* opaque_source) {
  if (opaque_destination == nullptr || opaque_source == nullptr ||
      opaque_destination == opaque_source) {
    return;
  }
  ASurfaceTransaction* disposal = ASurfaceTransaction_create();
  if (disposal == nullptr) {
    std::fprintf(stderr,
                 "ART Android SurfaceTransaction: merge disposal allocation failed\n");
    std::abort();
  }
  if (!darwin_art_android_surface_transaction_merge_deferred(
          opaque_destination, opaque_source, disposal)) {
    ASurfaceTransaction_delete(disposal);
    std::fprintf(stderr,
                 "ART Android SurfaceTransaction: structural merge failed\n");
    std::abort();
  }
  ASurfaceTransaction_delete(disposal);
}

extern "C" bool darwin_art_android_surface_transaction_merge_deferred(
    void* opaque_destination, void* opaque_source, void* opaque_disposal) {
  return darwin_art::window::MergeSurfaceTransactions(
      reinterpret_cast<SurfaceTransaction*>(opaque_destination),
      reinterpret_cast<SurfaceTransaction*>(opaque_source),
      reinterpret_cast<SurfaceTransaction*>(opaque_disposal));
}

extern "C" void
darwin_art_android_surface_transaction_set_transparent_region_hint(
    void* opaque, void* opaque_control, const int32_t* rects, size_t count) {
  auto* transaction = reinterpret_cast<SurfaceTransaction*>(opaque);
  auto* control = reinterpret_cast<ASurfaceControl*>(opaque_control);
  if (transaction == nullptr || control == nullptr) return;
  const size_t requested_count = rects == nullptr ? 0 : count;
  const bool changed = RequireSurfaceTransactionBuilderResult(
      SurfaceTransactionBuilder(transaction).SetTransparentRegion(control,
                                                                   rects, count),
      transaction, control);
  if (DebugSurfaceTransactions()) {
    uint32_t owner_process_id = 0;
    uint32_t layer_id = 0;
    (void)darwin_art::window::SurfaceControlRegistry::Instance().GetIdentity(
        control, &owner_process_id, &layer_id);
    std::fprintf(stderr, "ART Android SurfaceTransaction: transparent-region "
                         "layer=%u rects=%zu\n",
                 layer_id, changed ? std::min(requested_count, size_t{8}) : 0u);
  }
}

extern "C" size_t darwin_art_android_surface_control_copy_transparent_region(
    void* opaque, int32_t* rects, size_t capacity) {
  return darwin_art::window::SurfaceControlRegistry::Instance()
      .CopyTransparentRegion(
          static_cast<const ASurfaceControl*>(opaque), rects, capacity);
}


#define SURFACE_CONTROL_SETTER(name, signature, control_arg) \
  extern "C" void name signature {                           \
    auto* builder_transaction =                                        \
        reinterpret_cast<SurfaceTransaction*>(transaction);             \
    (void)RequireSurfaceTransactionBuilderResult(                       \
        SurfaceTransactionBuilder(builder_transaction).Remember(        \
            control_arg),                                               \
        builder_transaction, control_arg);                              \
  }

extern "C" void ASurfaceTransaction_reparent(
    ASurfaceTransaction* opaque, ASurfaceControl* control,
    ASurfaceControl* parent) {
  auto* transaction = reinterpret_cast<SurfaceTransaction*>(opaque);
  (void)RequireSurfaceTransactionBuilderResult(
      SurfaceTransactionBuilder(transaction).SetReparent(control, parent),
      transaction, control);
  if (DebugSurfaceTransactions()) {
    std::fprintf(stderr,
                 "ART Android SurfaceTransaction: reparent pid=%d "
                 "control=%p parent=%p\n",
                 getpid(), static_cast<void*>(control),
                 static_cast<void*>(parent));
  }
}
extern "C" void ASurfaceTransaction_setVisibility(
    ASurfaceTransaction* opaque, ASurfaceControl* control,
    enum ASurfaceTransactionVisibility visibility) {
  auto* transaction = reinterpret_cast<SurfaceTransaction*>(opaque);
  (void)RequireSurfaceTransactionBuilderResult(
      SurfaceTransactionBuilder(transaction).SetVisibility(
          control, visibility == ASURFACE_TRANSACTION_VISIBILITY_SHOW),
      transaction, control);
}
extern "C" void ASurfaceTransaction_setZOrder(ASurfaceTransaction* opaque,
                                                ASurfaceControl* control,
                                                int32_t z_order) {
  auto* transaction = reinterpret_cast<SurfaceTransaction*>(opaque);
  (void)RequireSurfaceTransactionBuilderResult(
      SurfaceTransactionBuilder(transaction).SetZOrder(control, z_order),
      transaction, control);
}
extern "C" void darwin_art_android_surface_transaction_set_relative_layer(
    void* opaque, void* opaque_control, void* opaque_relative_to, int32_t z) {
  auto* transaction = reinterpret_cast<SurfaceTransaction*>(opaque);
  auto* control = reinterpret_cast<ASurfaceControl*>(opaque_control);
  auto* relative_to = reinterpret_cast<ASurfaceControl*>(opaque_relative_to);
  (void)RequireSurfaceTransactionBuilderResult(
      SurfaceTransactionBuilder(transaction).SetRelativeLayer(control,
                                                               relative_to, z),
      transaction, control);
  if (DebugSurfaceTransactions()) {
    std::fprintf(stderr,
                 "ART Android SurfaceTransaction: relative-layer pid=%d "
                 "control=%p relative=%p z=%d\n",
                 getpid(), opaque_control, opaque_relative_to, z);
  }
}
extern "C" bool darwin_art_android_surface_transaction_set_buffer_checked(
    void* opaque, void* opaque_control, AHardwareBuffer* buffer, int fence_fd);

extern "C" void ASurfaceTransaction_setBuffer(
    ASurfaceTransaction* transaction, ASurfaceControl* control,
    AHardwareBuffer* buffer, int fence_fd) {
  auto* state = reinterpret_cast<SurfaceTransaction*>(transaction);
  // Collect diagnostics solely from the incoming arguments.  SetBuffer may
  // synchronously invoke arbitrary discard callbacks that clear, delete, or
  // reuse the public transaction.
  const auto surface = static_cast<IOSurfaceRef>(
      darwin_art_android_hardware_buffer_iosurface(buffer));
  AHardwareBuffer_Desc description{};
  AHardwareBuffer_describe(buffer, &description);
  const uint32_t surface_id = surface == nullptr ? 0 : IOSurfaceGetID(surface);
  const uint32_t width = description.width;
  const uint32_t height = description.height;
  const bool changed = RequireSurfaceTransactionBuilderResult(
      darwin_art_android_surface_transaction_set_buffer_checked(
          transaction, control, buffer, fence_fd),
      state, control);
  if (!DebugSurfaceTransactions()) return;
  std::fprintf(stderr,
               "ART Android SurfaceTransaction: setBuffer pid=%d control=%p "
               "buffer=%p iosurface=%u size=%ux%u fence=%d\n",
               getpid(), static_cast<void*>(control), static_cast<void*>(buffer),
               surface_id, width, height, fence_fd);
  if (!changed)
    std::fprintf(stderr,
                 "ART Android SurfaceTransaction: setBuffer rejected\n");
}
extern "C" bool darwin_art_android_surface_transaction_set_buffer_checked(
    void* opaque, void* opaque_control, AHardwareBuffer* buffer, int fence_fd) {
  auto* transaction = reinterpret_cast<SurfaceTransaction*>(opaque);
  auto* control = reinterpret_cast<ASurfaceControl*>(opaque_control);
  return SurfaceTransactionBuilder(transaction).SetBuffer(control, buffer,
                                                           fence_fd);
}
extern "C" void ASurfaceTransaction_setGeometry(
    ASurfaceTransaction* opaque, ASurfaceControl* control, const ARect& source,
    const ARect& destination, int32_t transform) {
  auto* transaction = reinterpret_cast<SurfaceTransaction*>(opaque);
  (void)RequireSurfaceTransactionBuilderResult(
      SurfaceTransactionBuilder(transaction).SetGeometry(
          control, source, destination, transform),
      transaction, control);
  if (DebugSurfaceTransactions()) {
    std::fprintf(stderr,
                 "ART Android SurfaceTransaction: geometry pid=%d control=%p "
                 "source=[%d,%d,%d,%d] destination=[%d,%d,%d,%d] "
                 "transform=%d\n",
                 getpid(), static_cast<void*>(control), source.left,
                 source.top, source.right, source.bottom, destination.left,
                 destination.top, destination.right, destination.bottom,
                 transform);
  }
}
extern "C" void ASurfaceTransaction_setCrop(ASurfaceTransaction* opaque,
                                               ASurfaceControl* control,
                                               const ARect& crop) {
  auto* transaction = reinterpret_cast<SurfaceTransaction*>(opaque);
  (void)RequireSurfaceTransactionBuilderResult(
      SurfaceTransactionBuilder(transaction).SetCrop(control, crop),
      transaction, control);
}
extern "C" void ASurfaceTransaction_setPosition(ASurfaceTransaction* opaque,
                                                   ASurfaceControl* control,
                                                   int32_t x, int32_t y) {
  auto* transaction = reinterpret_cast<SurfaceTransaction*>(opaque);
  (void)RequireSurfaceTransactionBuilderResult(
      SurfaceTransactionBuilder(transaction).SetPosition(control, x, y),
      transaction, control);
  if (DebugSurfaceTransactions()) {
    std::fprintf(stderr,
                 "ART Android SurfaceTransaction: position pid=%d control=%p "
                 "position=%d,%d\n",
                 getpid(), static_cast<void*>(control), x, y);
  }
}
extern "C" void ASurfaceTransaction_setBufferTransform(
    ASurfaceTransaction* opaque, ASurfaceControl* control, int32_t transform) {
  auto* transaction = reinterpret_cast<SurfaceTransaction*>(opaque);
  (void)RequireSurfaceTransactionBuilderResult(
      SurfaceTransactionBuilder(transaction).SetBufferTransform(control,
                                                                transform),
      transaction, control);
}
extern "C" void ASurfaceTransaction_setScale(ASurfaceTransaction* opaque,
                                                ASurfaceControl* control,
                                                float x, float y) {
  auto* transaction = reinterpret_cast<SurfaceTransaction*>(opaque);
  (void)RequireSurfaceTransactionBuilderResult(
      SurfaceTransactionBuilder(transaction).SetScale(control, x, y),
      transaction, control);
}
SURFACE_CONTROL_SETTER(ASurfaceTransaction_setBufferTransparency,
  (ASurfaceTransaction* transaction, ASurfaceControl* control, enum ASurfaceTransactionTransparency), control)
extern "C" void ASurfaceTransaction_setDamageRegion(
    ASurfaceTransaction* opaque, ASurfaceControl* control,
    const ARect* rects, uint32_t count) {
  auto* transaction = reinterpret_cast<SurfaceTransaction*>(opaque);
  (void)RequireSurfaceTransactionBuilderResult(
      SurfaceTransactionBuilder(transaction).SetDamageRegion(control, rects,
                                                             count),
      transaction, control);
  if (!DebugSurfaceTransactions()) return;
  std::fprintf(stderr,
               "ART Android SurfaceTransaction: damage pid=%d control=%p "
               "rects=%u",
               getpid(), static_cast<void*>(control), count);
  std::fprintf(stderr, "\n");
}
extern "C" void ASurfaceTransaction_setBufferAlpha(ASurfaceTransaction* opaque,
                                                      ASurfaceControl* control,
                                                      float alpha) {
  auto* transaction = reinterpret_cast<SurfaceTransaction*>(opaque);
  (void)RequireSurfaceTransactionBuilderResult(
      SurfaceTransactionBuilder(transaction).SetAlpha(control, alpha),
      transaction, control);
}
SURFACE_CONTROL_SETTER(ASurfaceTransaction_setBufferDataSpace,
  (ASurfaceTransaction* transaction, ASurfaceControl* control, enum ADataSpace), control)
extern "C" void ASurfaceTransaction_setColor(
    ASurfaceTransaction* opaque, ASurfaceControl* control, float red,
    float green, float blue, float alpha, enum ADataSpace) {
  auto* transaction = reinterpret_cast<SurfaceTransaction*>(opaque);
  (void)RequireSurfaceTransactionBuilderResult(
      SurfaceTransactionBuilder(transaction).Remember(control), transaction,
      control);
  if (DebugSurfaceTransactions()) {
    std::fprintf(stderr,
                 "ART Android SurfaceTransaction: color pid=%d control=%p "
                 "rgba=%.3f,%.3f,%.3f,%.3f\n",
                 getpid(), static_cast<void*>(control), red, green, blue,
                 alpha);
  }
}
SURFACE_CONTROL_SETTER(ASurfaceTransaction_setHdrMetadata_smpte2086,
  (ASurfaceTransaction* transaction, ASurfaceControl* control, struct AHdrMetadata_smpte2086*), control)
SURFACE_CONTROL_SETTER(ASurfaceTransaction_setHdrMetadata_cta861_3,
  (ASurfaceTransaction* transaction, ASurfaceControl* control, struct AHdrMetadata_cta861_3*), control)
SURFACE_CONTROL_SETTER(ASurfaceTransaction_setExtendedRangeBrightness,
  (ASurfaceTransaction* transaction, ASurfaceControl* control, float, float), control)
SURFACE_CONTROL_SETTER(ASurfaceTransaction_setDesiredHdrHeadroom,
  (ASurfaceTransaction* transaction, ASurfaceControl* control, float), control)
SURFACE_CONTROL_SETTER(ASurfaceTransaction_setFrameRate,
  (ASurfaceTransaction* transaction, ASurfaceControl* control, float, int8_t), control)
SURFACE_CONTROL_SETTER(ASurfaceTransaction_setFrameRateWithChangeStrategy,
  (ASurfaceTransaction* transaction, ASurfaceControl* control, float, int8_t, int8_t), control)
SURFACE_CONTROL_SETTER(ASurfaceTransaction_clearFrameRate,
  (ASurfaceTransaction* transaction, ASurfaceControl* control), control)
SURFACE_CONTROL_SETTER(ASurfaceTransaction_setEnableBackPressure,
  (ASurfaceTransaction* transaction, ASurfaceControl* control, bool), control)

extern "C" void ASurfaceTransaction_setDesiredPresentTime(ASurfaceTransaction*, int64_t) {}
extern "C" void ASurfaceTransaction_setFrameTimeline(ASurfaceTransaction*, AVsyncId) {}

using ServiceCallback = void (*)(void*, void*);
extern "C" void ANativeService_setOnBindCallback(ServiceCallback, void*) {}
extern "C" void ANativeService_setOnDestroyCallback(ServiceCallback, void*) {}
extern "C" void ANativeService_setOnRebindCallback(ServiceCallback, void*) {}
extern "C" void ANativeService_setOnUnbindCallback(ServiceCallback, void*) {}


extern "C" void* darwin_art_android_platform_symbol(const char* symbol) {
  if (symbol == nullptr) return nullptr;
#define ROUTE(name) if (std::strcmp(symbol, #name) == 0) return reinterpret_cast<void*>(&name)
  ROUTE(AHardwareBuffer_acquire);
  ROUTE(darwin_art_bionic_socket_broker_close);
  ROUTE(darwin_art_android_ANativeWindow_is_managed);
  ROUTE(darwin_art_android_ANativeWindow_acquire);
  ROUTE(darwin_art_android_ANativeWindow_release);
  ROUTE(darwin_art_android_ANativeWindow_getWidth);
  ROUTE(darwin_art_android_ANativeWindow_getHeight);
  ROUTE(darwin_art_android_ANativeWindow_setBuffersGeometry);
  ROUTE(darwin_art_android_ANativeWindow_prepare_swapchain);
  ROUTE(darwin_art_android_ANativeWindow_supports_mailbox);
  ROUTE(darwin_art_android_ANativeWindow_set_present_mode);
  ROUTE(darwin_art_android_ANativeWindow_dequeue_hardware_buffer);
  ROUTE(darwin_art_android_ANativeWindow_queue_hardware_buffer);
  ROUTE(darwin_art_android_ANativeWindow_cancel_hardware_buffer);
  ROUTE(AHardwareBuffer_allocate);
  ROUTE(AHardwareBuffer_describe);
  ROUTE(AHardwareBuffer_fromHardwareBuffer);
  ROUTE(AHardwareBuffer_toHardwareBuffer);
  ROUTE(AHardwareBuffer_isSupported);
  ROUTE(AHardwareBuffer_lock);
  ROUTE(AHardwareBuffer_lockPlanes);
  ROUTE(AHardwareBuffer_recvHandleFromUnixSocket);
  ROUTE(AHardwareBuffer_release);
  ROUTE(AHardwareBuffer_sendHandleToUnixSocket);
  ROUTE(AHardwareBuffer_unlock);
  ROUTE(darwin_art_android_hardware_buffer_export_identity);
  ROUTE(darwin_art_android_hardware_buffer_metal_texture);
  ROUTE(darwin_art_android_hardware_buffer_vulkan_metal_texture);
  ROUTE(darwin_art_android_hardware_buffer_vulkan_metal_texture_for_format);
  ROUTE(darwin_art_android_metal_texture_release);
  ROUTE(darwin_art_android_metal_shared_event_create);
  ROUTE(darwin_art_android_metal_shared_event_fence_fd);
  ROUTE(darwin_art_android_metal_shared_event_next_value);
  ROUTE(darwin_art_android_metal_shared_event_import_fence);
  ROUTE(darwin_art_android_metal_shared_event_release);
  ROUTE(AChoreographerFrameCallbackData_getFrameTimeNanos);
  ROUTE(AChoreographerFrameCallbackData_getFrameTimelineDeadlineNanos);
  ROUTE(AChoreographerFrameCallbackData_getFrameTimelineExpectedPresentationTimeNanos);
  ROUTE(AChoreographerFrameCallbackData_getFrameTimelineVsyncId);
  ROUTE(AChoreographerFrameCallbackData_getFrameTimelinesLength);
  ROUTE(AChoreographerFrameCallbackData_getPreferredFrameTimelineIndex);
  ROUTE(AChoreographer_getInstance);
  ROUTE(AChoreographer_postFrameCallback);
  ROUTE(AChoreographer_postFrameCallback64);
  ROUTE(AChoreographer_postFrameCallbackDelayed);
  ROUTE(AChoreographer_postFrameCallbackDelayed64);
  ROUTE(AChoreographer_postVsyncCallback);
  ROUTE(AChoreographer_registerRefreshRateCallback);
  ROUTE(AChoreographer_unregisterRefreshRateCallback);
  ROUTE(AInputEvent_getSource);
  ROUTE(AInputEvent_getType);
  ROUTE(AInputEvent_release);
  ROUTE(AMotionEvent_getAction);
  ROUTE(AMotionEvent_getAxisValue);
  ROUTE(AMotionEvent_getButtonState);
  ROUTE(AMotionEvent_getClassification);
  ROUTE(AMotionEvent_getDownTime);
  ROUTE(AMotionEvent_getEventTime);
  ROUTE(AMotionEvent_getHistoricalEventTime);
  ROUTE(AMotionEvent_getHistoricalTouchMajor);
  ROUTE(AMotionEvent_getHistoricalX);
  ROUTE(AMotionEvent_getHistoricalY);
  ROUTE(AMotionEvent_getHistorySize);
  ROUTE(AMotionEvent_getMetaState);
  ROUTE(AMotionEvent_getOrientation);
  ROUTE(AMotionEvent_getPointerCount);
  ROUTE(AMotionEvent_getPointerId);
  ROUTE(AMotionEvent_getPressure);
  ROUTE(AMotionEvent_getRawX);
  ROUTE(AMotionEvent_getRawY);
  ROUTE(AMotionEvent_getToolType);
  ROUTE(AMotionEvent_getTouchMajor);
  ROUTE(AMotionEvent_getTouchMinor);
  ROUTE(AMotionEvent_getX);
  ROUTE(AMotionEvent_getY);
  ROUTE(ALooper_acquire);
  ROUTE(ALooper_addFd);
  ROUTE(ALooper_forThread);
  ROUTE(ALooper_pollOnce);
  ROUTE(ALooper_prepare);
  ROUTE(ALooper_release);
  ROUTE(ALooper_removeFd);
  ROUTE(ALooper_wake);
  ROUTE(ASensorEventQueue_disableSensor);
  ROUTE(ASensorEventQueue_enableSensor);
  ROUTE(ASensorEventQueue_getEvents);
  ROUTE(ASensorEventQueue_hasEvents);
  ROUTE(ASensorEventQueue_setEventRate);
  ROUTE(ASensorManager_createEventQueue);
  ROUTE(ASensorManager_destroyEventQueue);
  ROUTE(ASensorManager_getDefaultSensor);
  ROUTE(ASensorManager_getInstance);
  ROUTE(ASensorManager_getInstanceForPackage);
  ROUTE(ASensorManager_getSensorList);
  ROUTE(ASensor_getMinDelay);
  ROUTE(ASensor_getName);
  ROUTE(ASensor_getResolution);
  ROUTE(ASensor_getType);
  ROUTE(ASensor_getVendor);
  ROUTE(ASharedMemory_create);
  ROUTE(ASharedMemory_setProt);
  ROUTE(ASurfaceControl_create);
  ROUTE(ASurfaceControl_createFromWindow);
  ROUTE(ASurfaceControl_fromJava);
  ROUTE(ASurfaceControl_acquire);
  ROUTE(ASurfaceControl_release);
  ROUTE(ASurfaceTransactionStats_getASurfaceControls);
  ROUTE(ASurfaceTransactionStats_getLatchTime);
  ROUTE(ASurfaceTransactionStats_getAcquireTime);
  ROUTE(ASurfaceTransactionStats_getPresentFenceFd);
  ROUTE(ASurfaceTransactionStats_getPreviousBufferMetadata);
  ROUTE(ASurfaceTransactionStats_getPreviousReleaseFenceFd);
  ROUTE(ASurfaceTransactionStats_releaseASurfaceControls);
  ROUTE(ASurfaceTransaction_apply);
  ROUTE(ASurfaceTransaction_create);
  ROUTE(ASurfaceTransaction_delete);
  ROUTE(ASurfaceTransaction_reparent);
  ROUTE(ASurfaceTransaction_setBuffer);
  ROUTE(ASurfaceTransaction_setBufferAlpha);
  ROUTE(ASurfaceTransaction_setBufferDataSpace);
  ROUTE(ASurfaceTransaction_setBufferTransform);
  ROUTE(ASurfaceTransaction_setBufferTransparency);
  ROUTE(ASurfaceTransaction_setColor);
  ROUTE(ASurfaceTransaction_setCrop);
  ROUTE(ASurfaceTransaction_setDamageRegion);
  ROUTE(ASurfaceTransaction_setDesiredHdrHeadroom);
  ROUTE(ASurfaceTransaction_setDesiredPresentTime);
  ROUTE(ASurfaceTransaction_setEnableBackPressure);
  ROUTE(ASurfaceTransaction_setExtendedRangeBrightness);
  ROUTE(ASurfaceTransaction_setFrameTimeline);
  ROUTE(ASurfaceTransaction_setFrameRate);
  ROUTE(ASurfaceTransaction_setFrameRateWithChangeStrategy);
  ROUTE(ASurfaceTransaction_clearFrameRate);
  ROUTE(ASurfaceTransaction_setGeometry);
  ROUTE(ASurfaceTransaction_setHdrMetadata_cta861_3);
  ROUTE(ASurfaceTransaction_setHdrMetadata_smpte2086);
  ROUTE(ASurfaceTransaction_setOnCommit);
  ROUTE(ASurfaceTransaction_setOnComplete);
  ROUTE(ASurfaceTransaction_setPosition);
  ROUTE(ASurfaceTransaction_setScale);
  ROUTE(ASurfaceTransaction_setVisibility);
  ROUTE(ASurfaceTransaction_setZOrder);
  ROUTE(ANativeService_setOnBindCallback);
  ROUTE(ANativeService_setOnDestroyCallback);
  ROUTE(ANativeService_setOnRebindCallback);
  ROUTE(ANativeService_setOnUnbindCallback);
  if (std::strcmp(symbol, "ANativeWindow_acquire") == 0)
    return reinterpret_cast<void*>(&ANativeWindow_acquire);
  if (std::strcmp(symbol, "ANativeWindow_fromSurface") == 0)
    return reinterpret_cast<void*>(&darwin_art_android_ANativeWindow_fromSurface);
  if (std::strcmp(symbol, "ANativeWindow_getFormat") == 0)
    return reinterpret_cast<void*>(&ANativeWindow_getFormat);
  if (std::strcmp(symbol, "ANativeWindow_getWidth") == 0)
    return reinterpret_cast<void*>(&ANativeWindow_getWidth);
  if (std::strcmp(symbol, "ANativeWindow_getHeight") == 0)
    return reinterpret_cast<void*>(&ANativeWindow_getHeight);
  if (std::strcmp(symbol, "ANativeWindow_setBuffersGeometry") == 0)
    return reinterpret_cast<void*>(&darwin_art_android_ANativeWindow_setBuffersGeometry);
  if (std::strcmp(symbol, "ANativeWindow_setFrameRate") == 0)
    return reinterpret_cast<void*>(&ANativeWindow_setFrameRate);
  if (std::strcmp(symbol, "ANativeWindow_setFrameRateWithChangeStrategy") == 0)
    return reinterpret_cast<void*>(&ANativeWindow_setFrameRateWithChangeStrategy);
  if (std::strcmp(symbol, "ANativeWindow_release") == 0)
    return reinterpret_cast<void*>(&ANativeWindow_release);
  if (std::strcmp(symbol, "ANativeWindow_toSurface") == 0)
    return reinterpret_cast<void*>(&darwin_art_android_ANativeWindow_toSurface);
#undef ROUTE
  return nullptr;
}
