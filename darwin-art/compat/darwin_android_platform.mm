#include "darwin_android_platform.h"
#include "window/surface_control_jni.h"
#include "memory/shared_memory.h"
#include "darwin_android_time.h"
#include "darwin_surface_bridge.h"
#include "surfaceflinger/transaction_bridge.h"
#include "surfaceflinger/service_darwin.h"

#include "darwin_angle_egl.h"
#include "darwin_art_bionic_socket_broker.h"

#import <IOSurface/IOSurface.h>
#import <Foundation/Foundation.h>
#import <Metal/Metal.h>

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
extern "C" int darwin_art_bionic_socket_broker_pipe(
    int32_t descriptors[2]);
extern "C" intptr_t darwin_art_bionic_socket_broker_write(
    int fd, const void* bytes, size_t count);
extern "C" int darwin_art_bionic_socket_broker_close(int fd);
extern "C" int darwin_art_bionic_socket_broker_dup(int fd);
extern "C" int sync_wait(int fd, int timeout_ms);

struct DarwinAndroidNativeBaseAbi {
  int32_t magic = ('_' << 24) | ('b' << 16) | ('f' << 8) | 'r';
  int32_t version = 0;
  void* reserved[4]{};
  void (*inc_ref)(DarwinAndroidNativeBaseAbi*) = nullptr;
  void (*dec_ref)(DarwinAndroidNativeBaseAbi*) = nullptr;
};

struct DarwinAndroidNativeWindowBufferAbi {
  DarwinAndroidNativeBaseAbi common{};
  int32_t width = 0;
  int32_t height = 0;
  int32_t stride = 0;
  int32_t format = 0;
  int32_t usage_deprecated = 0;
  uintptr_t layer_count = 0;
  void* reserved[1]{};
  const void* handle = nullptr;
  uint64_t usage = 0;
  void* reserved_proc[7]{};
};

struct AHardwareBuffer {
  // Android implements AHardwareBuffer as a GraphicBuffer.  Its public EGL
  // client-buffer view is the embedded ANativeWindowBuffer at +0x10 on
  // arm64.  Native consumers such as Chromium's bundled ANGLE read this ABI
  // directly to determine dimensions, format and usage.
  void* graphic_buffer_prefix[2]{};
  DarwinAndroidNativeWindowBufferAbi native_buffer{};
  std::atomic<uint32_t> references{1};
  AHardwareBuffer_Desc description{};
  IOSurfaceRef surface = nullptr;
  std::mutex mutex;
  uint32_t locks = 0;
};

static_assert(offsetof(AHardwareBuffer, native_buffer) == 0x10);

AHardwareBuffer* HardwareBufferFromNativeBase(
    DarwinAndroidNativeBaseAbi* base) {
  return base == nullptr
             ? nullptr
             : reinterpret_cast<AHardwareBuffer*>(
                   reinterpret_cast<char*>(base) -
                   offsetof(AHardwareBuffer, native_buffer));
}

void HardwareBufferNativeIncRef(DarwinAndroidNativeBaseAbi* base) {
  AHardwareBuffer_acquire(HardwareBufferFromNativeBase(base));
}

void HardwareBufferNativeDecRef(DarwinAndroidNativeBaseAbi* base) {
  AHardwareBuffer_release(HardwareBufferFromNativeBase(base));
}

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

struct ALooperRegistration {
  int fd;
  int ident;
  int events;
  ALooper_callbackFunc callback;
  void* data;
  uint64_t generation = 0;
  uint64_t last_host_turn = 0;
  std::shared_ptr<void> callback_owner;
};

thread_local uint32_t g_looper_callback_depth = 0;

uint64_t ThreadCpuNanos() {
  timespec value{};
  if (clock_gettime(CLOCK_THREAD_CPUTIME_ID, &value) != 0) return 0;
  return static_cast<uint64_t>(value.tv_sec) * UINT64_C(1000000000) +
         static_cast<uint64_t>(value.tv_nsec);
}

uint64_t CurrentThreadId() {
  uint64_t thread_id = 0;
  return pthread_threadid_np(nullptr, &thread_id) == 0 ? thread_id : 0;
}

std::string NativeAddressDescription(const void* address) {
  if (address == nullptr) return "<null>";
  Dl_info info{};
  if (dladdr(address, &info) == 0 || info.dli_fname == nullptr) {
    return "<guest>";
  }
  const auto base = reinterpret_cast<uintptr_t>(info.dli_saddr);
  const auto value = reinterpret_cast<uintptr_t>(address);
  const auto offset = value >= base ? value - base : 0;
  char buffer[32];
  std::snprintf(buffer, sizeof(buffer), "%llx",
                static_cast<unsigned long long>(offset));
  return std::string(info.dli_fname) + "+0x" + buffer;
}

enum class ChoreographerCallbackKind {
  kFrame,
  kFrame64,
  kVsync,
  kRefreshRate,
};

struct ChoreographerCallback {
  std::chrono::steady_clock::time_point deadline;
  ChoreographerCallbackKind kind;
  void* callback;
  void* data;
};

struct ALooper {
  std::atomic<uint32_t> references{1};
  int options = 0;
  int wake_fd = -1;
  uint64_t next_generation = 1;
  std::mutex mutex;
  std::vector<ALooperRegistration> registrations;
  std::vector<ChoreographerCallback> frame_callbacks;
};

struct AChoreographer {
  ALooper* looper = nullptr;
  std::mutex mutex;
  std::vector<std::pair<AChoreographer_refreshRateCallback, void*>>
      refresh_callbacks;
};

struct AChoreographerFrameCallbackData {
  int64_t frame_time_nanos = 0;
  AVsyncId vsync_id = 0;
  int64_t expected_presentation_time_nanos = 0;
  int64_t deadline_nanos = 0;
};

thread_local ALooper* g_thread_looper = nullptr;
thread_local AChoreographer* g_thread_choreographer = nullptr;
thread_local bool g_host_looper_turn_active = false;
thread_local uint64_t g_host_looper_turn = 0;

struct ASensorManager {};
struct ASensorEventQueue {};
struct ASensor {};
ASensorManager g_sensor_manager;

namespace {

std::mutex g_hardware_buffer_mutex;
std::unordered_map<const void*, AHardwareBuffer*> g_hardware_buffer_aliases;

constexpr uint32_t kDarwinBgraPixelFormat =
    (static_cast<uint32_t>('B') << 24) |
    (static_cast<uint32_t>('G') << 16) |
    (static_cast<uint32_t>('R') << 8) | static_cast<uint32_t>('A');

uint32_t BytesPerPixel(uint32_t format) {
  switch (format) {
    case AHARDWAREBUFFER_FORMAT_R8G8B8A8_UNORM:
    case AHARDWAREBUFFER_FORMAT_R8G8B8X8_UNORM:
      return 4;
    default:
      return 0;
  }
}

bool IsHardwareBufferDescriptionSupported(const AHardwareBuffer_Desc* desc) {
  if (desc == nullptr || desc->width == 0 || desc->height == 0 ||
      desc->layers != 1) {
    return false;
  }
  // The Darwin gralloc backend currently exposes one packed BGRA8 IOSurface
  // plane to Metal and EGL. Do not accept R8, YUV, depth, blob, or packed
  // 16-bit requests and silently allocate BGRA storage for them: Android
  // clients use allocation support to decide whether cross-thread SharedImage
  // and media paths are legal.
  return BytesPerPixel(desc->format) != 0;
}

AHardwareBuffer* WrapSurface(IOSurfaceRef surface,
                             const AHardwareBuffer_Desc& description) {
  if (surface == nullptr) return nullptr;
  auto* buffer = new (std::nothrow) AHardwareBuffer();
  if (buffer == nullptr) return nullptr;
  buffer->description = description;
  buffer->description.stride = static_cast<uint32_t>(
      IOSurfaceGetBytesPerRow(surface) / BytesPerPixel(description.format));
  buffer->native_buffer.common.version =
      sizeof(DarwinAndroidNativeWindowBufferAbi);
  buffer->native_buffer.common.inc_ref = &HardwareBufferNativeIncRef;
  buffer->native_buffer.common.dec_ref = &HardwareBufferNativeDecRef;
  buffer->native_buffer.width = static_cast<int>(description.width);
  buffer->native_buffer.height = static_cast<int>(description.height);
  buffer->native_buffer.stride = static_cast<int>(buffer->description.stride);
  buffer->native_buffer.format = static_cast<int>(description.format);
  buffer->native_buffer.usage_deprecated =
      static_cast<int>(description.usage & UINT32_MAX);
  buffer->native_buffer.layer_count = description.layers;
  buffer->native_buffer.handle = nullptr;
  buffer->native_buffer.usage = description.usage;
  buffer->surface = surface;
  {
    std::lock_guard<std::mutex> lock(g_hardware_buffer_mutex);
    g_hardware_buffer_aliases.emplace(buffer, buffer);
    // Android's EGL implementation exposes the embedded
    // ANativeWindowBuffer/GraphicBuffer client view rather than the owning
    // AHardwareBuffer address. ANGLE's host helper preserves that ABI offset.
    g_hardware_buffer_aliases.emplace(&buffer->native_buffer, buffer);
  }
  return buffer;
}

struct SurfaceControl {
  std::atomic<uint32_t> references{1};
  uint32_t owner_process_id = 0;
  uint32_t layer_id = 0;
  std::string name;
  SurfaceControl* parent = nullptr;
  // Cross-process parents are Binder identities, not local object pointers.
  // They are populated for the child created by createFromWindow and cleared
  // by an explicit reparent operation.
  uint32_t imported_parent_owner_process_id = 0;
  uint32_t imported_parent_layer_id = 0;
  SurfaceControl* relative_to = nullptr;
  // A control obtained from a Java Surface or an ANativeWindow is attached
  // to SurfaceFlinger's display tree even though its NDK parent is null.
  // Controls created through ASurfaceControl_create are ordinary children;
  // reparent(child, nullptr) detaches them and they must stop contributing to
  // composition until attached to a rooted ancestor again.
  bool composition_root = false;
  AHardwareBuffer* buffer = nullptr;
  ARect source{};
  ARect destination{};
  ARect crop{};
  bool has_geometry = false;
  bool has_crop = false;
  bool visible = true;
  int32_t position_x = 0;
  int32_t position_y = 0;
  int32_t transform = 0;
  int32_t z_order = 0;
  float scale_x = 1.0f;
  float scale_y = 1.0f;
  float alpha = 1.0f;
  std::vector<ARect> transparent_region;
};

constexpr size_t kMaxTransparentRegionRects = 8;

std::mutex g_surface_controls_mutex;
std::vector<SurfaceControl*> g_surface_controls;
std::atomic<uint32_t> g_next_surface_layer_id{1};
std::atomic<uint64_t> g_next_surface_transaction_id{1};
std::atomic<uint32_t> g_pending_latch_workers{0};
constexpr uint32_t kMaximumPendingLatchWorkers = 8;

bool IsAttachedToCompositionRoot(
    const SurfaceControl* control,
    const std::vector<SurfaceControl*>& controls) {
  // Parent links are runtime-owned identities. Bound the walk by the number
  // of live controls so a malformed/cyclic guest transaction fails detached
  // rather than looping forever or following a released pointer.
  for (size_t depth = 0; control != nullptr && depth <= controls.size();
       ++depth) {
    if (std::find(controls.begin(), controls.end(), control) == controls.end())
      return false;
    if (control->composition_root) return true;
    control = control->parent;
  }
  return false;
}

struct SurfacePresentation {
  SurfaceControl* control = nullptr;
  AHardwareBuffer* buffer = nullptr;
  ARect source{};
  ARect destination{};
  float alpha = 1.0f;
  int32_t z_order = 0;
  std::string name;
  int32_t transform = 0;
  bool reparented = false;
  bool has_damage = false;
  ARect damage{};
};

SurfacePresentation MakePresentation(SurfaceControl* control) {
  const auto& description = control->buffer->description;
  const ARect source = control->has_geometry
                           ? control->source
                           : (control->has_crop
                                  ? control->crop
                                  : ARect{0, 0,
                                          static_cast<int32_t>(description.width),
                                          static_cast<int32_t>(description.height)});
  const ARect destination = control->has_geometry
                                ? control->destination
                                : ARect{
                                      control->position_x,
                                      control->position_y,
                                      control->position_x +
                                          static_cast<int32_t>(
                                              (source.right - source.left) *
                                              control->scale_x),
                                      control->position_y +
                                          static_cast<int32_t>(
                                              (source.bottom - source.top) *
                                              control->scale_y)};
  AHardwareBuffer_acquire(control->buffer);
  return {.control = control,
          .buffer = control->buffer,
          .source = source,
          .destination = destination,
          .alpha = control->alpha,
          .z_order = control->z_order,
          .name = control->name,
          .transform = control->transform};
}

void SetPresentationDamage(SurfacePresentation* presentation,
                           const std::vector<ARect>& damage) {
  if (presentation == nullptr || damage.empty()) return;
  ARect bounds = damage.front();
  for (const ARect& rect : damage) {
    bounds.left = std::min(bounds.left, rect.left);
    bounds.top = std::min(bounds.top, rect.top);
    bounds.right = std::max(bounds.right, rect.right);
    bounds.bottom = std::max(bounds.bottom, rect.bottom);
  }
  // Android keeps layer geometry unchanged. Damage is buffer-coordinate
  // metadata used only to bound recomposition; it is not a crop or a frame.
  presentation->has_damage = true;
  presentation->damage = bounds;
}

struct SurfaceTransactionStats {
  std::vector<ASurfaceControl*> controls;
  std::unordered_map<ASurfaceControl*, AHardwareBuffer*> previous_buffers;
  int present_fence = -1;

  ~SurfaceTransactionStats() {
    for (const auto& [control, buffer] : previous_buffers) {
      (void)control;
      AHardwareBuffer_release(buffer);
    }
    if (present_fence >= 0)
      (void)darwin_art_bionic_socket_broker_close(present_fence);
  }
};

using TransactionCallback = void (*)(void*, ASurfaceTransactionStats*);

struct SurfaceTransaction {
  struct Update {
    ASurfaceControl* opaque = nullptr;
    AHardwareBuffer* buffer = nullptr;
    int acquire_fence = -1;
    bool has_buffer = false;
    ARect source{};
    ARect destination{};
    ARect crop{};
    bool has_geometry = false;
    bool has_crop = false;
    bool has_visibility = false;
    bool visible = true;
    bool has_position = false;
    int32_t position_x = 0;
    int32_t position_y = 0;
    bool has_transform = false;
    int32_t transform = 0;
    bool has_z_order = false;
    int32_t z_order = 0;
    bool has_scale = false;
    float scale_x = 1.0f;
    float scale_y = 1.0f;
    bool has_alpha = false;
    float alpha = 1.0f;
    bool has_parent = false;
    ASurfaceControl* parent = nullptr;
    bool has_relative_layer = false;
    ASurfaceControl* relative_to = nullptr;
    bool has_damage = false;
    std::vector<ARect> damage;
    bool has_transparent_region = false;
    std::vector<ARect> transparent_region;
  };

  std::vector<ASurfaceControl*> controls;
  std::vector<Update> updates;
  struct Callback {
    TransactionCallback function = nullptr;
    void* context = nullptr;
  };
  struct DiscardCallback {
    void (*function)(void*) = nullptr;
    void* context = nullptr;
  };
  std::vector<Callback> commits;
  std::vector<Callback> completes;
  std::vector<DiscardCallback> discards;
  struct BufferCallback {
    ASurfaceControl* control;
    void* context;
    TransactionCallback complete;
    void (*discard)(void*, int);
  };
  std::vector<BufferCallback> buffer_callbacks;
};

// An overwritten buffer never reaches the consumer, but its producer can still
// be writing. Return that acquire fence with the slot instead of releasing early.
void DiscardBufferCallbacks(SurfaceTransaction* transaction,
                            ASurfaceControl* control, bool quarantine = false) {
  std::vector<SurfaceTransaction::BufferCallback> discarded;
  auto& callbacks = transaction->buffer_callbacks;
  for (auto it = callbacks.begin(); it != callbacks.end();) {
    if (control == nullptr || it->control == control) {
      discarded.push_back(*it);
      it = callbacks.erase(it);
    } else {
      ++it;
    }
  }
  for (const auto& callback : discarded) {
    int fence = quarantine ? -2 : -1;
    for (const auto& update : transaction->updates) {
      if (quarantine) break;
      if (update.opaque == callback.control && update.acquire_fence >= 0) {
        fence = darwin_art_bionic_socket_broker_dup(update.acquire_fence);
        if (fence < 0) {
          // FD exhaustion must not make an unfinished producer reusable.
          int result;
          do { result = sync_wait(update.acquire_fence, -1); }
          while (result != 0 && errno == EINTR);
          if (result != 0) fence = -2;
        }
        break;
      }
    }
    callback.discard(callback.context, fence);
  }
}

void DiscardTransactionCallbacks(SurfaceTransaction* transaction) {
  if (transaction == nullptr) return;
  DiscardBufferCallbacks(transaction, nullptr);
  auto callbacks = std::move(transaction->discards);
  transaction->discards.clear();
  transaction->commits.clear();
  transaction->completes.clear();
  for (const auto& callback : callbacks) {
    if (callback.function != nullptr) callback.function(callback.context);
  }
}

struct DeferredLatchTransaction {
  std::unique_ptr<SurfaceTransaction> transaction;
  std::vector<ASurfaceControl*> controls;
};

struct LatchQueueState {
  std::mutex mutex;
  std::condition_variable condition;
  std::deque<DeferredLatchTransaction> pending;
  bool started = false;
};

LatchQueueState& LatchQueue() {
  // The queue intentionally lives until process exit. Android application
  // processes are one-shot, and leaking this tiny coordinator avoids a static
  // destructor racing a late producer during ART shutdown.
  static auto* state = new LatchQueueState();
  return *state;
}

SurfaceTransaction::Update* FindUpdate(SurfaceTransaction* transaction,
                                       ASurfaceControl* control) {
  if (transaction == nullptr || control == nullptr) return nullptr;
  auto found = std::find_if(
      transaction->updates.begin(), transaction->updates.end(),
      [control](const SurfaceTransaction::Update& update) {
        return update.opaque == control;
      });
  if (found != transaction->updates.end()) return &*found;
  transaction->updates.push_back({.opaque = control});
  return &transaction->updates.back();
}

ASurfaceControl* CreateSurfaceControl(ASurfaceControl* parent,
                                      const char* name,
                                      bool composition_root,
                                      uint32_t imported_owner_process_id = 0,
                                      uint32_t imported_layer_id = 0) {
  auto* control = new (std::nothrow) SurfaceControl();
  if (control != nullptr) {
    control->owner_process_id = imported_owner_process_id == 0
        ? static_cast<uint32_t>(getpid())
        : imported_owner_process_id;
    control->layer_id = imported_layer_id == 0
        ? g_next_surface_layer_id.fetch_add(1, std::memory_order_relaxed)
        : imported_layer_id;
    if (name != nullptr) control->name = name;
    control->parent = reinterpret_cast<SurfaceControl*>(parent);
    if (parent != nullptr) ASurfaceControl_acquire(parent);
    control->composition_root = composition_root;
    std::lock_guard<std::mutex> lock(g_surface_controls_mutex);
    g_surface_controls.push_back(control);
    if (std::getenv("DARWIN_ART_DEBUG_SURFACE_TRANSACTIONS") != nullptr) {
      std::fprintf(stderr,
                   "ART Android SurfaceControl: create pid=%d control=%p "
                   "owner=%u layer=%u parent=%p root=%d name=%s\n",
                   getpid(), static_cast<void*>(control),
                   control->owner_process_id, control->layer_id,
                   static_cast<void*>(parent), composition_root ? 1 : 0,
                   control->name.c_str());
    }
  }
  return reinterpret_cast<ASurfaceControl*>(control);
}

uint32_t SurfaceControlParentOwner(const SurfaceControl* control) {
  if (control == nullptr) return 0;
  return control->parent != nullptr
      ? control->parent->owner_process_id
      : control->imported_parent_owner_process_id;
}

uint32_t SurfaceControlParentLayer(const SurfaceControl* control) {
  if (control == nullptr) return 0;
  return control->parent != nullptr ? control->parent->layer_id
                                    : control->imported_parent_layer_id;
}

extern "C" void* darwin_art_android_surface_control_create_root(
    const char* name) {
  return CreateSurfaceControl(nullptr, name, true);
}

extern "C" void* darwin_art_android_surface_control_create_imported(
    uint32_t owner_process_id, uint32_t layer_id, const char* name) {
  if (owner_process_id == 0 || layer_id == 0) return nullptr;
  return CreateSurfaceControl(nullptr, name, true, owner_process_id, layer_id);
}

extern "C" bool darwin_art_android_surface_control_get_identity(
    void* opaque, uint32_t* owner_process_id, uint32_t* layer_id) {
  const auto* control = static_cast<const SurfaceControl*>(opaque);
  if (control == nullptr || owner_process_id == nullptr || layer_id == nullptr) {
    return false;
  }
  *owner_process_id = control->owner_process_id;
  *layer_id = control->layer_id;
  return true;
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

void Remember(SurfaceTransaction* transaction, ASurfaceControl* control) {
  if (transaction == nullptr || control == nullptr) return;
  if (std::find(transaction->controls.begin(), transaction->controls.end(),
                control) == transaction->controls.end()) {
    // Android transactions keep an sp<SurfaceControl> for every referenced
    // layer. The caller may release its handle as soon as the transaction is
    // applied, so the transaction must not merely remember a raw pointer.
    ASurfaceControl_acquire(control);
    transaction->controls.push_back(control);
  }
  (void)FindUpdate(transaction, control);
}

void ReleaseTransactionControls(SurfaceTransaction* transaction) {
  if (transaction == nullptr) return;
  auto controls = std::move(transaction->controls);
  transaction->controls.clear();
  for (ASurfaceControl* control : controls) {
    if (control != nullptr) ASurfaceControl_release(control);
  }
}

void ReleaseTransactionBuffers(SurfaceTransaction* transaction) {
  if (transaction == nullptr) return;
  for (auto& update : transaction->updates) {
    if (update.buffer != nullptr) AHardwareBuffer_release(update.buffer);
    if (update.acquire_fence >= 0)
      (void)darwin_art_bionic_socket_broker_close(update.acquire_fence);
    update.buffer = nullptr;
    update.acquire_fence = -1;
  }
  transaction->updates.clear();
}

void NoopServiceCallback(void*) {}

}  // namespace

extern "C" int AHardwareBuffer_allocate(const AHardwareBuffer_Desc* desc,
                                         AHardwareBuffer** out) {
  if (out == nullptr || !IsHardwareBufferDescriptionSupported(desc)) {
    return -EINVAL;
  }
  *out = nullptr;
  const uint32_t bytes_per_pixel = BytesPerPixel(desc->format);
  const size_t packed_row_bytes =
      static_cast<size_t>(desc->width) * bytes_per_pixel;
  if (packed_row_bytes / bytes_per_pixel != desc->width ||
      packed_row_bytes > SIZE_MAX - 15) {
    return -EOVERFLOW;
  }
  // IOSurface-backed Metal textures require a 16-byte-aligned row stride.
  // Android gralloc is likewise allowed to return a stride wider than the
  // requested pixel width, and clients discover it through
  // AHardwareBuffer_describe/ANativeWindowBuffer.  Keep the logical width
  // unchanged while allocating and reporting the aligned storage width.
  const size_t row_bytes = (packed_row_bytes + 15) & ~size_t{15};
  if (row_bytes / bytes_per_pixel > UINT32_MAX ||
      row_bytes > SIZE_MAX / desc->height) {
    return -EOVERFLOW;
  }
  NSDictionary* properties = @{
    (__bridge NSString*)kIOSurfaceWidth : @(desc->width),
    (__bridge NSString*)kIOSurfaceHeight : @(desc->height),
    (__bridge NSString*)kIOSurfaceBytesPerElement : @(bytes_per_pixel),
    (__bridge NSString*)kIOSurfaceBytesPerRow : @(row_bytes),
    (__bridge NSString*)kIOSurfaceAllocSize : @(row_bytes * desc->height),
    (__bridge NSString*)kIOSurfacePixelFormat : @(kDarwinBgraPixelFormat),
    (__bridge NSString*)kIOSurfaceIsGlobal : @YES,
  };
  IOSurfaceRef surface = IOSurfaceCreate((__bridge CFDictionaryRef)properties);
  if (surface == nullptr) return -ENOMEM;
  AHardwareBuffer* buffer = WrapSurface(surface, *desc);
  if (buffer == nullptr) {
    CFRelease(surface);
    return -ENOMEM;
  }
  *out = buffer;
  if (std::getenv("DARWIN_ART_DEBUG_GRAPHICS_DSO") != nullptr) {
    std::fprintf(stderr,
                 "ART Android AHardwareBuffer: allocate buffer=%p surface=%p "
                 "size=%ux%u format=%u usage=%llu\n",
                 static_cast<void*>(buffer), static_cast<void*>(surface),
                 desc->width, desc->height, desc->format,
                 static_cast<unsigned long long>(desc->usage));
  }
  return 0;
}

extern "C" int AHardwareBuffer_isSupported(
    const AHardwareBuffer_Desc* desc) {
  return IsHardwareBufferDescriptionSupported(desc) ? 1 : 0;
}

extern "C" void AHardwareBuffer_acquire(AHardwareBuffer* buffer) {
  if (buffer != nullptr) buffer->references.fetch_add(1, std::memory_order_relaxed);
}

extern "C" void* darwin_art_android_hardware_buffer_metal_texture(
    AHardwareBuffer* buffer, void* metal_device) {
  if (buffer == nullptr || buffer->surface == nullptr || metal_device == nullptr ||
      buffer->description.width == 0 || buffer->description.height == 0) {
    return nullptr;
  }
  return darwin_art_android_iosurface_metal_texture(
      buffer->surface, buffer->description.width, buffer->description.height,
      metal_device);
}

static void* CreateVulkanHardwareBufferTexture(
    AHardwareBuffer* buffer, void* metal_device, int32_t vk_format,
    bool importing_storage) {
  if (buffer == nullptr || buffer->surface == nullptr || metal_device == nullptr ||
      buffer->description.width == 0 || buffer->description.height == 0) {
    return nullptr;
  }
  id<MTLDevice> device = (__bridge id<MTLDevice>)metal_device;
  const uint32_t surface_width = IOSurfaceGetWidth(buffer->surface);
  const uint32_t surface_height = IOSurfaceGetHeight(buffer->surface);
  if (surface_width == 0 || surface_height == 0) return nullptr;
  if (vk_format != 37 && vk_format != 43) return nullptr;
  const MTLPixelFormat format = vk_format == 43 ? MTLPixelFormatRGBA8Unorm_sRGB
                                               : MTLPixelFormatRGBA8Unorm;
  MTLTextureDescriptor* descriptor =
      [MTLTextureDescriptor texture2DDescriptorWithPixelFormat:format
                                                         width:surface_width
                                                        height:surface_height
                                                     mipmapped:NO];
  descriptor.storageMode = MTLStorageModeShared;
  descriptor.usage = MTLTextureUsageShaderRead | MTLTextureUsageShaderWrite |
                     MTLTextureUsageRenderTarget | MTLTextureUsagePixelFormatView;
  // This producer writes actual RGBA bytes. Preserve that storage contract
  // for consumers which otherwise assume the legacy HWUI BGRA storage.
  if (importing_storage) {
    IOSurfaceSetValue(buffer->surface, CFSTR("DarwinArtStorageRGBA"), kCFBooleanTrue);
    // Vulkan framebuffer coordinates and the imported Metal texture are
    // top-left based; unlike HWUI's Ganesh target they need no storage Y flip.
    IOSurfaceSetValue(buffer->surface, CFSTR("DarwinArtProducerTopLeft"), kCFBooleanTrue);
  }
  return reinterpret_cast<void*>(
      [device newTextureWithDescriptor:descriptor
                             iosurface:buffer->surface
                                 plane:0]);
}

extern "C" void* darwin_art_android_hardware_buffer_vulkan_metal_texture_for_format(
    AHardwareBuffer* buffer, void* metal_device, int32_t vk_format) {
  return CreateVulkanHardwareBufferTexture(buffer, metal_device, vk_format, true);
}

extern "C" void* darwin_art_android_hardware_buffer_vulkan_metal_texture(
    AHardwareBuffer* buffer, void* metal_device) {
  // Capability queries must not change an existing producer's storage format.
  return CreateVulkanHardwareBufferTexture(buffer, metal_device, 37, false);
}

extern "C" void* darwin_art_android_iosurface_metal_texture(
    void* iosurface, uint32_t width, uint32_t height, void* metal_device) {
  if (iosurface == nullptr || metal_device == nullptr || width == 0 ||
      height == 0) {
    return nullptr;
  }
  id<MTLDevice> device = (__bridge id<MTLDevice>)metal_device;
  const uint32_t surface_width = IOSurfaceGetWidth((IOSurfaceRef)iosurface);
  const uint32_t surface_height = IOSurfaceGetHeight((IOSurfaceRef)iosurface);
  if (surface_width == 0 || surface_height == 0) return nullptr;
  CFTypeRef storage_rgba = IOSurfaceCopyValue((IOSurfaceRef)iosurface,
                                             CFSTR("DarwinArtStorageRGBA"));
  const bool rgba = storage_rgba != nullptr && CFEqual(storage_rgba, kCFBooleanTrue);
  if (storage_rgba != nullptr) CFRelease(storage_rgba);
  MTLTextureDescriptor* descriptor =
      [MTLTextureDescriptor texture2DDescriptorWithPixelFormat:
                               (rgba ? MTLPixelFormatRGBA8Unorm : MTLPixelFormatBGRA8Unorm)
                                                         width:surface_width
                                                        height:surface_height
                                                     mipmapped:NO];
  descriptor.storageMode = MTLStorageModeShared;
  descriptor.usage = MTLTextureUsageShaderRead | MTLTextureUsageShaderWrite |
                     MTLTextureUsageRenderTarget;
  id<MTLTexture> texture = [device newTextureWithDescriptor:descriptor
                                                  iosurface:(IOSurfaceRef)iosurface
                                                      plane:0];
  // newTextureWithDescriptor returns a +1 object in this non-ARC translation
  // unit; transfer that ownership to the opaque C handle.
  return reinterpret_cast<void*>(texture);
}

extern "C" void darwin_art_android_metal_texture_release(void* texture) {
  if (texture != nullptr) CFRelease(texture);
}

extern "C" void* darwin_art_android_metal_shared_event_create(
    void* metal_device, uint64_t* signal_value) {
  if (metal_device == nullptr || signal_value == nullptr) return nullptr;
  id<MTLDevice> device = (__bridge id<MTLDevice>)metal_device;
  id<MTLSharedEvent> event = [device newSharedEvent];
  if (event == nil) return nullptr;
  *signal_value = event.signaledValue + 1;
  // newSharedEvent returns a +1 object in this non-ARC translation unit.
  // Transfer that ownership to the opaque C handle released below.
  return reinterpret_cast<void*>(event);
}

extern "C" int darwin_art_android_metal_shared_event_fence_fd(
    void* shared_event, uint64_t signal_value) {
  if (shared_event == nullptr || signal_value == 0) return -1;
  id<MTLSharedEvent> event = (__bridge id<MTLSharedEvent>)shared_event;
  int32_t descriptors[2]{-1, -1};
  if (darwin_art_bionic_socket_broker_pipe(descriptors) != 0) return -1;
  if (event.signaledValue >= signal_value) {
    constexpr uint64_t kSignaledMetalFence =
        UINT64_C(0x44415257494e4653);  // "DARWINFS"
    const intptr_t written = darwin_art_bionic_socket_broker_write(
        descriptors[1], &kSignaledMetalFence, sizeof(kSignaledMetalFence));
    (void)darwin_art_bionic_socket_broker_close(descriptors[1]);
    if (written == static_cast<intptr_t>(sizeof(kSignaledMetalFence)))
      return descriptors[0];
    (void)darwin_art_bionic_socket_broker_close(descriptors[0]);
    return -1;
  }
  const int write_descriptor = descriptors[1];
  // A sync_file may be closed before its GPU work completes. Keep a private
  // read reference until notification finishes: the implementation's pipe
  // must never deliver SIGPIPE to the Android app on fence cancellation.
  const int completion_read_guard =
      darwin_art_bionic_socket_broker_dup(descriptors[0]);
  if (completion_read_guard < 0) {
    (void)darwin_art_bionic_socket_broker_close(descriptors[0]);
    (void)darwin_art_bionic_socket_broker_close(descriptors[1]);
    return -1;
  }
  MTLSharedEventListener* listener = [[MTLSharedEventListener alloc] init];
  if (listener == nil) {
    (void)darwin_art_bionic_socket_broker_close(completion_read_guard);
    (void)darwin_art_bionic_socket_broker_close(descriptors[0]);
    (void)darwin_art_bionic_socket_broker_close(descriptors[1]);
    return -1;
  }
  [event notifyListener:listener
                atValue:signal_value
                  block:^(id<MTLSharedEvent>, uint64_t) {
                    constexpr uint64_t kSignaledMetalFence =
                        UINT64_C(0x44415257494e4653);  // "DARWINFS"
                    (void)darwin_art_bionic_socket_broker_write(
                        write_descriptor, &kSignaledMetalFence,
                        sizeof(kSignaledMetalFence));
                    (void)darwin_art_bionic_socket_broker_close(
                        write_descriptor);
                    (void)darwin_art_bionic_socket_broker_close(
                        completion_read_guard);
                  }];
  [listener release];
  return descriptors[0];
}

extern "C" uint64_t darwin_art_android_metal_shared_event_next_value(
    void* shared_event) {
  if (shared_event == nullptr) return 0;
  id<MTLSharedEvent> event = (__bridge id<MTLSharedEvent>)shared_event;
  return event.signaledValue + 1;
}

extern "C" int darwin_art_android_metal_shared_event_import_fence(
    void* shared_event, uint64_t signal_value, int fence_fd) {
  if (shared_event == nullptr || signal_value == 0 || fence_fd < -1) return -1;
  id<MTLSharedEvent> event = (__bridge id<MTLSharedEvent>)shared_event;
  if (fence_fd == -1) {
    if (event.signaledValue < signal_value) event.signaledValue = signal_value;
    return 0;
  }
  CFRetain((__bridge CFTypeRef)event);
  try {
    std::thread([event, signal_value, fence_fd] {
      const int wait_result = sync_wait(fence_fd, -1);
      (void)darwin_art_bionic_socket_broker_close(fence_fd);
      if (wait_result == 0 && event.signaledValue < signal_value)
        event.signaledValue = signal_value;
      CFRelease((__bridge CFTypeRef)event);
    }).detach();
  } catch (...) {
    CFRelease((__bridge CFTypeRef)event);
    return -1;
  }
  return 0;
}

extern "C" void darwin_art_android_metal_shared_event_release(
    void* shared_event) {
  if (shared_event != nullptr) CFRelease(shared_event);
}

extern "C" void* darwin_art_android_hardware_buffer_native_window_buffer(
    AHardwareBuffer* buffer) {
  return buffer == nullptr ? nullptr : &buffer->native_buffer;
}

extern "C" void AHardwareBuffer_release(AHardwareBuffer* buffer) {
  if (buffer != nullptr &&
      buffer->references.fetch_sub(1, std::memory_order_acq_rel) == 1) {
    {
      std::lock_guard<std::mutex> lock(g_hardware_buffer_mutex);
      std::erase_if(g_hardware_buffer_aliases,
                    [buffer](const auto& entry) {
                      return entry.second == buffer;
                    });
    }
    if (buffer->surface != nullptr) CFRelease(buffer->surface);
    delete buffer;
  }
}

extern "C" AHardwareBuffer*
darwin_art_android_hardware_buffer_from_client_buffer(void* client_buffer) {
  std::lock_guard<std::mutex> lock(g_hardware_buffer_mutex);
  auto found = g_hardware_buffer_aliases.find(client_buffer);
  return found == g_hardware_buffer_aliases.end() ? nullptr : found->second;
}

extern "C" void* darwin_art_android_hardware_buffer_iosurface(
    AHardwareBuffer* buffer) {
  if (std::getenv("DARWIN_ART_DEBUG_GRAPHICS_DSO") != nullptr) {
    std::fprintf(stderr,
                 "ART Android AHardwareBuffer: iosurface buffer=%p surface=%p\n",
                 static_cast<void*>(buffer),
                 buffer == nullptr ? nullptr : static_cast<void*>(buffer->surface));
  }
  return buffer == nullptr ? nullptr : buffer->surface;
}

extern "C" void AHardwareBuffer_describe(const AHardwareBuffer* buffer,
                                           AHardwareBuffer_Desc* out) {
  if (buffer != nullptr && out != nullptr) *out = buffer->description;
}

extern "C" int AHardwareBuffer_lock(AHardwareBuffer* buffer, uint64_t,
                                     int32_t, const ARect*, void** out) {
  if (buffer == nullptr || out == nullptr || buffer->surface == nullptr)
    return -EINVAL;
  std::lock_guard<std::mutex> lock(buffer->mutex);
  if (buffer->locks++ == 0 &&
      IOSurfaceLock(buffer->surface, 0, nullptr) != kIOReturnSuccess) {
    --buffer->locks;
    return -EIO;
  }
  *out = IOSurfaceGetBaseAddress(buffer->surface);
  return *out == nullptr ? -EIO : 0;
}

extern "C" int AHardwareBuffer_lockPlanes(AHardwareBuffer* buffer,
                                            uint64_t usage, int32_t fence,
                                            const ARect* rect,
                                            AHardwareBuffer_Planes* out) {
  if (out == nullptr) return -EINVAL;
  std::memset(out, 0, sizeof(*out));
  void* data = nullptr;
  const int result = AHardwareBuffer_lock(buffer, usage, fence, rect, &data);
  if (result != 0) return result;
  out->planeCount = 1;
  out->planes[0].data = data;
  out->planes[0].pixelStride = BytesPerPixel(buffer->description.format);
  out->planes[0].rowStride =
      static_cast<uint32_t>(IOSurfaceGetBytesPerRow(buffer->surface));
  return 0;
}

extern "C" int AHardwareBuffer_unlock(AHardwareBuffer* buffer,
                                       int32_t* fence) {
  if (buffer == nullptr || buffer->surface == nullptr) return -EINVAL;
  std::lock_guard<std::mutex> lock(buffer->mutex);
  if (buffer->locks == 0) return -EINVAL;
  if (--buffer->locks == 0 &&
      IOSurfaceUnlock(buffer->surface, 0, nullptr) != kIOReturnSuccess)
    return -EIO;
  if (fence != nullptr) *fence = -1;
  return 0;
}

struct HardwareBufferWire {
  uint32_t magic;
  uint32_t surface_id;
  AHardwareBuffer_Desc description;
};

extern "C" int AHardwareBuffer_sendHandleToUnixSocket(
    const AHardwareBuffer* buffer, int socket_fd) {
  if (buffer == nullptr || buffer->surface == nullptr) return -EINVAL;
  const HardwareBufferWire wire{0x44414842u, IOSurfaceGetID(buffer->surface),
                                buffer->description};
  const intptr_t written =
      darwin_art_bionic_socket_broker_write(socket_fd, &wire, sizeof(wire));
  return written == sizeof(wire) ? 0 : -EIO;
}

extern "C" int AHardwareBuffer_recvHandleFromUnixSocket(int socket_fd,
                                                          AHardwareBuffer** out) {
  if (out == nullptr) return -EINVAL;
  *out = nullptr;
  HardwareBufferWire wire{};
  const intptr_t read =
      darwin_art_bionic_socket_broker_read(socket_fd, &wire, sizeof(wire));
  if (read != sizeof(wire) || wire.magic != 0x44414842u) return -EIO;
  IOSurfaceRef surface = IOSurfaceLookup(wire.surface_id);
  if (surface == nullptr) return -ENOENT;
  *out = WrapSurface(surface, wire.description);
  if (*out == nullptr) {
    CFRelease(surface);
    return -ENOMEM;
  }
  return 0;
}

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

int64_t MonotonicNanos() {
  return darwin_art::AndroidUptimeNanos();
}

int DispatchDueFrameCallbacks(ALooper* looper) {
  if (looper == nullptr) return 0;
  std::vector<ChoreographerCallback> callbacks;
  const auto now = std::chrono::steady_clock::now();
  {
    std::lock_guard<std::mutex> lock(looper->mutex);
    auto callback = looper->frame_callbacks.begin();
    while (callback != looper->frame_callbacks.end()) {
      if (callback->deadline > now) {
        ++callback;
        continue;
      }
      callbacks.push_back(*callback);
      callback = looper->frame_callbacks.erase(callback);
    }
  }
  static std::atomic<AVsyncId> next_vsync_id{1};
  static std::atomic<uint32_t> dispatched_count{0};
  for (const ChoreographerCallback& callback : callbacks) {
    const int64_t frame_time = MonotonicNanos();
    const uint32_t sequence =
        dispatched_count.fetch_add(1, std::memory_order_relaxed) + 1;
    if (std::getenv("DARWIN_ART_DEBUG_CHOREOGRAPHER") != nullptr &&
        sequence <= 64) {
      std::fprintf(stderr,
                   "ART Android Choreographer: dispatch sequence=%u kind=%u "
                   "frame_ns=%lld pid=%d\n",
                   sequence, static_cast<unsigned>(callback.kind),
                   static_cast<long long>(frame_time), getpid());
    }
    switch (callback.kind) {
      case ChoreographerCallbackKind::kFrame:
        reinterpret_cast<AChoreographer_frameCallback>(callback.callback)(
            static_cast<long>(frame_time), callback.data);
        break;
      case ChoreographerCallbackKind::kFrame64:
        reinterpret_cast<AChoreographer_frameCallback64>(callback.callback)(
            frame_time, callback.data);
        break;
      case ChoreographerCallbackKind::kVsync: {
        AChoreographerFrameCallbackData data{
            frame_time, next_vsync_id.fetch_add(1, std::memory_order_relaxed),
            frame_time + 16'666'667, frame_time + 15'000'000};
        reinterpret_cast<AChoreographer_vsyncCallback>(callback.callback)(
            &data, callback.data);
        break;
      }
      case ChoreographerCallbackKind::kRefreshRate:
        reinterpret_cast<AChoreographer_refreshRateCallback>(callback.callback)(
            16'666'667, callback.data);
        break;
    }
  }
  return static_cast<int>(callbacks.size());
}

int NextFrameCallbackDelayMillis(ALooper* looper) {
  if (looper == nullptr) return -1;
  std::lock_guard<std::mutex> lock(looper->mutex);
  if (looper->frame_callbacks.empty()) return -1;
  const auto next = std::min_element(
      looper->frame_callbacks.begin(), looper->frame_callbacks.end(),
      [](const auto& left, const auto& right) {
        return left.deadline < right.deadline;
      });
  const auto delay = next->deadline - std::chrono::steady_clock::now();
  if (delay <= std::chrono::steady_clock::duration::zero()) return 0;
  const auto milliseconds =
      std::chrono::duration_cast<std::chrono::milliseconds>(delay).count();
  return static_cast<int>(std::min<int64_t>(milliseconds + 1, INT_MAX));
}

void ScheduleChoreographerCallback(AChoreographer* choreographer,
                                   ChoreographerCallbackKind kind,
                                   void* callback, void* data,
                                   uint32_t delay_millis) {
  if (choreographer == nullptr || choreographer->looper == nullptr ||
      callback == nullptr)
    return;
  const auto deadline = std::chrono::steady_clock::now() +
                        std::chrono::milliseconds(delay_millis) +
                        (delay_millis == 0 ? std::chrono::milliseconds(16)
                                           : std::chrono::milliseconds(0));
  static std::atomic<uint32_t> scheduled_count{0};
  const uint32_t sequence =
      scheduled_count.fetch_add(1, std::memory_order_relaxed) + 1;
  if (std::getenv("DARWIN_ART_DEBUG_CHOREOGRAPHER") != nullptr &&
      sequence <= 64) {
    std::fprintf(stderr,
                 "ART Android Choreographer: schedule sequence=%u kind=%u "
                 "delay_ms=%u pid=%d\n",
                 sequence, static_cast<unsigned>(kind), delay_millis,
                 getpid());
  }
  {
    std::lock_guard<std::mutex> lock(choreographer->looper->mutex);
    choreographer->looper->frame_callbacks.push_back(
        {deadline, kind, callback, data});
  }
  ALooper_wake(choreographer->looper);
}
}  // namespace

extern "C" int32_t AInputEvent_getType(const AInputEvent* event) {
  event = Input(event); return event == nullptr ? 0 : event->type;
}
extern "C" int32_t AInputEvent_getSource(const AInputEvent* event) {
  event = Input(event); return event == nullptr ? 0 : event->source;
}
extern "C" void AInputEvent_release(const AInputEvent* event) {
  auto* owned = const_cast<AInputEvent*>(Input(event));
  if (owned != nullptr &&
      owned->references.fetch_sub(1, std::memory_order_acq_rel) == 1)
    delete owned;
}
extern "C" int32_t AMotionEvent_getAction(const AInputEvent* e) { e=Input(e); return e?e->action:0; }
extern "C" int32_t AMotionEvent_getMetaState(const AInputEvent* e) { e=Input(e); return e?e->meta_state:0; }
extern "C" int32_t AMotionEvent_getButtonState(const AInputEvent* e) { e=Input(e); return e?e->button_state:0; }
extern "C" int32_t AMotionEvent_getClassification(const AInputEvent* e) { e=Input(e); return e?e->classification:0; }
extern "C" int64_t AMotionEvent_getDownTime(const AInputEvent* e) { e=Input(e); return e?e->down_time:0; }
extern "C" int64_t AMotionEvent_getEventTime(const AInputEvent* e) { e=Input(e); return e?e->event_time:0; }
extern "C" size_t AMotionEvent_getPointerCount(const AInputEvent* e) { return Input(e)?1:0; }
extern "C" int32_t AMotionEvent_getPointerId(const AInputEvent* e, size_t i) { e=Input(e); return e&&i==0?e->pointer_id:-1; }
extern "C" int32_t AMotionEvent_getToolType(const AInputEvent* e, size_t i) { e=Input(e); return e&&i==0?e->tool_type:AMOTION_EVENT_TOOL_TYPE_UNKNOWN; }
extern "C" float AMotionEvent_getRawX(const AInputEvent* e, size_t i) { e=Input(e); return e&&i==0?e->raw_x:0; }
extern "C" float AMotionEvent_getRawY(const AInputEvent* e, size_t i) { e=Input(e); return e&&i==0?e->raw_y:0; }
extern "C" float AMotionEvent_getX(const AInputEvent* e, size_t i) { e=Input(e); return e&&i==0?e->x:0; }
extern "C" float AMotionEvent_getY(const AInputEvent* e, size_t i) { e=Input(e); return e&&i==0?e->y:0; }
extern "C" float AMotionEvent_getPressure(const AInputEvent* e, size_t i) { e=Input(e); return e&&i==0?e->pressure:0; }
extern "C" float AMotionEvent_getTouchMajor(const AInputEvent* e, size_t i) { e=Input(e); return e&&i==0?e->touch_major:0; }
extern "C" float AMotionEvent_getTouchMinor(const AInputEvent* e, size_t i) { e=Input(e); return e&&i==0?e->touch_minor:0; }
extern "C" float AMotionEvent_getOrientation(const AInputEvent* e, size_t i) { e=Input(e); return e&&i==0?e->orientation:0; }
extern "C" float AMotionEvent_getAxisValue(const AInputEvent* e, int32_t axis, size_t i) { return i==0?MotionAxis(e,axis):0; }
extern "C" size_t AMotionEvent_getHistorySize(const AInputEvent*) { return 0; }
extern "C" int64_t AMotionEvent_getHistoricalEventTime(const AInputEvent*, size_t) { return 0; }
extern "C" float AMotionEvent_getHistoricalX(const AInputEvent*, size_t, size_t) { return 0; }
extern "C" float AMotionEvent_getHistoricalY(const AInputEvent*, size_t, size_t) { return 0; }
extern "C" float AMotionEvent_getHistoricalTouchMajor(const AInputEvent*, size_t, size_t) { return 0; }

extern "C" ALooper* ALooper_forThread() { return g_thread_looper; }
extern "C" ALooper* ALooper_prepare(int options) {
  if (g_thread_looper == nullptr) {
    g_thread_looper = new (std::nothrow) ALooper();
    if (g_thread_looper != nullptr) {
      g_thread_looper->options = options;
      // EFD_NONBLOCK | EFD_CLOEXEC in Android's ABI. The broker implements
      // eventfd over a datagram socketpair while preserving guest fd identity.
      g_thread_looper->wake_fd =
          darwin_art_bionic_socket_broker_eventfd(0, 0x80800);
      if (g_thread_looper->wake_fd < 0) {
        delete g_thread_looper;
        g_thread_looper = nullptr;
      }
    }
  }
  return g_thread_looper;
}
extern "C" void ALooper_acquire(ALooper* looper) {
  if (looper != nullptr) looper->references.fetch_add(1, std::memory_order_relaxed);
}
extern "C" void ALooper_release(ALooper* looper) {
  // The thread association is a process-lifetime safety reference. This
  // mirrors ART's other opaque compatibility tokens and prevents a borrowed
  // ALooper_forThread pointer from becoming dangling.
  if (looper != nullptr && looper->references.load(std::memory_order_acquire) > 1)
    looper->references.fetch_sub(1, std::memory_order_acq_rel);
}
int AddLooperFd(ALooper* looper, int fd, int ident, int events,
                ALooper_callbackFunc callback, void* data,
                std::shared_ptr<void> callback_owner) {
  if (looper == nullptr || fd < 0 ||
      (callback == nullptr &&
       (looper->options & ALOOPER_PREPARE_ALLOW_NON_CALLBACKS) == 0))
    return -1;
  {
    std::lock_guard<std::mutex> lock(looper->mutex);
    if (std::getenv("DARWIN_ART_DEBUG_SLOW_FRAME") != nullptr) {
      const int status_flags =
          darwin_art_bionic_socket_broker_fcntl(fd, /*F_GETFL*/ 3, 0);
      std::fprintf(stderr,
                   "DARWIN_ART looper-add-fd tid=%llu looper=%p fd=%d ident=%d events=0x%x "
                   "callback=%p (%s) caller=%p (%s) data=%p status_flags=0x%x\n",
                   static_cast<unsigned long long>(CurrentThreadId()), looper,
                   fd, ident, events, reinterpret_cast<void*>(callback),
                   NativeAddressDescription(reinterpret_cast<void*>(callback)).c_str(),
                   __builtin_return_address(0),
                   NativeAddressDescription(__builtin_return_address(0)).c_str(),
                   data,
                   status_flags);
    }
    auto found = std::find_if(looper->registrations.begin(),
                              looper->registrations.end(),
                              [fd](const auto& value) { return value.fd == fd; });
    const ALooperRegistration registration{
        fd, ident, events, callback, data, looper->next_generation++, 0,
        std::move(callback_owner)};
    if (found == looper->registrations.end())
      looper->registrations.push_back(registration);
    else
      *found = registration;
  }
  // The registration can be published by a Binder or render worker while the
  // owner thread is blocked in an earlier poll snapshot. Android's Looper
  // wakes that poll so the next iteration observes the new request set.
  ALooper_wake(looper);
  return 1;
}
extern "C" int ALooper_addFd(ALooper* looper, int fd, int ident, int events,
                              ALooper_callbackFunc callback, void* data) {
  return AddLooperFd(looper, fd, ident, events, callback, data, {});
}

bool RemoveLooperFdGeneration(ALooper* looper, int fd, uint64_t generation) {
  if (looper == nullptr) return false;
  bool removed = false;
  {
    std::lock_guard<std::mutex> lock(looper->mutex);
    const auto old_size = looper->registrations.size();
    std::erase_if(looper->registrations, [fd, generation](const auto& value) {
      return value.fd == fd && value.generation == generation;
    });
    removed = looper->registrations.size() != old_size;
  }
  if (removed) ALooper_wake(looper);
  return removed;
}
extern "C" int ALooper_removeFd(ALooper* looper, int fd) {
  if (looper == nullptr) return -1;
  bool removed = false;
  {
    std::lock_guard<std::mutex> lock(looper->mutex);
    const auto old_size = looper->registrations.size();
    std::erase_if(looper->registrations,
                  [fd](const auto& value) { return value.fd == fd; });
    removed = looper->registrations.size() != old_size;
  }
  if (removed) ALooper_wake(looper);
  return removed ? 1 : 0;
}
extern "C" void ALooper_wake(ALooper* looper) {
  if (looper == nullptr || looper->wake_fd < 0) return;
  const uint64_t value = 1;
  (void)darwin_art_bionic_socket_broker_write(looper->wake_fd, &value,
                                               sizeof(value));
}
extern "C" int ALooper_pollOnce(int timeout_ms, int* out_fd,
                                 int* out_events, void** out_data) {
  if (out_fd != nullptr) *out_fd = 0;
  if (out_events != nullptr) *out_events = 0;
  if (out_data != nullptr) *out_data = nullptr;
  ALooper* looper = g_thread_looper;
  if (looper == nullptr) return ALOOPER_POLL_ERROR;
  if (g_looper_callback_depth != 0 &&
      std::getenv("DARWIN_ART_DEBUG_SLOW_FRAME") != nullptr) {
    std::fprintf(stderr,
                 "DARWIN_ART nested-looper-poll depth=%u timeout_ms=%d\n",
                 g_looper_callback_depth, timeout_ms);
  }
  if (DispatchDueFrameCallbacks(looper) > 0) return ALOOPER_POLL_CALLBACK;
  const int frame_delay = NextFrameCallbackDelayMillis(looper);
  if (frame_delay >= 0 && (timeout_ms < 0 || frame_delay < timeout_ms))
    timeout_ms = frame_delay;
  std::vector<ALooperRegistration> registrations;
  {
    std::lock_guard<std::mutex> lock(looper->mutex);
    registrations = looper->registrations;
  }
  std::vector<DarwinArtBionicPollFd> descriptors;
  descriptors.reserve(registrations.size() + 1);
  descriptors.push_back({looper->wake_fd, 0x0001, 0});
  for (const auto& registration : registrations) {
    int16_t poll_events = 0;
    if ((registration.events & ALOOPER_EVENT_INPUT) != 0) poll_events |= 0x0001;
    if ((registration.events & ALOOPER_EVENT_OUTPUT) != 0) poll_events |= 0x0004;
    descriptors.push_back({registration.fd, poll_events, 0});
  }
  if (std::getenv("DARWIN_ART_DEBUG_INPUT_LATENCY") != nullptr) {
    static thread_local size_t logged_registration_count = 0;
    if (logged_registration_count != registrations.size()) {
      logged_registration_count = registrations.size();
      std::fprintf(stderr,
                   "ART Android Looper poll-set tid=%llu looper=%p wake_fd=%d registrations=%zu",
                   static_cast<unsigned long long>(CurrentThreadId()), looper,
                   looper->wake_fd, registrations.size());
      for (const auto& registration : registrations) {
        std::fprintf(stderr, " fd=%d", registration.fd);
      }
      std::fputc('\n', stderr);
    }
  }
  int ready = -1;
  do {
    ready = darwin_art_bionic_socket_broker_poll(
        descriptors.data(), descriptors.size(), timeout_ms);
  } while (ready < 0 && errno == EINTR);
  if (ready > 0 &&
      std::getenv("DARWIN_ART_DEBUG_INPUT_LATENCY") != nullptr) {
    std::fprintf(stderr, "ART Android Looper poll-ready tid=%llu ready=%d",
                 static_cast<unsigned long long>(CurrentThreadId()), ready);
    for (const auto& descriptor : descriptors) {
      if (descriptor.revents != 0) {
        std::fprintf(stderr, " fd=%d revents=0x%x", descriptor.fd,
                     descriptor.revents);
      }
    }
    std::fputc('\n', stderr);
  }
  if (ready < 0) return ALOOPER_POLL_ERROR;
  if (ready == 0) {
    return DispatchDueFrameCallbacks(looper) > 0 ? ALOOPER_POLL_CALLBACK
                                                 : ALOOPER_POLL_TIMEOUT;
  }
  const bool received_wake = (descriptors[0].revents & 0x0001) != 0;
  if (received_wake) {
    uint64_t value = 0;
    (void)darwin_art_bionic_socket_broker_read(looper->wake_fd, &value,
                                                sizeof(value));
  }
  bool invoked_callback = false;
  bool skipped_host_callback = false;
  for (size_t index = 1; index < descriptors.size(); ++index) {
    const int16_t poll_events = descriptors[index].revents;
    if (poll_events == 0) continue;
    int events = 0;
    if ((poll_events & 0x0001) != 0) events |= ALOOPER_EVENT_INPUT;
    if ((poll_events & 0x0004) != 0) events |= ALOOPER_EVENT_OUTPUT;
    if ((poll_events & 0x0008) != 0) events |= ALOOPER_EVENT_ERROR;
    if ((poll_events & 0x0010) != 0) events |= ALOOPER_EVENT_HANGUP;
    if ((poll_events & 0x0020) != 0) events |= ALOOPER_EVENT_INVALID;
    const auto& registration = registrations[index - 1];
    if (registration.callback != nullptr) {
      bool registration_current = false;
      bool already_dispatched = false;
      {
        std::lock_guard<std::mutex> lock(looper->mutex);
        const auto found = std::find_if(
            looper->registrations.begin(), looper->registrations.end(),
            [&registration](const auto& current) {
              return current.fd == registration.fd &&
                     current.generation == registration.generation;
            });
        if (found != looper->registrations.end()) {
          registration_current = true;
          if (g_host_looper_turn_active) {
            already_dispatched = found->last_host_turn == g_host_looper_turn;
            if (!already_dispatched)
              found->last_host_turn = g_host_looper_turn;
          }
        }
      }
      if (!registration_current) continue;
      if (already_dispatched) {
        skipped_host_callback = true;
        continue;
      }
      invoked_callback = true;
      if (std::getenv("DARWIN_ART_DEBUG_SLOW_FRAME") != nullptr &&
          std::getenv("DARWIN_ART_DEBUG_CALLBACK_VTABLE") != nullptr) {
        // Chromium's NativeChildProcessService callback is a small guest
        // thunk that dispatches through the service object's vtable. Read the
        // same slot used by that thunk so a slow callback can be mapped back
        // to its concrete method without changing callback affinity.
        uintptr_t object_vtable = 0;
        if (registration.data != nullptr) {
          std::memcpy(&object_vtable, registration.data,
                      sizeof(object_vtable));
        }
        int32_t vtable_offset = 0;
        if (object_vtable != 0) {
          std::memcpy(&vtable_offset,
                      reinterpret_cast<const void*>(object_vtable + 0x2c),
                      sizeof(vtable_offset));
        }
        const uintptr_t callback_target =
            object_vtable == 0
                ? 0
                : object_vtable + static_cast<int64_t>(vtable_offset);
        std::fprintf(stderr,
                     "DARWIN_ART looper-callback-target pid=%d process=%s "
                     "tid=%llu fd=%d callback=%p data=%p vtable=%p "
                     "slot_offset=%d target=%p\n",
                     getpid(),
                     std::getenv("DARWIN_ART_APK_PROCESS_NAME") == nullptr
                         ? "<main>"
                         : std::getenv("DARWIN_ART_APK_PROCESS_NAME"),
                     static_cast<unsigned long long>(CurrentThreadId()),
                     registration.fd,
                     reinterpret_cast<void*>(registration.callback),
                     registration.data,
                     reinterpret_cast<void*>(object_vtable), vtable_offset,
                     reinterpret_cast<void*>(callback_target));
      }
      const auto callback_started = std::chrono::steady_clock::now();
      const uint64_t callback_cpu_started = ThreadCpuNanos();
      ++g_looper_callback_depth;
      const int callback_result =
          registration.callback(registration.fd, events, registration.data);
      --g_looper_callback_depth;
      if (std::getenv("DARWIN_ART_DEBUG_SLOW_FRAME") != nullptr) {
        const auto callback_us =
            std::chrono::duration_cast<std::chrono::microseconds>(
                std::chrono::steady_clock::now() - callback_started)
                .count();
        if (callback_us >= 100'000) {
          const uint64_t callback_cpu_finished = ThreadCpuNanos();
          const uint64_t callback_cpu_us =
              callback_cpu_finished >= callback_cpu_started
                  ? (callback_cpu_finished - callback_cpu_started) / 1000
                  : 0;
          std::fprintf(stderr,
                       "DARWIN_ART slow-native-callback pid=%d process=%s "
                       "tid=%llu fd=%d events=0x%x "
                       "callback=%p data=%p result=%d elapsed_us=%lld "
                       "cpu_us=%llu callback_desc=%s\n",
                       getpid(),
                       std::getenv("DARWIN_ART_APK_PROCESS_NAME") == nullptr
                           ? "<main>"
                           : std::getenv("DARWIN_ART_APK_PROCESS_NAME"),
                       static_cast<unsigned long long>(CurrentThreadId()),
                       registration.fd, events,
                       reinterpret_cast<void*>(registration.callback),
                       registration.data, callback_result,
                       static_cast<long long>(callback_us),
                       static_cast<unsigned long long>(callback_cpu_us),
                       NativeAddressDescription(reinterpret_cast<void*>(
                           registration.callback)).c_str());
        }
      }
      if (callback_result == 0 ||
          (events & ALOOPER_EVENT_INVALID) != 0)
        (void)RemoveLooperFdGeneration(looper, registration.fd,
                                       registration.generation);
      // AOSP Looper::pollInner walks every callback response captured by the
      // same poll before returning POLL_CALLBACK. Discarding the remainder
      // lets an always-ready earlier descriptor starve InputChannel forever.
      // The outer host drain still bounds independent poll iterations.
      continue;
    } else {
      if (out_fd != nullptr) *out_fd = registration.fd;
      if (out_events != nullptr) *out_events = events;
      if (out_data != nullptr) *out_data = registration.data;
      return registration.ident;
    }
  }
  if (invoked_callback) return ALOOPER_POLL_CALLBACK;
  if (skipped_host_callback) return ALOOPER_POLL_TIMEOUT;
  return received_wake ? ALOOPER_POLL_WAKE : ALOOPER_POLL_TIMEOUT;
}

extern "C" AChoreographer* AChoreographer_getInstance() {
  ALooper* looper = ALooper_forThread();
  if (looper == nullptr) return nullptr;
  if (g_thread_choreographer == nullptr) {
    g_thread_choreographer = new (std::nothrow) AChoreographer();
    if (g_thread_choreographer != nullptr)
      g_thread_choreographer->looper = looper;
  }
  return g_thread_choreographer;
}

extern "C" void AChoreographer_postFrameCallback(
    AChoreographer* choreographer, AChoreographer_frameCallback callback,
    void* data) {
  ScheduleChoreographerCallback(choreographer,
                               ChoreographerCallbackKind::kFrame,
                               reinterpret_cast<void*>(callback), data, 0);
}

extern "C" void AChoreographer_postFrameCallbackDelayed(
    AChoreographer* choreographer, AChoreographer_frameCallback callback,
    void* data, long delay_millis) {
  ScheduleChoreographerCallback(
      choreographer, ChoreographerCallbackKind::kFrame,
      reinterpret_cast<void*>(callback), data,
      delay_millis <= 0 ? 0 : static_cast<uint32_t>(delay_millis));
}

extern "C" void AChoreographer_postFrameCallback64(
    AChoreographer* choreographer, AChoreographer_frameCallback64 callback,
    void* data) {
  ScheduleChoreographerCallback(choreographer,
                               ChoreographerCallbackKind::kFrame64,
                               reinterpret_cast<void*>(callback), data, 0);
}

extern "C" void AChoreographer_postFrameCallbackDelayed64(
    AChoreographer* choreographer, AChoreographer_frameCallback64 callback,
    void* data, uint32_t delay_millis) {
  ScheduleChoreographerCallback(choreographer,
                               ChoreographerCallbackKind::kFrame64,
                               reinterpret_cast<void*>(callback), data,
                               delay_millis);
}

extern "C" void AChoreographer_postVsyncCallback(
    AChoreographer* choreographer, AChoreographer_vsyncCallback callback,
    void* data) {
  ScheduleChoreographerCallback(choreographer,
                               ChoreographerCallbackKind::kVsync,
                               reinterpret_cast<void*>(callback), data, 0);
}

extern "C" void AChoreographer_registerRefreshRateCallback(
    AChoreographer* choreographer,
    AChoreographer_refreshRateCallback callback, void* data) {
  if (choreographer == nullptr || callback == nullptr) return;
  {
    std::lock_guard<std::mutex> lock(choreographer->mutex);
    choreographer->refresh_callbacks.emplace_back(callback, data);
  }
  ScheduleChoreographerCallback(choreographer,
                               ChoreographerCallbackKind::kRefreshRate,
                               reinterpret_cast<void*>(callback), data, 0);
}

extern "C" void AChoreographer_unregisterRefreshRateCallback(
    AChoreographer* choreographer,
    AChoreographer_refreshRateCallback callback, void* data) {
  if (choreographer == nullptr) return;
  std::lock_guard<std::mutex> lock(choreographer->mutex);
  std::erase(choreographer->refresh_callbacks, std::make_pair(callback, data));
}

extern "C" int64_t AChoreographerFrameCallbackData_getFrameTimeNanos(
    const AChoreographerFrameCallbackData* data) {
  return data == nullptr ? 0 : data->frame_time_nanos;
}
extern "C" size_t AChoreographerFrameCallbackData_getFrameTimelinesLength(
    const AChoreographerFrameCallbackData*) { return 1; }
extern "C" size_t
AChoreographerFrameCallbackData_getPreferredFrameTimelineIndex(
    const AChoreographerFrameCallbackData*) { return 0; }
extern "C" AVsyncId AChoreographerFrameCallbackData_getFrameTimelineVsyncId(
    const AChoreographerFrameCallbackData* data, size_t index) {
  return data == nullptr || index != 0 ? -1 : data->vsync_id;
}
extern "C" int64_t
AChoreographerFrameCallbackData_getFrameTimelineExpectedPresentationTimeNanos(
    const AChoreographerFrameCallbackData* data, size_t index) {
  return data == nullptr || index != 0 ? 0
                                      : data->expected_presentation_time_nanos;
}
extern "C" int64_t
AChoreographerFrameCallbackData_getFrameTimelineDeadlineNanos(
    const AChoreographerFrameCallbackData* data, size_t index) {
  return data == nullptr || index != 0 ? 0 : data->deadline_nanos;
}

extern "C" int darwin_art_android_platform_poll_current_looper() {
  if (g_thread_looper == nullptr) return 0;
  int dispatched = 0;
  ++g_host_looper_turn;
  g_host_looper_turn_active = true;
  // Keep the bounded host drain separate from the public ALooper ABI. A
  // callback is still sequence-affine to the registering Looper thread, but
  // the small per-turn budget prevents a ready native-fd burst from consuming
  // an entire ART/UI turn before Java MessageQueue and Choreographer work run.
  constexpr int kHostCallbackBudget = 8;
  int status = 0;
  for (int iteration = 0; iteration < kHostCallbackBudget; ++iteration) {
    const int result = ALooper_pollOnce(0, nullptr, nullptr, nullptr);
    if (result == ALOOPER_POLL_CALLBACK || result == ALOOPER_POLL_WAKE) {
      ++dispatched;
      continue;
    }
    if (result == ALOOPER_POLL_TIMEOUT) {
      status = dispatched;
      break;
    }
    status = result == ALOOPER_POLL_ERROR ? -1 : dispatched;
    break;
  }
  g_host_looper_turn_active = false;
  return status == 0 ? dispatched : status;
}

extern "C" void* darwin_art_android_platform_prepare_current_looper() {
  return ALooper_prepare(0);
}

extern "C" int darwin_art_android_platform_poll_current_looper_timeout(
    int timeout_ms) {
  return ALooper_pollOnce(timeout_ms, nullptr, nullptr, nullptr);
}

extern "C" int darwin_art_android_platform_wait_current_looper(
    int timeout_ms) {
  if (g_thread_looper == nullptr) return 0;
  timeout_ms = std::clamp(timeout_ms, 0, 16);
  ++g_host_looper_turn;
  g_host_looper_turn_active = true;
  const int result = ALooper_pollOnce(timeout_ms, nullptr, nullptr, nullptr);
  g_host_looper_turn_active = false;
  return result == ALOOPER_POLL_ERROR ? -1 : 0;
}

extern "C" void darwin_art_android_platform_wake_looper(void* looper) {
  ALooper_wake(static_cast<ALooper*>(looper));
}

extern "C" int darwin_art_android_platform_add_fd(
    void* looper, int fd, int ident, int events,
    DarwinArtLooperCallback callback, void* data) {
  if (looper == nullptr || callback == nullptr) return 0;
  return ALooper_addFd(static_cast<ALooper*>(looper), fd, ident, events,
                       callback, data);
}

extern "C" int darwin_art_android_platform_add_fd_owned(
    void* looper, int fd, int ident, int events,
    DarwinArtLooperCallback callback, void* data, void* owner,
    DarwinArtLooperOwnerRelease release) {
  std::shared_ptr<void> callback_owner(
      owner, [release](void* value) {
        if (release != nullptr && value != nullptr) release(value);
      });
  if (looper == nullptr || callback == nullptr || owner == nullptr ||
      release == nullptr) {
    return 0;
  }
  return AddLooperFd(static_cast<ALooper*>(looper), fd, ident, events,
                     callback, data, std::move(callback_owner));
}

extern "C" int darwin_art_android_platform_remove_fd(void* looper, int fd) {
  return looper == nullptr ? 0
                           : ALooper_removeFd(static_cast<ALooper*>(looper), fd);
}

extern "C" ASensorManager* ASensorManager_getInstanceForPackage(const char*) {
  return &g_sensor_manager;
}

extern "C" ASensorManager* ASensorManager_getInstance() {
  return &g_sensor_manager;
}

extern "C" int ASensorManager_getSensorList(ASensorManager*, ASensorList* list) {
  if (list != nullptr) *list = nullptr;
  return 0;
}
extern "C" const ASensor* ASensorManager_getDefaultSensor(ASensorManager*, int) {
  return nullptr;
}
extern "C" ASensorEventQueue* ASensorManager_createEventQueue(
    ASensorManager* manager, ALooper* looper, int, ALooper_callbackFunc, void*) {
  if (manager == nullptr || looper == nullptr) return nullptr;
  return new (std::nothrow) ASensorEventQueue();
}
extern "C" int ASensorManager_destroyEventQueue(ASensorManager*, ASensorEventQueue* queue) {
  delete queue; return 0;
}
extern "C" int ASensorEventQueue_enableSensor(ASensorEventQueue*, const ASensor*) { return -EINVAL; }
extern "C" int ASensorEventQueue_disableSensor(ASensorEventQueue*, const ASensor*) { return -EINVAL; }
extern "C" int ASensorEventQueue_setEventRate(ASensorEventQueue*, const ASensor*, int32_t) { return -EINVAL; }
extern "C" ssize_t ASensorEventQueue_getEvents(ASensorEventQueue*, ASensorEvent*, size_t) { return 0; }
extern "C" int ASensorEventQueue_hasEvents(ASensorEventQueue*) { return 0; }
extern "C" const char* ASensor_getName(const ASensor*) { return ""; }
extern "C" float ASensor_getResolution(const ASensor*) { return 0.0f; }
extern "C" int ASensor_getType(const ASensor*) { return 0; }
extern "C" const char* ASensor_getVendor(const ASensor*) { return ""; }
extern "C" int ASensor_getMinDelay(ASensor const*) { return 0; }

extern "C" ASurfaceControl* ASurfaceControl_createFromWindow(
    ANativeWindow* window, const char* name) {
  uint32_t owner_process_id = 0;
  uint32_t layer_id = 0;
  if (darwin_art_android_ANativeWindow_get_imported_surface_identity(
          window, &owner_process_id, &layer_id)) {
    auto* child = reinterpret_cast<SurfaceControl*>(
        CreateSurfaceControl(nullptr, name, false));
    if (child != nullptr) {
      std::lock_guard<std::mutex> lock(g_surface_controls_mutex);
      child->imported_parent_owner_process_id = owner_process_id;
      child->imported_parent_layer_id = layer_id;
      if (DebugSurfaceTransactions()) {
        std::fprintf(stderr,
                     "ART Android SurfaceControl: create-from-window "
                     "layer=%u imported-parent=%u:%u name=%s\n",
                     child->layer_id, owner_process_id, layer_id,
                     name == nullptr ? "" : name);
      }
    }
    return reinterpret_cast<ASurfaceControl*>(child);
  }
  return CreateSurfaceControl(nullptr, name, true);
}

extern "C" ASurfaceControl* ASurfaceControl_create(ASurfaceControl* parent,
                                                     const char* name) {
  return CreateSurfaceControl(parent, name, false);
}

extern "C" void ASurfaceControl_acquire(ASurfaceControl* opaque) {
  auto* control = reinterpret_cast<SurfaceControl*>(opaque);
  if (control != nullptr) {
    control->references.fetch_add(1, std::memory_order_relaxed);
  }
}

extern "C" void ASurfaceControl_release(ASurfaceControl* opaque) {
  auto* control = reinterpret_cast<SurfaceControl*>(opaque);
  if (control != nullptr &&
      control->references.fetch_sub(1, std::memory_order_acq_rel) == 1) {
    SurfaceControl* parent = nullptr;
    SurfaceControl* relative_to = nullptr;
    {
      std::lock_guard<std::mutex> lock(g_surface_controls_mutex);
      std::erase(g_surface_controls, control);
      parent = std::exchange(control->parent, nullptr);
      relative_to = std::exchange(control->relative_to, nullptr);
      for (SurfaceControl* child : g_surface_controls) {
        if (child->parent == control) child->parent = nullptr;
        if (child->relative_to == control) child->relative_to = nullptr;
      }
    }
    if (control->buffer != nullptr) AHardwareBuffer_release(control->buffer);
    delete control;
    if (relative_to != nullptr) {
      ASurfaceControl_release(reinterpret_cast<ASurfaceControl*>(relative_to));
    }
    if (parent != nullptr) {
      ASurfaceControl_release(reinterpret_cast<ASurfaceControl*>(parent));
    }
  }
}

extern "C" ASurfaceTransaction* ASurfaceTransaction_create() {
  return reinterpret_cast<ASurfaceTransaction*>(new (std::nothrow) SurfaceTransaction());
}

extern "C" void ASurfaceTransaction_delete(ASurfaceTransaction* opaque) {
  auto* transaction = reinterpret_cast<SurfaceTransaction*>(opaque);
  DiscardTransactionCallbacks(transaction);
  ReleaseTransactionBuffers(transaction);
  ReleaseTransactionControls(transaction);
  delete transaction;
}

extern "C" void darwin_art_android_surface_transaction_clear(
    void* opaque) {
  auto* transaction = reinterpret_cast<SurfaceTransaction*>(opaque);
  if (transaction == nullptr) return;
  DiscardTransactionCallbacks(transaction);
  ReleaseTransactionBuffers(transaction);
  ReleaseTransactionControls(transaction);
  transaction->updates.clear();
}

extern "C" void darwin_art_android_surface_transaction_merge(
    void* opaque_destination, void* opaque_source) {
  auto* destination = reinterpret_cast<SurfaceTransaction*>(opaque_destination);
  auto* source = reinterpret_cast<SurfaceTransaction*>(opaque_source);
  if (destination == nullptr || source == nullptr || destination == source) {
    return;
  }
  for (ASurfaceControl* control : source->controls) {
    if (std::find(destination->controls.begin(), destination->controls.end(),
                  control) == destination->controls.end()) {
      destination->controls.push_back(control);
    } else {
      ASurfaceControl_release(control);
    }
  }
  for (auto& incoming : source->updates) {
    SurfaceTransaction::Update* merged =
        FindUpdate(destination, incoming.opaque);
    if (merged == nullptr) continue;
    if (incoming.has_buffer) {
      DiscardBufferCallbacks(destination, incoming.opaque);
      if (merged->buffer != nullptr) AHardwareBuffer_release(merged->buffer);
      if (merged->acquire_fence >= 0) {
        (void)darwin_art_bionic_socket_broker_close(merged->acquire_fence);
      }
      merged->buffer = incoming.buffer;
      merged->acquire_fence = incoming.acquire_fence;
      merged->has_buffer = true;
      incoming.buffer = nullptr;
      incoming.acquire_fence = -1;
    }
#define MERGE_SURFACE_FIELD(flag, field) \
    if (incoming.flag) {                  \
      merged->flag = true;                \
      merged->field = incoming.field;     \
    }
    MERGE_SURFACE_FIELD(has_geometry, source)
    if (incoming.has_geometry) merged->destination = incoming.destination;
    MERGE_SURFACE_FIELD(has_crop, crop)
    MERGE_SURFACE_FIELD(has_visibility, visible)
    MERGE_SURFACE_FIELD(has_position, position_x)
    if (incoming.has_position) merged->position_y = incoming.position_y;
    MERGE_SURFACE_FIELD(has_transform, transform)
    MERGE_SURFACE_FIELD(has_z_order, z_order)
    MERGE_SURFACE_FIELD(has_scale, scale_x)
    if (incoming.has_scale) merged->scale_y = incoming.scale_y;
    MERGE_SURFACE_FIELD(has_alpha, alpha)
    MERGE_SURFACE_FIELD(has_parent, parent)
#undef MERGE_SURFACE_FIELD
    if (incoming.has_z_order || incoming.has_relative_layer) {
      merged->has_z_order = incoming.has_z_order;
      merged->has_relative_layer = incoming.has_relative_layer;
      merged->relative_to = incoming.relative_to;
      merged->z_order = incoming.z_order;
    }
    if (incoming.has_damage) {
      merged->has_damage = true;
      merged->damage = std::move(incoming.damage);
    }
    if (incoming.has_transparent_region) {
      merged->has_transparent_region = true;
      merged->transparent_region = std::move(incoming.transparent_region);
    }
  }
  destination->commits.insert(destination->commits.end(),
                              source->commits.begin(), source->commits.end());
  destination->completes.insert(destination->completes.end(),
                                source->completes.begin(), source->completes.end());
  destination->discards.insert(destination->discards.end(),
                               source->discards.begin(), source->discards.end());
  destination->buffer_callbacks.insert(destination->buffer_callbacks.end(),
      source->buffer_callbacks.begin(), source->buffer_callbacks.end());
  source->buffer_callbacks.clear();
  source->controls.clear();
  source->updates.clear();
  source->commits.clear();
  source->completes.clear();
  source->discards.clear();
}

extern "C" void darwin_art_android_surface_transaction_set_on_discard(
    void* opaque, void* context, void (*callback)(void*)) {
  auto* transaction = reinterpret_cast<SurfaceTransaction*>(opaque);
  if (transaction != nullptr && callback != nullptr) {
    transaction->discards.push_back({callback, context});
  }
}

extern "C" void
darwin_art_android_surface_transaction_set_transparent_region_hint(
    void* opaque, void* opaque_control, const int32_t* rects, size_t count) {
  auto* transaction = reinterpret_cast<SurfaceTransaction*>(opaque);
  auto* control = reinterpret_cast<ASurfaceControl*>(opaque_control);
  if (transaction == nullptr || control == nullptr) return;
  Remember(transaction, control);
  auto* update = FindUpdate(transaction, control);
  if (update == nullptr) return;
  update->has_transparent_region = true;
  update->transparent_region.clear();
  if (rects == nullptr) count = 0;
  count = std::min(count, kMaxTransparentRegionRects);
  update->transparent_region.reserve(count);
  for (size_t index = 0; index < count; ++index) {
    const int32_t* rect = rects + index * 4;
    if (rect[2] <= rect[0] || rect[3] <= rect[1]) continue;
    update->transparent_region.push_back(
        {rect[0], rect[1], rect[2], rect[3]});
  }
  if (DebugSurfaceTransactions()) {
    std::fprintf(stderr, "ART Android SurfaceTransaction: transparent-region "
                         "layer=%u rects=%zu\n",
                 reinterpret_cast<SurfaceControl*>(control)->layer_id,
                 update->transparent_region.size());
    for (size_t index = 0; index < update->transparent_region.size(); ++index) {
      const ARect& rect = update->transparent_region[index];
      std::fprintf(stderr,
                   "ART Android SurfaceTransaction: transparent-region "
                   "layer=%u rect[%zu]=[%d,%d,%d,%d]\n",
                   reinterpret_cast<SurfaceControl*>(control)->layer_id,
                   index, rect.left, rect.top, rect.right, rect.bottom);
    }
  }
}

extern "C" size_t darwin_art_android_surface_control_copy_transparent_region(
    void* opaque, int32_t* rects, size_t capacity) {
  auto* control = reinterpret_cast<SurfaceControl*>(opaque);
  if (control == nullptr) return 0;
  std::lock_guard<std::mutex> lock(g_surface_controls_mutex);
  const size_t count = control->transparent_region.size();
  if (rects == nullptr || capacity == 0) return count;
  const size_t copied = std::min(count, capacity);
  for (size_t index = 0; index < copied; ++index) {
    const ARect& rect = control->transparent_region[index];
    rects[index * 4 + 0] = rect.left;
    rects[index * 4 + 1] = rect.top;
    rects[index * 4 + 2] = rect.right;
    rects[index * 4 + 3] = rect.bottom;
  }
  return count;
}

extern "C" void darwin_art_android_surface_transaction_set_buffer_callbacks(
    void* opaque, void* control, void* context,
    void (*complete)(void*, ASurfaceTransactionStats*),
    void (*discard)(void*, int)) {
  auto* transaction = reinterpret_cast<SurfaceTransaction*>(opaque);
  if (transaction != nullptr && complete != nullptr && discard != nullptr) {
    transaction->buffer_callbacks.push_back(
        {static_cast<ASurfaceControl*>(control), context, complete, discard});
  }
}

static void ApplySurfaceTransactionImpl(SurfaceTransaction* transaction) {
  if (transaction == nullptr) return;
  // Commit listeners may close or reuse the Java transaction synchronously.
  // Consume its state before invoking any listener and clean up only this batch.
  SurfaceTransaction batch = std::move(*transaction);
  *transaction = SurfaceTransaction{};
  transaction = &batch;
  // A buffer may not be latched until its producer's acquire fence signals.
  // The descriptor is backed by the producer's MTLSharedEvent and remains
  // owned by this transaction until this latch boundary.
  for (auto& update : transaction->updates) {
    if (update.acquire_fence < 0) continue;
    const int fence = update.acquire_fence;
    int wait_result;
    do { wait_result = sync_wait(fence, -1); }
    while (wait_result != 0 && errno == EINTR);
    (void)darwin_art_bionic_socket_broker_close(fence);
    update.acquire_fence = -1;
    if (wait_result != 0) {
      std::fprintf(stderr,
                   "ART Android SurfaceTransaction: acquire fence failed "
                   "control=%p fence=%d errno=%d\n",
                   static_cast<void*>(update.opaque), fence, errno);
      DiscardBufferCallbacks(transaction, nullptr, true);
      DiscardTransactionCallbacks(transaction);
      ReleaseTransactionBuffers(transaction);
      ReleaseTransactionControls(transaction);
      return;
    }
  }
  std::vector<DarwinArtSurfaceFlingerLayerUpdate> frontend_updates;
  frontend_updates.reserve(transaction->updates.size());
  std::vector<DarwinArtMetalComposerLayer> control_states;
  control_states.reserve(transaction->updates.size());
  const auto populate_transparent_region =
      [](DarwinArtMetalComposerLayer* state, const std::vector<ARect>& region,
         bool changed) {
        if (state == nullptr || !changed) return;
        state->what |= DARWIN_ART_SF_TRANSPARENT_REGION_CHANGED;
        const size_t count = std::min(region.size(), kMaxTransparentRegionRects);
        state->transparent_region_count = 0;
        for (size_t index = 0; index < count; ++index) {
          const ARect& rect = region[index];
          if (rect.right <= rect.left || rect.bottom <= rect.top) continue;
          state->transparent_region[state->transparent_region_count++] = {
              .left = rect.left,
              .top = rect.top,
              .right = rect.right,
              .bottom = rect.bottom,
          };
        }
      };
  for (const auto& update : transaction->updates) {
    const auto* control = reinterpret_cast<const SurfaceControl*>(update.opaque);
    if (control == nullptr) continue;
    uint64_t what = 0;
    if (update.has_position) what |= DARWIN_ART_SF_POSITION_CHANGED;
    if (update.has_z_order) what |= DARWIN_ART_SF_LAYER_CHANGED;
    if (update.has_alpha) what |= DARWIN_ART_SF_ALPHA_CHANGED;
    if (update.has_scale) what |= DARWIN_ART_SF_MATRIX_CHANGED;
    if (update.has_visibility) what |= DARWIN_ART_SF_FLAGS_CHANGED;
    if (update.has_parent) what |= DARWIN_ART_SF_REPARENT;
    if (update.has_relative_layer) {
      what |= DARWIN_ART_SF_RELATIVE_LAYER_CHANGED;
    }
    if (update.has_transform) what |= DARWIN_ART_SF_BUFFER_TRANSFORM_CHANGED;
    if (update.has_crop) what |= DARWIN_ART_SF_CROP_CHANGED;
    if (update.has_buffer) what |= DARWIN_ART_SF_BUFFER_CHANGED;
    if (update.has_damage) what |= DARWIN_ART_SF_DAMAGE_CHANGED;
    if (update.has_geometry) what |= DARWIN_ART_SF_DESTINATION_FRAME_CHANGED;
    if (TraceReparentToNull() && update.has_parent && update.parent == nullptr) {
      std::fprintf(stderr,
                   "ART SurfaceTransaction trace: producer-reparent-null "
                   "owner=%u local=%u what=0x%llx parent=0 target=%u\n",
                   control->owner_process_id, control->layer_id,
                   static_cast<unsigned long long>(what),
                   HostTargetSurfaceIdForTrace());
    }
    const auto* parent =
        update.has_parent
            ? reinterpret_cast<const SurfaceControl*>(update.parent)
            : control->parent;
    const uint32_t parent_owner_process_id = update.has_parent
        ? (parent == nullptr ? 0 : parent->owner_process_id)
        : SurfaceControlParentOwner(control);
    const uint32_t parent_layer_id = update.has_parent
        ? (parent == nullptr ? 0 : parent->layer_id)
        : SurfaceControlParentLayer(control);
    const auto* relative_to =
        update.has_relative_layer
            ? reinterpret_cast<const SurfaceControl*>(update.relative_to)
            : control->relative_to;
    const ARect destination =
        update.has_geometry ? update.destination : control->destination;
    frontend_updates.push_back({
        .layer_id = control->layer_id,
        .parent_id = parent_layer_id,
        .relative_parent_id =
            relative_to == nullptr ? 0 : relative_to->layer_id,
        .what = what,
        .flags = update.has_visibility
            ? (update.visible ? 0u : 1u)
            : (control->visible ? 0u : 1u),
        .mask = update.has_visibility ? 1u : 0u,
        .transform = update.has_transform
            ? static_cast<uint32_t>(update.transform)
            : static_cast<uint32_t>(control->transform),
        .x = update.has_position ? static_cast<float>(update.position_x)
                                 : static_cast<float>(control->position_x),
        .y = update.has_position ? static_cast<float>(update.position_y)
                                 : static_cast<float>(control->position_y),
        .scale_x = update.has_scale ? update.scale_x : control->scale_x,
        .scale_y = update.has_scale ? update.scale_y : control->scale_y,
        .z = (update.has_z_order || update.has_relative_layer)
            ? update.z_order
            : control->z_order,
        .alpha = update.has_alpha ? update.alpha : control->alpha,
        .destination_left = destination.left,
        .destination_top = destination.top,
        .destination_right = destination.right,
        .destination_bottom = destination.bottom,
        .crop_left = update.has_crop ? update.crop.left : control->crop.left,
        .crop_top = update.has_crop ? update.crop.top : control->crop.top,
        .crop_right = update.has_crop ? update.crop.right : control->crop.right,
        .crop_bottom = update.has_crop
            ? update.crop.bottom : control->crop.bottom,
    });
    const bool has_transparent_region =
        update.has_transparent_region || !control->transparent_region.empty();
    const uint64_t structural_what =
        what & ~static_cast<uint64_t>(DARWIN_ART_SF_BUFFER_CHANGED);
    if (structural_what != 0 || has_transparent_region) {
      control_states.push_back({
          .owner_process_id = control->owner_process_id,
          .layer_id = control->layer_id,
          .parent_owner_process_id = parent_owner_process_id,
          .parent_id = parent_layer_id,
          .relative_parent_owner_process_id =
              relative_to == nullptr ? 0 : relative_to->owner_process_id,
          .relative_parent_id =
              relative_to == nullptr ? 0 : relative_to->layer_id,
          .what = structural_what,
          .flags = update.has_visibility
              ? (update.visible ? 0u : 1u)
              : (control->visible ? 0u : 1u),
          .mask = update.has_visibility ? 1u : 0u,
          .transform = update.has_transform
              ? static_cast<uint32_t>(update.transform)
              : static_cast<uint32_t>(control->transform),
          .iosurface = nullptr,
          .destination_left = update.has_position
              ? update.position_x : destination.left,
          .destination_top = update.has_position
              ? update.position_y : destination.top,
          .destination_right = destination.right,
          .destination_bottom = destination.bottom,
          .position_x = update.has_position
              ? update.position_x : control->position_x,
          .position_y = update.has_position
              ? update.position_y : control->position_y,
          .scale_x = update.has_scale ? update.scale_x : control->scale_x,
          .scale_y = update.has_scale ? update.scale_y : control->scale_y,
          .has_crop = update.has_crop || control->has_crop,
          .crop_left = update.has_crop ? update.crop.left : control->crop.left,
          .crop_top = update.has_crop ? update.crop.top : control->crop.top,
          .crop_right = update.has_crop ? update.crop.right : control->crop.right,
          .crop_bottom = update.has_crop
              ? update.crop.bottom : control->crop.bottom,
          .z = (update.has_z_order || update.has_relative_layer)
              ? update.z_order
              : control->z_order,
          .alpha = update.has_alpha ? update.alpha : control->alpha,
      });
      // `control` is deliberately read only here: the update is applied to it
      // below. Prefer the transaction's new value, including an empty vector
      // which explicitly clears a previously published hint.
      const auto& region = update.has_transparent_region
                               ? update.transparent_region
                               : control->transparent_region;
      populate_transparent_region(&control_states.back(), region,
                                  has_transparent_region);
    }
    if (DebugSurfaceTransactions()) {
      std::fprintf(
          stderr,
          "ART Android SurfaceTransaction: update pid=%d control=%p "
          "layer=%u name=%s what=0x%llx parent=%u visible=%d "
          "flags[pos=%d,z=%d,alpha=%d,scale=%d,visibility=%d,parent=%d,"
          "transform=%d(value=%d),relative=%d(to=%u),crop=%d,buffer=%d,"
          "damage=%d,geometry=%d]\n",
          getpid(), static_cast<void*>(update.opaque), control->layer_id,
          control->name.c_str(), static_cast<unsigned long long>(what),
          parent_layer_id,
          update.has_visibility ? (update.visible ? 1 : 0)
                                : (control->visible ? 1 : 0),
          update.has_position ? 1 : 0, update.has_z_order ? 1 : 0,
          update.has_alpha ? 1 : 0, update.has_scale ? 1 : 0,
          update.has_visibility ? 1 : 0, update.has_parent ? 1 : 0,
          update.has_transform ? 1 : 0,
          update.has_transform ? update.transform : control->transform,
          update.has_relative_layer ? 1 : 0,
          relative_to == nullptr ? 0 : relative_to->layer_id,
          update.has_crop ? 1 : 0,
          update.has_buffer ? 1 : 0, update.has_damage ? 1 : 0,
          update.has_geometry ? 1 : 0);
    }
  }
  const uint64_t transaction_id =
      g_next_surface_transaction_id.fetch_add(1, std::memory_order_relaxed);
  DarwinArtSurfaceFlingerCommitResult frontend_result{};
  const char* surfaceflinger_socket =
      std::getenv("DARWIN_ART_SURFACEFLINGER_SOCKET");
  const bool central_surfaceflinger =
      surfaceflinger_socket != nullptr && surfaceflinger_socket[0] != '\0';
  const char* app_package = std::getenv("DARWIN_ART_APK_APP_PACKAGE");
  const bool application_runtime = app_package != nullptr && app_package[0] != '\0';
  if (application_runtime && !central_surfaceflinger) {
    std::fprintf(stderr,
                 "ART Android SurfaceTransaction: application %s requires "
                 "the central SurfaceFlinger service\n",
                 app_package);
    DiscardTransactionCallbacks(transaction);
    ReleaseTransactionBuffers(transaction);
    ReleaseTransactionControls(transaction);
    return;
  }
  if (!central_surfaceflinger &&
      !darwin_art_surfaceflinger_commit_transaction(
          transaction_id, frontend_updates.data(), frontend_updates.size(),
          &frontend_result)) {
    std::fprintf(stderr,
                 "ART Android SurfaceTransaction: AOSP frontend rejected "
                 "transaction=%llu layers=%zu\n",
                 static_cast<unsigned long long>(transaction_id),
                 frontend_updates.size());
    DiscardTransactionCallbacks(transaction);
    ReleaseTransactionBuffers(transaction);
    ReleaseTransactionControls(transaction);
    return;
  }
  SurfaceTransactionStats stats;
  stats.controls = transaction->controls;
  if (DebugSurfaceTransactions()) {
    std::fprintf(stderr,
                 "ART Android SurfaceTransaction: apply pid=%d controls=%zu\n",
                 getpid(), transaction->controls.size());
  }
  std::vector<SurfacePresentation> presentations;
  std::vector<SurfaceControl*> replaced_relationships;
  {
    std::lock_guard<std::mutex> controls_lock(g_surface_controls_mutex);
    for (auto& update : transaction->updates) {
      auto* control = reinterpret_cast<SurfaceControl*>(update.opaque);
      if (control == nullptr) continue;
      if (update.has_buffer) {
        if (control->buffer != nullptr && control->buffer != update.buffer) {
          AHardwareBuffer_acquire(control->buffer);
          stats.previous_buffers.emplace(update.opaque, control->buffer);
        }
        if (control->buffer != nullptr)
          AHardwareBuffer_release(control->buffer);
        control->buffer = update.buffer;
        update.buffer = nullptr;
      }
      if (update.has_geometry) {
        control->source = update.source;
        control->destination = update.destination;
        control->has_geometry = true;
      }
      if (update.has_crop) {
        control->crop = update.crop;
        control->has_crop = true;
      }
      if (update.has_visibility) control->visible = update.visible;
      if (update.has_position) {
        control->position_x = update.position_x;
        control->position_y = update.position_y;
      }
      if (update.has_transform) control->transform = update.transform;
      if (update.has_z_order) {
        control->z_order = update.z_order;
        if (control->relative_to != nullptr) {
          replaced_relationships.push_back(control->relative_to);
          control->relative_to = nullptr;
        }
      }
      if (update.has_relative_layer) {
        control->z_order = update.z_order;
        auto* relative_to =
            reinterpret_cast<SurfaceControl*>(update.relative_to);
        if (control->relative_to != relative_to) {
          if (relative_to != nullptr) {
            ASurfaceControl_acquire(
                reinterpret_cast<ASurfaceControl*>(relative_to));
          }
          if (control->relative_to != nullptr) {
            replaced_relationships.push_back(control->relative_to);
          }
          control->relative_to = relative_to;
        }
      }
      if (update.has_scale) {
        control->scale_x = update.scale_x;
        control->scale_y = update.scale_y;
      }
      if (update.has_alpha) control->alpha = update.alpha;
      if (update.has_transparent_region) {
        control->transparent_region = std::move(update.transparent_region);
      }
      if (update.has_parent) {
        auto* parent = reinterpret_cast<SurfaceControl*>(update.parent);
        if (control->parent != parent) {
          if (parent != nullptr) {
            ASurfaceControl_acquire(reinterpret_cast<ASurfaceControl*>(parent));
          }
          if (control->parent != nullptr) {
            replaced_relationships.push_back(control->parent);
          }
          control->parent = parent;
        }
        control->imported_parent_owner_process_id = 0;
        control->imported_parent_layer_id = 0;
        // createFromWindow/fromJava controls start attached to the display,
        // but an explicit reparent replaces that initial attachment. In
        // particular, reparent(control, nullptr) means detach; retaining the
        // root bit would keep Chromium's old tab surface in composition.
        control->composition_root = false;
      }
    }

    // SurfaceControl creation carries structural state even when the layer
    // never owns a buffer. Publish a complete snapshot for every locally
    // owned structural control that this transaction did not otherwise
    // mention. This mirrors SurfaceFlinger's handle graph and lets a renderer
    // process attach a buffer to an imported BLAST parent without promoting
    // that parent to an unrelated display root.
    for (SurfaceControl* control : g_surface_controls) {
      if (control == nullptr || control->buffer != nullptr ||
          control->owner_process_id != static_cast<uint32_t>(getpid())) {
        continue;
      }
      const bool already_present = std::any_of(
          control_states.begin(), control_states.end(),
          [control](const DarwinArtMetalComposerLayer& state) {
            return state.owner_process_id == control->owner_process_id &&
                state.layer_id == control->layer_id;
          });
      if (already_present) continue;
      const uint64_t ordering_change =
          control->relative_to == nullptr
              ? DARWIN_ART_SF_LAYER_CHANGED
              : DARWIN_ART_SF_RELATIVE_LAYER_CHANGED;
      control_states.push_back({
          .owner_process_id = control->owner_process_id,
          .layer_id = control->layer_id,
          .parent_owner_process_id = SurfaceControlParentOwner(control),
          .parent_id = SurfaceControlParentLayer(control),
          .relative_parent_owner_process_id =
              control->relative_to == nullptr
                  ? 0
                  : control->relative_to->owner_process_id,
          .relative_parent_id =
              control->relative_to == nullptr ? 0
                                               : control->relative_to->layer_id,
          .what = DARWIN_ART_SF_POSITION_CHANGED | ordering_change |
              DARWIN_ART_SF_ALPHA_CHANGED | DARWIN_ART_SF_FLAGS_CHANGED |
              DARWIN_ART_SF_MATRIX_CHANGED |
              (control->has_crop ? DARWIN_ART_SF_CROP_CHANGED : 0) |
              (SurfaceControlParentLayer(control) == 0
                   ? 0
                   : DARWIN_ART_SF_REPARENT) |
              DARWIN_ART_SF_BUFFER_TRANSFORM_CHANGED,
          .flags = control->visible ? 0u : 1u,
          .mask = 1u,
          .transform = static_cast<uint32_t>(control->transform),
          .iosurface = nullptr,
          .destination_left = control->position_x,
          .destination_top = control->position_y,
          .destination_right = control->position_x,
          .destination_bottom = control->position_y,
          .position_x = control->position_x,
          .position_y = control->position_y,
          .scale_x = control->scale_x,
          .scale_y = control->scale_y,
          .has_crop = control->has_crop,
          .crop_left = control->crop.left,
          .crop_top = control->crop.top,
          .crop_right = control->crop.right,
          .crop_bottom = control->crop.bottom,
          .z = control->z_order,
          .alpha = control->alpha,
      });
      populate_transparent_region(&control_states.back(),
                                  control->transparent_region,
                                  !control->transparent_region.empty());
    }

    // SurfaceFlinger composes the current layer tree, not just the controls
    // carrying new buffers in this transaction. Re-blending only an updated
    // translucent layer over the previous display accumulates its color and
    // leaves pixels from hidden/moved layers behind. Keep each AHardwareBuffer
    // as the retained layer backing, then rebuild the host target entirely on
    // the GPU for every committed transaction.
    for (SurfaceControl* control : g_surface_controls) {
      if (control->visible && control->buffer != nullptr &&
          (central_surfaceflinger ||
           IsAttachedToCompositionRoot(control, g_surface_controls))) {
        SurfacePresentation presentation = MakePresentation(control);
        const auto update = std::find_if(
            transaction->updates.begin(), transaction->updates.end(),
            [control](const SurfaceTransaction::Update& candidate) {
              return reinterpret_cast<SurfaceControl*>(candidate.opaque) ==
                  control;
            });
        presentation.reparented =
            update != transaction->updates.end() && update->has_parent;
        presentations.push_back(std::move(presentation));
      }
    }
  }
  for (SurfaceControl* relationship : replaced_relationships) {
    ASurfaceControl_release(reinterpret_cast<ASurfaceControl*>(relationship));
  }
  // Preserve transaction/control enumeration here. The AOSP
  // SurfaceFlinger layer hierarchy is the single authority for composition
  // order; the Darwin HWC consumes the order exported by that hierarchy.
  bool composition_started =
      !presentations.empty() &&
      darwin_art_android_begin_hardware_buffer_composition(
          presentations.front().buffer, true, transaction_id);
  bool actual_target_composition_marked = false;
  if (composition_started) {
    for (const auto& state : control_states) {
      std::array<int32_t, kMaxTransparentRegionRects * 4>
          transparent_region_rects{};
      for (uint32_t index = 0; index < state.transparent_region_count;
           ++index) {
        const auto& rect = state.transparent_region[index];
        transparent_region_rects[index * 4 + 0] = rect.left;
        transparent_region_rects[index * 4 + 1] = rect.top;
        transparent_region_rects[index * 4 + 2] = rect.right;
        transparent_region_rects[index * 4 + 3] = rect.bottom;
      }
      darwin_art_android_present_surface_control_state(
          state.owner_process_id, state.layer_id,
          state.parent_owner_process_id, state.parent_id,
          state.relative_parent_owner_process_id, state.relative_parent_id,
          state.what, state.flags, state.mask, state.transform,
          state.destination_left, state.destination_top, state.destination_right,
          state.destination_bottom, state.position_x, state.position_y,
          state.scale_x, state.scale_y, state.has_crop, state.crop_left,
          state.crop_top, state.crop_right, state.crop_bottom, state.z,
          state.alpha,
          state.transparent_region_count == 0
              ? nullptr
              : transparent_region_rects.data(),
          state.transparent_region_count);
    }
    for (const auto& presentation : presentations) {
      if (DebugSurfaceTransactions()) {
        std::fprintf(stderr,
                     "ART Android SurfaceTransaction: present pid=%d name=%s "
                     "source=[%d,%d,%d,%d] destination=[%d,%d,%d,%d] "
                     "alpha=%.3f z=%d transform=%d\n",
                     getpid(), presentation.name.c_str(),
                     presentation.source.left, presentation.source.top,
                     presentation.source.right, presentation.source.bottom,
                     presentation.destination.left, presentation.destination.top,
                     presentation.destination.right,
                     presentation.destination.bottom, presentation.alpha,
                     presentation.z_order, presentation.transform);
      }
      darwin_art_android_present_hardware_buffer(
          presentation.control, presentation.control->owner_process_id,
          presentation.control->layer_id,
          SurfaceControlParentOwner(presentation.control),
          SurfaceControlParentLayer(presentation.control),
          DARWIN_ART_SF_BUFFER_CHANGED |
              (presentation.reparented ? DARWIN_ART_SF_REPARENT : 0),
          presentation.control->relative_to == nullptr
              ? 0
              : presentation.control->relative_to->owner_process_id,
          presentation.control->relative_to == nullptr
              ? 0
              : presentation.control->relative_to->layer_id,
          presentation.z_order, presentation.buffer,
          static_cast<uint32_t>(presentation.transform),
          presentation.source.left,
          presentation.source.top, presentation.source.right,
          presentation.source.bottom, presentation.destination.left,
          presentation.destination.top, presentation.destination.right,
          presentation.destination.bottom, presentation.has_damage,
          presentation.damage.left, presentation.damage.top,
          presentation.damage.right, presentation.damage.bottom,
          presentation.alpha);
    }
    stats.present_fence =
        darwin_art_android_end_hardware_buffer_composition();
  }
  if (!composition_started && central_surfaceflinger &&
      presentations.empty() && !control_states.empty()) {
    stats.present_fence = darwin_art_surfaceflinger_service_commit(
        transaction_id, control_states.data(), control_states.size());
    composition_started = stats.present_fence >= 0;
  }
  if (!composition_started && central_surfaceflinger &&
      (!control_states.empty() || !presentations.empty())) {
    const char* encoded_target = std::getenv("DARWIN_ART_HOST_IOSURFACE_ID");
    char* end = nullptr;
    const unsigned long parsed =
        encoded_target == nullptr ? 0 : std::strtoul(encoded_target, &end, 10);
    IOSurfaceRef target =
        encoded_target != nullptr && end != encoded_target && *end == '\0' &&
                parsed > 0 && parsed <= UINT32_MAX
            ? IOSurfaceLookup(static_cast<uint32_t>(parsed))
            : nullptr;
    const bool has_explicit_target = target != nullptr;
    const uint32_t target_width = has_explicit_target
        ? static_cast<uint32_t>(IOSurfaceGetWidth(target))
        : 0;
    const uint32_t target_height = has_explicit_target
        ? static_cast<uint32_t>(IOSurfaceGetHeight(target))
        : 0;
    {
      const jint configured_width = darwin_art::DarwinAngleHostSurfaceWidth();
      const jint configured_height = darwin_art::DarwinAngleHostSurfaceHeight();
      const uint32_t logical_target_width =
          configured_width > 0 ? static_cast<uint32_t>(configured_width)
                               : target_width;
      const uint32_t logical_target_height =
          configured_height > 0 ? static_cast<uint32_t>(configured_height)
                                : target_height;

      // The normal path appends each retained AHardwareBuffer to the Metal
      // composer after publishing the structural control states.  Keep the
      // same layer payload when the client-side ANGLE context is unavailable:
      // the central service can import the IOSurface directly, and does not
      // need a producer EGL context to latch an already fence-waited buffer.
      // AHardwareBuffer references in `presentations` remain held until the
      // synchronous service call has serialized every IOSurface id below.
      std::vector<DarwinArtMetalComposerLayer> fallback_layers = control_states;
      fallback_layers.reserve(control_states.size() + presentations.size());
      for (const auto& presentation : presentations) {
        if (presentation.control == nullptr || presentation.buffer == nullptr)
          continue;
        AHardwareBuffer_Desc description{};
        AHardwareBuffer_describe(presentation.buffer, &description);
        void* iosurface =
            darwin_art_android_hardware_buffer_iosurface(presentation.buffer);
        if (iosurface == nullptr || description.width == 0 ||
            description.height == 0) {
          continue;
        }
        const int32_t buffer_width =
            static_cast<int32_t>(description.width);
        const int32_t buffer_height =
            static_cast<int32_t>(description.height);
        const int32_t source_left =
            std::clamp(presentation.source.left, 0, buffer_width);
        const int32_t source_top =
            std::clamp(presentation.source.top, 0, buffer_height);
        const int32_t source_right =
            std::clamp(presentation.source.right, 0, buffer_width);
        const int32_t source_bottom =
            std::clamp(presentation.source.bottom, 0, buffer_height);
        const int32_t destination_left = has_explicit_target
            ? std::clamp(presentation.destination.left, 0,
                         static_cast<int32_t>(logical_target_width))
            : presentation.destination.left;
        const int32_t destination_top = has_explicit_target
            ? std::clamp(presentation.destination.top, 0,
                         static_cast<int32_t>(logical_target_height))
            : presentation.destination.top;
        const int32_t destination_right = has_explicit_target
            ? std::clamp(presentation.destination.right, 0,
                         static_cast<int32_t>(logical_target_width))
            : presentation.destination.right;
        const int32_t destination_bottom = has_explicit_target
            ? std::clamp(presentation.destination.bottom, 0,
                         static_cast<int32_t>(logical_target_height))
            : presentation.destination.bottom;
        const auto* control = presentation.control;
        fallback_layers.push_back({
            .owner_process_id = control->owner_process_id,
            .layer_id = control->layer_id,
            .parent_owner_process_id = SurfaceControlParentOwner(control),
            .parent_id = SurfaceControlParentLayer(control),
            .relative_parent_owner_process_id =
                control->relative_to == nullptr
                    ? 0
                    : control->relative_to->owner_process_id,
            .relative_parent_id =
                control->relative_to == nullptr ? 0 : control->relative_to->layer_id,
            .what = static_cast<uint64_t>(
                DARWIN_ART_SF_BUFFER_CHANGED |
                (presentation.reparented ? DARWIN_ART_SF_REPARENT : 0)),
            .flags = 0,
            .mask = 0,
            .transform = static_cast<uint32_t>(presentation.transform),
            .producer_bottom_left =
                (description.usage & AHARDWAREBUFFER_USAGE_COMPOSER_OVERLAY) == 0,
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
            .z = presentation.z_order,
            .alpha = presentation.alpha,
        });
      }
      stats.present_fence = has_explicit_target
          ? darwin_art_surfaceflinger_service_present(
                static_cast<uint32_t>(parsed), logical_target_width,
                logical_target_height, transaction_id, fallback_layers.data(),
                fallback_layers.size(), nullptr, 0)
          : darwin_art_surfaceflinger_service_submit(
                transaction_id, fallback_layers.data(),
                fallback_layers.size());
      composition_started = stats.present_fence >= 0;
      // This path does not enter ANGLE, so its thread-local EGL target may be
      // empty or stale. Mark the target actually submitted to the central
      // service and register a private copy of its completion fence with the
      // host scanout monitor. The Android-facing stats fence remains owned by
      // the transaction callbacks; the duplicate is consumed by the monitor.
      if (has_explicit_target) {
        darwin_art_surface_gpu_set_iosurface_composition_active(
            target, composition_started);
        actual_target_composition_marked = true;
      }
      if (has_explicit_target && composition_started) {
        DarwinArtSurface* host = darwin_art_surface_active_gpu();
        const int monitor_fence =
            host == nullptr
                ? -1
                : darwin_art_bionic_socket_broker_dup(stats.present_fence);
        if (host != nullptr && monitor_fence >= 0) {
          if (!darwin_art_surface_gpu_track_composition_fence(
                  host, monitor_fence) &&
              DebugSurfaceTransactions()) {
            std::fprintf(stderr,
                         "ART Android SurfaceTransaction: fallback readiness "
                         "monitor rejected fence=%d\n",
                         monitor_fence);
          }
        }
      }
      if (target != nullptr) CFRelease(target);
    }
  }
  // The host and Chromium renderer import the same IOSurface in different
  // processes. Publish SurfaceFlinger's retained-layer visibility with that
  // shared object so parent HWUI never samples a detached child layer.
  if (!actual_target_composition_marked) {
    darwin_art_android_set_hardware_buffer_composition_active(
        composition_started);
  }
  for (const auto& presentation : presentations) {
    AHardwareBuffer_release(presentation.buffer);
  }
  auto* stats_opaque = reinterpret_cast<ASurfaceTransactionStats*>(&stats);
  if (DebugSurfaceTransactions()) {
    std::fprintf(stderr,
                 "ART Android SurfaceTransaction: callbacks pid=%d commits=%zu "
                 "completes=%zu fence=%d\n",
                 getpid(), transaction->commits.size(),
                 transaction->completes.size(),
                 stats.present_fence);
  }
  auto commits = std::move(transaction->commits);
  auto completes = std::move(transaction->completes);
  auto buffer_callbacks = std::move(transaction->buffer_callbacks);
  transaction->buffer_callbacks.clear();
  transaction->commits.clear();
  transaction->completes.clear();
  transaction->discards.clear();
  for (const auto& callback : commits) {
    if (callback.function != nullptr)
      callback.function(callback.context, stats_opaque);
  }
  for (const auto& callback : completes) {
    if (callback.function != nullptr)
      callback.function(callback.context, stats_opaque);
  }
  for (const auto& callback : buffer_callbacks) {
    callback.complete(callback.context, stats_opaque);
  }
  ReleaseTransactionBuffers(transaction);
  ReleaseTransactionControls(transaction);
}

static bool StartLatchWorker() {
  auto& state = LatchQueue();
  {
    std::lock_guard<std::mutex> lock(state.mutex);
    if (state.started) return true;
    state.started = true;
  }
  try {
    std::thread([&state] {
      for (;;) {
        DeferredLatchTransaction item;
        {
          std::unique_lock<std::mutex> lock(state.mutex);
          state.condition.wait(lock,
                               [&] { return !state.pending.empty(); });
          item = std::move(state.pending.front());
          state.pending.pop_front();
        }
        ApplySurfaceTransactionImpl(item.transaction.get());
        g_pending_latch_workers.fetch_sub(1, std::memory_order_acq_rel);
      }
    }).detach();
    return true;
  } catch (...) {
    std::lock_guard<std::mutex> lock(state.mutex);
    state.started = false;
    return false;
  }
}

extern "C" void ASurfaceTransaction_apply(ASurfaceTransaction* opaque) {
  auto* transaction = reinterpret_cast<SurfaceTransaction*>(opaque);
  if (transaction == nullptr) return;

  // SurfaceFlinger latches a buffer only after its acquire fence signals.  Do
  // not park the ART/UI or renderer thread behind an unsignaled producer
  // fence: transfer this transaction to a bounded one-shot latch worker and
  // return immediately.  The public transaction is cleared so the caller may
  // delete it according to the Android API contract while the retained
  // buffers/controls remain owned by the deferred copy.
  // Once a transaction is queued behind an unsignaled acquire fence, later
  // transactions join the same FIFO so a fast producer cannot overtake the
  // pending latch and reorder SurfaceFlinger state.
  bool defer = g_pending_latch_workers.load(std::memory_order_acquire) != 0;
  for (const auto& update : transaction->updates) {
    if (update.acquire_fence < 0) continue;
    if (sync_wait(update.acquire_fence, 0) != 0 && errno == ETIMEDOUT) {
      defer = true;
      break;
    }
  }
  if (!defer) {
    ApplySurfaceTransactionImpl(transaction);
    return;
  }

  uint32_t pending = g_pending_latch_workers.load(std::memory_order_relaxed);
  while (pending < kMaximumPendingLatchWorkers &&
         !g_pending_latch_workers.compare_exchange_weak(
             pending, pending + 1, std::memory_order_acq_rel,
             std::memory_order_relaxed)) {
  }
  if (pending >= kMaximumPendingLatchWorkers) {
    // Keep the latch queue bounded. This is an exceptional producer burst;
    // applying synchronously preserves Android transaction ordering while
    // preventing unbounded detached waiters.
    ApplySurfaceTransactionImpl(transaction);
    return;
  }

  if (DebugSurfaceTransactions()) {
    std::fprintf(stderr,
                 "ART Android SurfaceTransaction: defer acquire-fence "
                 "updates=%zu pending=%u\n",
                 transaction->updates.size(), pending + 1);
  }

  auto deferred = std::make_unique<SurfaceTransaction>(std::move(*transaction));
  transaction->buffer_callbacks.clear();
  transaction->controls.clear();
  transaction->updates.clear();
  transaction->commits.clear();
  transaction->completes.clear();
  transaction->discards.clear();
  if (!StartLatchWorker()) {
    // A thread creation failure is exceptional; preserve correctness even if
    // latency temporarily regresses by applying on the caller as a fallback.
    ApplySurfaceTransactionImpl(deferred.get());
    g_pending_latch_workers.fetch_sub(1, std::memory_order_acq_rel);
    return;
  }
  auto& state = LatchQueue();
  {
    std::lock_guard<std::mutex> lock(state.mutex);
    state.pending.push_back(
        DeferredLatchTransaction{std::move(deferred), {}});
  }
  state.condition.notify_one();
}

extern "C" void ASurfaceTransaction_setOnCommit(ASurfaceTransaction* opaque,
                                                  void* context,
                                                  TransactionCallback callback) {
  auto* transaction = reinterpret_cast<SurfaceTransaction*>(opaque);
  if (transaction != nullptr) {
    if (callback != nullptr) transaction->commits.push_back({callback, context});
    if (DebugSurfaceTransactions()) {
      std::fprintf(stderr,
                   "ART Android SurfaceTransaction: setOnCommit pid=%d "
                   "callback=%p context=%p\n",
                   getpid(), reinterpret_cast<void*>(callback), context);
    }
  }
}

extern "C" void ASurfaceTransaction_setOnComplete(ASurfaceTransaction* opaque,
                                                    void* context,
                                                    TransactionCallback callback) {
  auto* transaction = reinterpret_cast<SurfaceTransaction*>(opaque);
  if (transaction != nullptr) {
    if (callback != nullptr) transaction->completes.push_back({callback, context});
    if (DebugSurfaceTransactions()) {
      std::fprintf(stderr,
                   "ART Android SurfaceTransaction: setOnComplete pid=%d "
                   "callback=%p context=%p\n",
                   getpid(), reinterpret_cast<void*>(callback), context);
    }
  }
}

extern "C" void ASurfaceTransactionStats_getASurfaceControls(
    ASurfaceTransactionStats* opaque, ASurfaceControl*** out, size_t* count) {
  if (out == nullptr || count == nullptr) return;
  auto* stats = reinterpret_cast<SurfaceTransactionStats*>(opaque);
  *count = stats == nullptr ? 0 : stats->controls.size();
  if (*count == 0) { *out = nullptr; return; }
  *out = static_cast<ASurfaceControl**>(std::malloc(*count * sizeof(**out)));
  if (*out == nullptr) { *count = 0; return; }
  std::memcpy(*out, stats->controls.data(), *count * sizeof(**out));
}

extern "C" void ASurfaceTransactionStats_releaseASurfaceControls(
    ASurfaceControl** controls) { std::free(controls); }
extern "C" int ASurfaceTransactionStats_getPreviousReleaseFenceFd(
    ASurfaceTransactionStats* opaque, ASurfaceControl* control) {
  auto* stats = reinterpret_cast<SurfaceTransactionStats*>(opaque);
  if (stats == nullptr || control == nullptr ||
      std::find(stats->controls.begin(), stats->controls.end(), control) ==
          stats->controls.end()) {
    return -1;
  }
  auto previous = stats->previous_buffers.find(control);
  if (previous == stats->previous_buffers.end()) return -1;
  // SurfaceFlinger returns the completion fence for the composition that last
  // sampled this displaced slot. Keep the IOSurface canonical until that exact
  // Metal queue boundary is reached and the producer reacquires the slot.
  darwin_art_android_mark_hardware_buffer_released(previous->second);
  const int fence = stats->present_fence < 0
                        ? -1
                        : darwin_art_bionic_socket_broker_dup(
                              stats->present_fence);
  if (DebugSurfaceTransactions()) {
    std::fprintf(stderr,
                 "ART Android SurfaceTransaction: previous-release-fence "
                 "control=%p fence=%d\n",
                 control, fence);
  }
  return fence;
}
extern "C" int ASurfaceTransactionStats_getPresentFenceFd(
    ASurfaceTransactionStats* opaque) {
  auto* stats = reinterpret_cast<SurfaceTransactionStats*>(opaque);
  const int fence = stats == nullptr || stats->present_fence < 0
                        ? -1
                        : darwin_art_bionic_socket_broker_dup(
                              stats->present_fence);
  if (DebugSurfaceTransactions()) {
    std::fprintf(stderr,
                 "ART Android SurfaceTransaction: present-fence fence=%d\n",
                 fence);
  }
  return fence;
}
extern "C" int64_t ASurfaceTransactionStats_getLatchTime(
    ASurfaceTransactionStats*) { return 0; }
extern "C" int64_t ASurfaceTransactionStats_getAcquireTime(
    ASurfaceTransactionStats*, ASurfaceControl*) { return -1; }

#define SURFACE_CONTROL_SETTER(name, signature, control_arg) \
  extern "C" void name signature {                           \
    Remember(reinterpret_cast<SurfaceTransaction*>(transaction), control_arg); \
  }

extern "C" void ASurfaceTransaction_reparent(
    ASurfaceTransaction* opaque, ASurfaceControl* control,
    ASurfaceControl* parent) {
  auto* transaction = reinterpret_cast<SurfaceTransaction*>(opaque);
  Remember(transaction, control);
  Remember(transaction, parent);
  auto* update = FindUpdate(transaction, control);
  if (update != nullptr) {
    update->has_parent = true;
    update->parent = parent;
  }
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
  Remember(transaction, control);
  auto* update = FindUpdate(transaction, control);
  if (update != nullptr) {
    update->has_visibility = true;
    update->visible = visibility == ASURFACE_TRANSACTION_VISIBILITY_SHOW;
  }
}
extern "C" void ASurfaceTransaction_setZOrder(ASurfaceTransaction* opaque,
                                                ASurfaceControl* control,
                                                int32_t z_order) {
  auto* transaction = reinterpret_cast<SurfaceTransaction*>(opaque);
  Remember(transaction, control);
  auto* update = FindUpdate(transaction, control);
  if (update != nullptr) {
    update->has_z_order = true;
    update->has_relative_layer = false;
    update->relative_to = nullptr;
    update->z_order = z_order;
  }
}
extern "C" void darwin_art_android_surface_transaction_set_relative_layer(
    void* opaque, void* opaque_control, void* opaque_relative_to, int32_t z) {
  auto* transaction = reinterpret_cast<SurfaceTransaction*>(opaque);
  auto* control = reinterpret_cast<ASurfaceControl*>(opaque_control);
  auto* relative_to = reinterpret_cast<ASurfaceControl*>(opaque_relative_to);
  Remember(transaction, control);
  Remember(transaction, relative_to);
  auto* update = FindUpdate(transaction, control);
  if (update == nullptr) return;
  update->has_z_order = false;
  update->has_relative_layer = true;
  update->relative_to = relative_to;
  update->z_order = z;
  if (DebugSurfaceTransactions()) {
    std::fprintf(stderr,
                 "ART Android SurfaceTransaction: relative-layer pid=%d "
                 "control=%p relative=%p z=%d\n",
                 getpid(), opaque_control, opaque_relative_to, z);
  }
}
extern "C" void ASurfaceTransaction_setBuffer(
    ASurfaceTransaction* transaction, ASurfaceControl* control,
    AHardwareBuffer* buffer, int fence_fd) {
  auto* state = reinterpret_cast<SurfaceTransaction*>(transaction);
  Remember(state, control);
  auto* update = FindUpdate(state, control);
  if (update != nullptr) {
    DiscardBufferCallbacks(state, control);
    if (update->buffer != nullptr) AHardwareBuffer_release(update->buffer);
    if (update->acquire_fence >= 0)
      (void)darwin_art_bionic_socket_broker_close(update->acquire_fence);
    update->buffer = buffer;
    update->acquire_fence = fence_fd;
    update->has_buffer = true;
    if (buffer != nullptr) AHardwareBuffer_acquire(buffer);
  }
  if (update == nullptr && fence_fd >= 0)
    (void)darwin_art_bionic_socket_broker_close(fence_fd);
  if (!DebugSurfaceTransactions()) return;
  const uint32_t surface_id =
      buffer == nullptr || buffer->surface == nullptr
          ? 0
          : IOSurfaceGetID(buffer->surface);
  const uint32_t width = buffer == nullptr ? 0 : buffer->description.width;
  const uint32_t height = buffer == nullptr ? 0 : buffer->description.height;
  std::fprintf(stderr,
               "ART Android SurfaceTransaction: setBuffer pid=%d control=%p "
               "buffer=%p iosurface=%u size=%ux%u fence=%d\n",
               getpid(), static_cast<void*>(control), static_cast<void*>(buffer),
               surface_id, width, height, fence_fd);
}
extern "C" void ASurfaceTransaction_setGeometry(
    ASurfaceTransaction* opaque, ASurfaceControl* control, const ARect& source,
    const ARect& destination, int32_t transform) {
  auto* transaction = reinterpret_cast<SurfaceTransaction*>(opaque);
  Remember(transaction, control);
  auto* update = FindUpdate(transaction, control);
  if (update != nullptr) {
    update->source = source;
    update->destination = destination;
    update->has_geometry = true;
    update->transform = transform;
    update->has_transform = true;
  }
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
  Remember(transaction, control);
  auto* update = FindUpdate(transaction, control);
  if (update != nullptr) {
    update->crop = crop;
    update->has_crop = true;
  }
}
extern "C" void ASurfaceTransaction_setPosition(ASurfaceTransaction* opaque,
                                                   ASurfaceControl* control,
                                                   int32_t x, int32_t y) {
  auto* transaction = reinterpret_cast<SurfaceTransaction*>(opaque);
  Remember(transaction, control);
  auto* update = FindUpdate(transaction, control);
  if (update != nullptr) {
    update->has_position = true;
    update->position_x = x;
    update->position_y = y;
  }
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
  Remember(transaction, control);
  auto* update = FindUpdate(transaction, control);
  if (update != nullptr) {
    update->has_transform = true;
    update->transform = transform;
  }
}
extern "C" void ASurfaceTransaction_setScale(ASurfaceTransaction* opaque,
                                                ASurfaceControl* control,
                                                float x, float y) {
  auto* transaction = reinterpret_cast<SurfaceTransaction*>(opaque);
  Remember(transaction, control);
  auto* update = FindUpdate(transaction, control);
  if (update != nullptr) {
    update->has_scale = true;
    update->scale_x = x;
    update->scale_y = y;
  }
}
SURFACE_CONTROL_SETTER(ASurfaceTransaction_setBufferTransparency,
  (ASurfaceTransaction* transaction, ASurfaceControl* control, enum ASurfaceTransactionTransparency), control)
extern "C" void ASurfaceTransaction_setDamageRegion(
    ASurfaceTransaction* opaque, ASurfaceControl* control,
    const ARect* rects, uint32_t count) {
  auto* transaction = reinterpret_cast<SurfaceTransaction*>(opaque);
  Remember(transaction, control);
  auto* update = FindUpdate(transaction, control);
  if (update == nullptr) return;
  update->has_damage = true;
  update->damage.clear();
  if (rects != nullptr && count != 0)
    update->damage.assign(rects, rects + count);
  if (!DebugSurfaceTransactions()) return;
  std::fprintf(stderr,
               "ART Android SurfaceTransaction: damage pid=%d control=%p "
               "rects=%u",
               getpid(), static_cast<void*>(control), count);
  for (const ARect& rect : update->damage) {
    std::fprintf(stderr, " [%d,%d,%d,%d]", rect.left, rect.top, rect.right,
                 rect.bottom);
  }
  std::fprintf(stderr, "\n");
}
extern "C" void ASurfaceTransaction_setBufferAlpha(ASurfaceTransaction* opaque,
                                                      ASurfaceControl* control,
                                                      float alpha) {
  auto* transaction = reinterpret_cast<SurfaceTransaction*>(opaque);
  Remember(transaction, control);
  auto* update = FindUpdate(transaction, control);
  if (update != nullptr) {
    update->has_alpha = true;
    update->alpha = alpha;
  }
}
SURFACE_CONTROL_SETTER(ASurfaceTransaction_setBufferDataSpace,
  (ASurfaceTransaction* transaction, ASurfaceControl* control, enum ADataSpace), control)
extern "C" void ASurfaceTransaction_setColor(
    ASurfaceTransaction* opaque, ASurfaceControl* control, float red,
    float green, float blue, float alpha, enum ADataSpace) {
  auto* transaction = reinterpret_cast<SurfaceTransaction*>(opaque);
  Remember(transaction, control);
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
