#include "darwin_angle_egl.h"
#include "darwin_android_platform.h"
#include "darwin_surface_bridge.h"
#include "window/locked_surface.h"
#include "window/native_window_transaction_consumer.h"
#include "window/native_window_buffer_queue.h"

#include <android/hardware_buffer.h>
#include <android/surface_control.h>

#include <algorithm>
#include <atomic>
#include <cerrno>
#include <cstdarg>
#include <cstdint>
#include <cstdlib>
#include <cstring>
#include <iostream>
#include <memory>
#include <mutex>
#include <new>
#include <unordered_map>
#include <vector>

#include <unistd.h>

extern "C" int sync_wait(int fd, int timeout_ms);
extern "C" int darwin_art_bionic_socket_broker_close(int fd);
extern "C" int darwin_art_bionic_socket_broker_dup(int fd);
extern "C" int ASurfaceTransactionStats_getPreviousReleaseFenceFd(
    ASurfaceTransactionStats* stats, ASurfaceControl* control);

namespace {
struct DarwinAndroidNativeWindowBuffer {
  std::vector<uint8_t> pixels;
  uint32_t width = 0;
  uint32_t height = 0;
  uint32_t stride_pixels = 0;
  int32_t format = 1;
  uint64_t generation = 0;
};

struct AndroidNativeBaseAbi {
  int32_t magic;
  int32_t version;
  void* reserved[4];
  void (*inc_ref)(AndroidNativeBaseAbi* base);
  void (*dec_ref)(AndroidNativeBaseAbi* base);
};

struct AndroidNativeWindowAbi {
  AndroidNativeBaseAbi common;
  uint32_t flags;
  int32_t min_swap_interval;
  int32_t max_swap_interval;
  float xdpi;
  float ydpi;
  intptr_t oem[4];
  int (*set_swap_interval)(AndroidNativeWindowAbi*, int);
  int (*dequeue_buffer_deprecated)(AndroidNativeWindowAbi*, void**);
  int (*lock_buffer_deprecated)(AndroidNativeWindowAbi*, void*);
  int (*queue_buffer_deprecated)(AndroidNativeWindowAbi*, void*);
  int (*query)(const AndroidNativeWindowAbi*, int, int*);
  int (*perform)(AndroidNativeWindowAbi*, int, ...);
  int (*cancel_buffer_deprecated)(AndroidNativeWindowAbi*, void*);
  int (*dequeue_buffer)(AndroidNativeWindowAbi*, void**, int*);
  int (*queue_buffer)(AndroidNativeWindowAbi*, void*, int);
  int (*cancel_buffer)(AndroidNativeWindowAbi*, void*, int);
};

struct NativeWindowTransactionObserver {
  DarwinArtAndroidNativeWindowTransactionCallback callback = nullptr;
  void* context = nullptr;
  void (*release_context)(void*) = nullptr;
  ~NativeWindowTransactionObserver() {
    if (release_context != nullptr) release_context(context);
  }
};

struct NativeWindowQueueObserver {
  DarwinArtAndroidNativeWindowQueueCallback callback = nullptr;
  void* context = nullptr;
  void (*release_context)(void*) = nullptr;
  ~NativeWindowQueueObserver() {
    if (release_context != nullptr) release_context(context);
  }
};

struct DarwinAndroidNativeWindow {
  // Must remain first. HWUI receives this object as a real ANativeWindow and
  // uses its Android native-base refcount/query ABI before handing it to the
  // render thread.
  AndroidNativeWindowAbi abi{};
  jlong java_surface_identity = 0;
  std::atomic<uint32_t> references{1};
  std::atomic<int32_t> width{0};
  std::atomic<int32_t> height{0};
  std::atomic<int32_t> format{1};
  // Android propagates the producer color space through BufferQueue.  The
  // Darwin queue stores the same state on its ANativeWindow producer so HWUI
  // can negotiate wide-color surfaces without a host-side policy override.
  std::atomic<int32_t> dataspace{0};
  std::mutex mutex;
  std::shared_ptr<DarwinAndroidNativeWindowBuffer> locked;
  std::shared_ptr<DarwinAndroidNativeWindowBuffer> published;
  std::unique_ptr<darwin_art::window::NativeWindowBufferQueue> gpu_queue;
  ASurfaceControl* surface_control = nullptr;
  uint32_t imported_surface_owner_process_id = 0;
  uint32_t imported_surface_layer_id = 0;
  std::shared_ptr<NativeWindowQueueObserver> queue_observer;
  std::shared_ptr<NativeWindowTransactionObserver> transaction_observer;
  std::unique_ptr<darwin_art::window::NativeWindowTransactionConsumer>
      transaction_consumer;
};

struct AndroidNativeWindowBufferAbi {
  int32_t width;
  int32_t height;
  int32_t stride;
  int32_t format;
  void* bits;
  uint32_t reserved[6];
};

std::mutex g_android_native_window_mutex;
DarwinAndroidNativeWindow* g_android_native_window_published = nullptr;
std::unordered_map<jlong, DarwinAndroidNativeWindow*>
    g_android_native_windows_by_surface;
struct ImportedSurfaceIdentity {
  uint32_t owner_process_id = 0;
  uint32_t layer_id = 0;
  int32_t width = 0;
  int32_t height = 0;
  int32_t format = 1;
};
std::unordered_map<jlong, ImportedSurfaceIdentity>
    g_imported_surface_identities;
std::atomic<uint64_t> g_android_native_window_generation{0};

bool DebugAndroidNativeWindow() {
  const char* value = std::getenv("DARWIN_ART_DEBUG_ANATIVEWINDOW");
  return value != nullptr && std::strcmp(value, "0") != 0;
}

uint32_t AndroidNativeWindowBytesPerPixel(int32_t format) {
  // android/native_window.h: RGBA_8888=1, RGBX_8888=2, RGB_565=4.
  return format == 4 ? 2u : 4u;
}

DarwinAndroidNativeWindow* WindowFromAbi(AndroidNativeWindowAbi* abi) {
  return reinterpret_cast<DarwinAndroidNativeWindow*>(abi);
}

void ReleaseNativeWindow(DarwinAndroidNativeWindow* window);

void CloseFence(int fence) {
  if (fence >= 0) (void)darwin_art_bionic_socket_broker_close(fence);
}

void ReleaseConsumerSlot(DarwinAndroidNativeWindow* window,
                         int32_t slot_index, int release_fence) {
  if (window == nullptr || window->gpu_queue == nullptr) {
    CloseFence(release_fence);
    return;
  }
  window->gpu_queue->ReturnCurrentSlot(slot_index, release_fence);
}

const DarwinAndroidNativeWindow* WindowFromAbi(
    const AndroidNativeWindowAbi* abi) {
  return reinterpret_cast<const DarwinAndroidNativeWindow*>(abi);
}

void ReleaseNativeWindow(DarwinAndroidNativeWindow* window) {
  if (window == nullptr ||
      window->references.fetch_sub(1, std::memory_order_acq_rel) != 1) {
    return;
  }
  {
    std::lock_guard<std::mutex> global_lock(g_android_native_window_mutex);
    if (g_android_native_window_published == window) {
      g_android_native_window_published = nullptr;
    }
    if (window->java_surface_identity != 0) {
      auto found = g_android_native_windows_by_surface.find(
          window->java_surface_identity);
      if (found != g_android_native_windows_by_surface.end() &&
          found->second == window) {
        g_android_native_windows_by_surface.erase(found);
      }
    }
  }
  if (window->surface_control != nullptr) {
    ASurfaceControl_release(window->surface_control);
  }
  delete window;
}

ASurfaceTransaction* CreateNativeWindowTransaction(void*) {
  return ASurfaceTransaction_create();
}

void DeleteNativeWindowTransaction(void*, ASurfaceTransaction* transaction) {
  if (transaction != nullptr) ASurfaceTransaction_delete(transaction);
}

void ApplyNativeWindowTransaction(void*, ASurfaceTransaction* transaction) {
  if (transaction != nullptr) ASurfaceTransaction_apply(transaction);
}

bool SetNativeWindowBufferChecked(void*, ASurfaceTransaction* transaction,
                                  ASurfaceControl* control,
                                  AHardwareBuffer* buffer, int fence,
                                  uint64_t submission_cookie) {
  return darwin_art_android_surface_transaction_set_buffer_with_cookie_checked(
      transaction, control, buffer, fence, submission_cookie);
}

bool SetNativeWindowCallbacksChecked(
    void*, ASurfaceTransaction* transaction, ASurfaceControl* control,
    void* context, darwin_art::window::NativeWindowTransactionComplete complete,
    darwin_art::window::NativeWindowTransactionDiscard discard) {
  return darwin_art_android_surface_transaction_set_buffer_callbacks_checked(
      transaction, control, context, complete, discard);
}

int DuplicateNativeWindowFence(void*, int fence) {
  return darwin_art_bionic_socket_broker_dup(fence);
}

void CloseNativeWindowFence(void*, int fence) { CloseFence(fence); }

int PreviousNativeWindowReleaseFence(void*, ASurfaceTransactionStats* stats,
                                     ASurfaceControl* control) {
  if (stats == nullptr || control == nullptr) return -1;
  return ASurfaceTransactionStats_getPreviousReleaseFenceFd(stats, control);
}

bool PreviousNativeWindowBufferMetadata(
    void*, ASurfaceTransactionStats* stats, ASurfaceControl* control,
    AHardwareBuffer** buffer, uint64_t* submission_cookie) {
  return ASurfaceTransactionStats_getPreviousBufferMetadata(
      stats, control, buffer, submission_cookie);
}

bool GetNativeWindowLayerIdentity(void*, ASurfaceControl* control,
                                  uint32_t* owner_process, uint32_t* layer) {
  return control != nullptr && owner_process != nullptr && layer != nullptr &&
         darwin_art_android_surface_control_get_identity(control, owner_process,
                                                         layer);
}

void AcquireNativeWindowControl(void*, ASurfaceControl* control) {
  if (control != nullptr) ASurfaceControl_acquire(control);
}

void ReleaseNativeWindowControl(void*, ASurfaceControl* control) {
  if (control != nullptr) ASurfaceControl_release(control);
}

void RetainNativeWindowOwner(void* opaque) {
  auto* window = static_cast<DarwinAndroidNativeWindow*>(opaque);
  if (window != nullptr)
    window->references.fetch_add(1, std::memory_order_relaxed);
}

void ReleaseNativeWindowOwner(void* opaque) {
  ReleaseNativeWindow(static_cast<DarwinAndroidNativeWindow*>(opaque));
}

void ReturnNativeWindowFrame(
    void* opaque, const darwin_art::window::NativeWindowTransactionFrame& frame,
    int fence, bool quarantine) {
  auto* window = static_cast<DarwinAndroidNativeWindow*>(opaque);
  if (window == nullptr || window->gpu_queue == nullptr) {
    CloseFence(fence);
    return;
  }
  window->gpu_queue->Return({frame.slot, frame.generation, frame.frame},
                            fence, quarantine);
}

darwin_art::window::NativeWindowTransactionConsumerHooks
MakeNativeWindowTransactionConsumerHooks(DarwinAndroidNativeWindow* window) {
  return {
      .context = window,
      .create_transaction = &CreateNativeWindowTransaction,
      .delete_transaction = &DeleteNativeWindowTransaction,
      .apply_transaction = &ApplyNativeWindowTransaction,
      .set_buffer_checked = &SetNativeWindowBufferChecked,
      .set_callbacks_checked = &SetNativeWindowCallbacksChecked,
      .duplicate_fence = &DuplicateNativeWindowFence,
      .close_fence = &CloseNativeWindowFence,
      .previous_release_fence = &PreviousNativeWindowReleaseFence,
      .previous_buffer_metadata = &PreviousNativeWindowBufferMetadata,
      .get_layer_identity = &GetNativeWindowLayerIdentity,
      .acquire_control = &AcquireNativeWindowControl,
      .release_control = &ReleaseNativeWindowControl,
      .retain_owner = &RetainNativeWindowOwner,
      .release_owner = &ReleaseNativeWindowOwner,
      .return_frame = &ReturnNativeWindowFrame,
  };
}

void NativeWindowIncRef(AndroidNativeBaseAbi* base) {
  if (base == nullptr) return;
  auto* window = reinterpret_cast<DarwinAndroidNativeWindow*>(base);
  window->references.fetch_add(1, std::memory_order_relaxed);
}

void NativeWindowDecRef(AndroidNativeBaseAbi* base) {
  ReleaseNativeWindow(
      reinterpret_cast<DarwinAndroidNativeWindow*>(base));
}

int NativeWindowSetSwapInterval(AndroidNativeWindowAbi*, int) { return 0; }
int NativeWindowDequeue(AndroidNativeWindowAbi* abi, void** out_buffer,
                        int* out_fence) {
  auto* window = WindowFromAbi(abi);
  if (window == nullptr || !out_buffer || !out_fence) return -EINVAL;
  if (window->gpu_queue == nullptr) return -ENOMEM;
  darwin_art::window::NativeWindowDequeuedBuffer dequeued;
  const int status = window->gpu_queue->Dequeue(&dequeued);
  // Preserve the existing ANativeWindow ABI's invalid-geometry mapping.
  if (status != 0) return status == -EINVAL ? -ENOMEM : status;
  *out_buffer = dequeued.native_buffer;
  *out_fence = dequeued.acquire_fence;
  return 0;
}
int NativeWindowDequeueDeprecated(AndroidNativeWindowAbi* abi,
                                  void** out_buffer) {
  int fence = -1;
  return NativeWindowDequeue(abi, out_buffer, &fence);
}
int NativeWindowUnsupportedBuffer(AndroidNativeWindowAbi*, void*) {
  return -ENOSYS;
}
struct NativeWindowOperationPin {
  explicit NativeWindowOperationPin(DarwinAndroidNativeWindow* value) : window(value) {
    window->references.fetch_add(1, std::memory_order_relaxed);
  }
  ~NativeWindowOperationPin() { ReleaseNativeWindow(window); }
  DarwinAndroidNativeWindow* window;
};

int NativeWindowQueue(AndroidNativeWindowAbi* abi, void* native_buffer,
                      int fence) {
  auto* window = WindowFromAbi(abi);
  if (window == nullptr) {
    CloseFence(fence);
    return -EINVAL;
  }
  NativeWindowOperationPin operation(window);
  darwin_art::window::NativeWindowQueuedBuffer queued;
  AHardwareBuffer* buffer = nullptr;
  std::shared_ptr<NativeWindowQueueObserver> queue_observer;
  ASurfaceControl* control = nullptr;
  int32_t slot_index = -1;
  uint64_t generation = 0;
  uint64_t queued_frame = 0;
  int32_t dataspace = 0;
  std::shared_ptr<NativeWindowTransactionObserver> transaction_observer;
  {
    std::lock_guard<std::mutex> lock(window->mutex);
    if (window->gpu_queue == nullptr) { CloseFence(fence); return -ENOMEM; }
    const int status = window->gpu_queue->Queue(native_buffer, &queued);
    if (status != 0) { CloseFence(fence); return status; }
    buffer = queued.buffer;
    slot_index = queued.token.slot;
    generation = queued.token.generation;
    queued_frame = queued.token.frame;
    queue_observer = window->queue_observer;
    dataspace = window->dataspace.load(std::memory_order_acquire);
    transaction_observer = window->transaction_observer;
    if (queue_observer == nullptr || queue_observer->callback == nullptr) {
      if (window->surface_control == nullptr) {
        // A Surface transported to another Android process still queues into
        // the original BufferQueue layer.  The producer process owns only a
        // local proxy; creating a new process-owned root here makes the layer
        // disappear when (for example) Chromium restarts its GPU process.
        // Reconstruct the imported SurfaceControl identity so a replacement
        // producer can attach to the SurfaceView-owned layer exactly as it
        // would through IGraphicBufferProducer on Android.
        window->surface_control = reinterpret_cast<ASurfaceControl*>(
            window->imported_surface_owner_process_id != 0 &&
                    window->imported_surface_layer_id != 0
                ? darwin_art_android_surface_control_create_imported(
                      window->imported_surface_owner_process_id,
                      window->imported_surface_layer_id,
                      "Imported BufferQueue producer")
                : darwin_art_android_surface_control_create_root(
                      "HWUI ViewRoot"));
      }
      control = window->surface_control;
      if (control != nullptr) ASurfaceControl_acquire(control);
    }
  }
  if (queue_observer != nullptr && queue_observer->callback != nullptr) {
    // The callback is arbitrary consumer code and may release the producer.
    // Keep this window alive across the unlocked callback invocation.
    queue_observer->callback(queue_observer->context, buffer, slot_index,
                             fence, dataspace);
    return 0;
  }
  if (DebugAndroidNativeWindow()) {
    uint32_t owner_process_id = 0;
    uint32_t layer_id = 0;
    const bool identified = darwin_art_android_surface_control_get_identity(
        control, &owner_process_id, &layer_id);
    std::cerr << "ART Android ANativeWindow: queue window=" << window
              << " control=" << control
              << " identified=" << (identified ? 1 : 0)
              << " owner=" << owner_process_id << " layer=" << layer_id
              << " slot=" << slot_index << "\n";
  }
  if (window->transaction_consumer == nullptr) {
    ReturnNativeWindowFrame(
        window,
        {.control = control,
         .buffer = buffer,
        .slot = slot_index,
        .generation = generation,
         .frame = queued_frame,
         .control_retained = control != nullptr},
        fence, false);
    if (control != nullptr) ASurfaceControl_release(control);
    return -ENOMEM;
  }
  const darwin_art::window::NativeWindowTransactionFrame frame{
      .control = control,
      .buffer = buffer,
      .slot = slot_index,
      .generation = generation,
      .frame = queued_frame,
      .control_retained = control != nullptr};
  std::shared_ptr<void> transaction_observer_lifetime;
  if (transaction_observer != nullptr)
    transaction_observer_lifetime = std::shared_ptr<void>(
        transaction_observer, transaction_observer.get());
  const bool submitted = window->transaction_consumer->Submit(
      frame, fence,
      transaction_observer != nullptr ? transaction_observer->callback : nullptr,
      transaction_observer != nullptr ? transaction_observer->context : nullptr,
      std::move(transaction_observer_lifetime));
  return submitted ? 0 : -ENOMEM;
}

int NativeWindowCancel(AndroidNativeWindowAbi* abi, void* native_buffer,
                       int fence) {
  auto* window = WindowFromAbi(abi);
  if (window == nullptr || window->gpu_queue == nullptr) {
    CloseFence(fence);
    return -EINVAL;
  }
  return window->gpu_queue->Cancel(native_buffer, fence);
}

int NativeWindowQuery(const AndroidNativeWindowAbi* abi, int what, int* value) {
  if (abi == nullptr || value == nullptr) return -EINVAL;
  const auto* window = WindowFromAbi(abi);
  switch (what) {
    case 0:  // NATIVE_WINDOW_WIDTH
    case 7:  // NATIVE_WINDOW_DEFAULT_WIDTH
      *value = window->width.load(std::memory_order_relaxed);
      return 0;
    case 1:  // NATIVE_WINDOW_HEIGHT
    case 8:  // NATIVE_WINDOW_DEFAULT_HEIGHT
      *value = window->height.load(std::memory_order_relaxed);
      return 0;
    case 2:  // NATIVE_WINDOW_FORMAT
      *value = window->format.load(std::memory_order_relaxed);
      return 0;
    case 3:  // NATIVE_WINDOW_MIN_UNDEQUEUED_BUFFERS
      *value = 0;
      return 0;
    case 4:  // NATIVE_WINDOW_QUEUES_TO_WINDOW_COMPOSER
      *value = 1;
      return 0;
    case 6:  // NATIVE_WINDOW_TRANSFORM_HINT
      *value = 0;
      return 0;
    case 17:  // NATIVE_WINDOW_IS_VALID
      *value = 1;
      return 0;
    case 20:  // NATIVE_WINDOW_DATASPACE
      *value = window->dataspace.load(std::memory_order_acquire);
      return 0;
    default:
      return -ENOENT;
  }
}

int NativeWindowPerform(AndroidNativeWindowAbi* abi, int operation, ...) {
  if (abi == nullptr) return -EINVAL;
  auto* window = WindowFromAbi(abi);
  if (operation == 24) {  // NATIVE_WINDOW_GET_NEXT_FRAME_ID
    va_list arguments;
    va_start(arguments, operation);
    auto* frame_id = va_arg(arguments, uint64_t*);
    va_end(arguments);
    if (frame_id == nullptr) return -EINVAL;
    std::lock_guard<std::mutex> lock(window->mutex);
    *frame_id = window->gpu_queue != nullptr ? window->gpu_queue->NextFrame() : 0;
    return 0;
  }
  // Frame-rate policy has not yet been connected to IGraphicBufferProducer.
  // Propagate that missing contract instead of silently accepting the request.
  if (operation == 40) return -ENOSYS;
  if (operation == 19) {  // NATIVE_WINDOW_SET_BUFFERS_DATASPACE
    va_list arguments;
    va_start(arguments, operation);
    const int32_t dataspace = va_arg(arguments, int32_t);
    va_end(arguments);
    window->dataspace.store(dataspace, std::memory_order_release);
  }
  // The Darwin BufferQueue accepts the remaining producer configuration as
  // advisory state until each operation has a corresponding Composer field.
  return 0;
}

int PrepareGpuSwapchainLocked(DarwinAndroidNativeWindow* window,
                              int32_t width, int32_t height) {
  if (window == nullptr || width <= 0 || height <= 0) return -EINVAL;
  if (window->gpu_queue == nullptr) return -ENOMEM;
  const int status = window->gpu_queue->PrepareGeometry(width, height);
  if (status == 0) {
    window->width.store(width, std::memory_order_relaxed);
    window->height.store(height, std::memory_order_relaxed);
  }
  return status;
}

void InitializeNativeWindowAbi(DarwinAndroidNativeWindow* window) {
  constexpr int32_t kAndroidNativeWindowMagic =
      ('_' << 24) | ('w' << 16) | ('n' << 8) | 'd';
  window->abi.common.magic = kAndroidNativeWindowMagic;
  window->abi.common.version = sizeof(AndroidNativeWindowAbi);
  window->abi.common.inc_ref = &NativeWindowIncRef;
  window->abi.common.dec_ref = &NativeWindowDecRef;
  window->abi.min_swap_interval = 0;
  window->abi.max_swap_interval = 1;
  window->abi.set_swap_interval = &NativeWindowSetSwapInterval;
  window->abi.dequeue_buffer_deprecated = &NativeWindowDequeueDeprecated;
  window->abi.lock_buffer_deprecated = &NativeWindowUnsupportedBuffer;
  window->abi.queue_buffer_deprecated = &NativeWindowUnsupportedBuffer;
  window->abi.query = &NativeWindowQuery;
  window->abi.perform = &NativeWindowPerform;
  window->abi.cancel_buffer_deprecated = &NativeWindowUnsupportedBuffer;
  window->abi.dequeue_buffer = &NativeWindowDequeue;
  window->abi.queue_buffer = &NativeWindowQueue;
  window->abi.cancel_buffer = &NativeWindowCancel;
}
}  // namespace

extern "C" void* darwin_art_android_ANativeWindow_fromSurface(void* opaque_env,
                                                                void* surface) {
  auto* env = static_cast<JNIEnv*>(opaque_env);
  const jobject java_surface = static_cast<jobject>(surface);
  darwin_art::window::LockedSurface surface_owner(env, java_surface);
  const jlong identity = surface_owner.identity();
  if (identity == 0) {
    if (DebugAndroidNativeWindow()) {
      std::cerr << "ART Android ANativeWindow: fromSurface rejected pid="
                << getpid() << " javaSurface=" << surface
                << " identity=0\n";
    }
    return nullptr;
  }
  if (identity != 0) {
    std::lock_guard<std::mutex> lock(g_android_native_window_mutex);
    auto found = g_android_native_windows_by_surface.find(identity);
    if (found != g_android_native_windows_by_surface.end()) {
      found->second->references.fetch_add(1, std::memory_order_relaxed);
      if (DebugAndroidNativeWindow()) {
        std::cerr << "ART Android ANativeWindow: fromSurface cached pid="
                  << getpid() << " identity=" << identity
                  << " window=" << found->second << "\n";
      }
      return found->second;
    }
  }
  auto* window = new (std::nothrow) DarwinAndroidNativeWindow();
  if (window == nullptr) return nullptr;
  InitializeNativeWindowAbi(window);
  window->java_surface_identity = identity;
  window->width.store(darwin_art::DarwinAngleHostSurfaceWidth(),
                      std::memory_order_relaxed);
  window->height.store(darwin_art::DarwinAngleHostSurfaceHeight(),
                       std::memory_order_relaxed);
  try {
    window->transaction_consumer = std::make_unique<
        darwin_art::window::NativeWindowTransactionConsumer>(
        MakeNativeWindowTransactionConsumerHooks(window));
  } catch (const std::bad_alloc&) {
    delete window;
    return nullptr;
  }
  if (identity != 0) {
    std::unique_lock<std::mutex> lock(g_android_native_window_mutex);
    const auto imported = g_imported_surface_identities.find(identity);
    if (imported != g_imported_surface_identities.end()) {
      window->imported_surface_owner_process_id = imported->second.owner_process_id;
      window->imported_surface_layer_id = imported->second.layer_id;
      if (imported->second.width > 0 && imported->second.height > 0) {
        window->width.store(imported->second.width, std::memory_order_relaxed);
        window->height.store(imported->second.height, std::memory_order_relaxed);
      }
      window->format.store(imported->second.format, std::memory_order_relaxed);
    }
    // The queue must be complete before publishing this facade to another
    // thread. Imported geometry is resolved under the same registry lock.
    try {
      window->gpu_queue = std::make_unique<darwin_art::window::NativeWindowBufferQueue>(
          window->width.load(std::memory_order_relaxed),
          window->height.load(std::memory_order_relaxed));
    } catch (const std::bad_alloc&) {
      lock.unlock();
      delete window;
      return nullptr;
    }
    try {
      auto [found, inserted] =
          g_android_native_windows_by_surface.emplace(identity, window);
      if (!inserted) {
        found->second->references.fetch_add(1, std::memory_order_relaxed);
        auto* existing = found->second;
        lock.unlock();
        delete window;
        return existing;
      }
    } catch (const std::bad_alloc&) {
      lock.unlock();
      delete window;
      return nullptr;
    }
  }
  if (DebugAndroidNativeWindow()) {
    std::cerr << "ART Android ANativeWindow: fromSurface pid=" << getpid()
              << " javaSurface=" << surface << " identity=0x" << std::hex
              << identity << std::dec << " window=" << window
              << " size=" << window->width.load(std::memory_order_relaxed)
              << "x" << window->height.load(std::memory_order_relaxed)
              << " format=" << window->format.load(std::memory_order_relaxed)
              << " imported=" << window->imported_surface_owner_process_id
              << ":" << window->imported_surface_layer_id
              << "\n";
  }
  if (identity == 0) try {
    window->gpu_queue = std::make_unique<darwin_art::window::NativeWindowBufferQueue>(
        window->width.load(std::memory_order_relaxed),
        window->height.load(std::memory_order_relaxed));
  } catch (const std::bad_alloc&) {
    delete window;
    return nullptr;
  }
  return window;
}

extern "C" void* darwin_art_android_ANativeWindow_create(
    int32_t width, int32_t height, int32_t format) {
  auto* window = new (std::nothrow) DarwinAndroidNativeWindow();
  if (window == nullptr) return nullptr;
  InitializeNativeWindowAbi(window);
  window->width.store(width, std::memory_order_relaxed);
  window->height.store(height, std::memory_order_relaxed);
  window->format.store(format, std::memory_order_relaxed);
  window->java_surface_identity = reinterpret_cast<jlong>(window);
  try {
    window->gpu_queue = std::make_unique<darwin_art::window::NativeWindowBufferQueue>(width, height);
    window->transaction_consumer = std::make_unique<
        darwin_art::window::NativeWindowTransactionConsumer>(
        MakeNativeWindowTransactionConsumerHooks(window));
  } catch (const std::bad_alloc&) {
    delete window;
    return nullptr;
  }
  try {
    std::lock_guard<std::mutex> lock(g_android_native_window_mutex);
    g_android_native_windows_by_surface.emplace(window->java_surface_identity,
                                                 window);
  } catch (const std::bad_alloc&) {
    // Registry lock has unwound before resource-owning facade destruction.
    delete window;
    return nullptr;
  }
  return window;
}

extern "C" void darwin_art_android_ANativeWindow_acquire(void* opaque) {
  auto* window = static_cast<DarwinAndroidNativeWindow*>(opaque);
  if (window != nullptr) window->references.fetch_add(1, std::memory_order_relaxed);
}

extern "C" int32_t darwin_art_android_ANativeWindow_getFormat(void* opaque) {
  auto* window = static_cast<DarwinAndroidNativeWindow*>(opaque);
  return window == nullptr ? 0 : window->format.load(std::memory_order_relaxed);
}

extern "C" int32_t darwin_art_android_ANativeWindow_getWidth(void* opaque) {
  auto* window = static_cast<DarwinAndroidNativeWindow*>(opaque);
  return window == nullptr ? 0 : window->width.load(std::memory_order_relaxed);
}

extern "C" int32_t darwin_art_android_ANativeWindow_getHeight(void* opaque) {
  auto* window = static_cast<DarwinAndroidNativeWindow*>(opaque);
  return window == nullptr ? 0 : window->height.load(std::memory_order_relaxed);
}

extern "C" void* darwin_art_android_ANativeWindow_toSurface(void*, void*) {
  // The framework Surface wrapper is created by the Java bridge. Native
  // callers still retain and render through the stable ANativeWindow token.
  return nullptr;
}

extern "C" void darwin_art_android_ANativeWindow_release(void* opaque) {
  ReleaseNativeWindow(static_cast<DarwinAndroidNativeWindow*>(opaque));
}

extern "C" void darwin_art_android_ANativeWindow_set_queue_callback(
    void* opaque, DarwinArtAndroidNativeWindowQueueCallback callback,
    void* context) {
  (void)darwin_art_android_ANativeWindow_set_owned_queue_callback(
      opaque, callback, context, nullptr);
}

extern "C" bool darwin_art_android_ANativeWindow_set_owned_queue_callback(
    void* opaque, DarwinArtAndroidNativeWindowQueueCallback callback,
    void* context, void (*release_context)(void*)) {
  auto* window = static_cast<DarwinAndroidNativeWindow*>(opaque);
  if (window == nullptr) return false;
  std::shared_ptr<NativeWindowQueueObserver> observer;
  if (callback != nullptr) {
    try {
      observer = std::make_shared<NativeWindowQueueObserver>();
    } catch (const std::bad_alloc&) {
      return false;
    }
    observer->callback = callback;
    observer->context = context;
    observer->release_context = release_context;
  }
  {
    std::lock_guard<std::mutex> lock(window->mutex);
    observer.swap(window->queue_observer);
  }
  // Consumer cleanup can reenter the still-retained producer (for example to
  // return acquired slots). Release the previous observer outside its mutex.
  return true;
}

extern "C" bool darwin_art_android_ANativeWindow_set_transaction_callback(
    void* opaque, DarwinArtAndroidNativeWindowTransactionCallback callback,
    void* context, void (*release_context)(void*)) {
  auto* window = static_cast<DarwinAndroidNativeWindow*>(opaque);
  if (window == nullptr) return false;
  std::shared_ptr<NativeWindowTransactionObserver> observer;
  if (callback != nullptr) {
    try {
      observer = std::make_shared<NativeWindowTransactionObserver>();
    } catch (const std::bad_alloc&) {
      return false;
    }
    observer->callback = callback;
    observer->context = context;
    observer->release_context = release_context;
  }
  {
    std::lock_guard<std::mutex> lock(window->mutex);
    observer.swap(window->transaction_observer);
  }
  // Destruction can release JNI globals or other queue state; never do it
  // while holding the producer mutex.
  return true;
}

extern "C" uint64_t darwin_art_android_ANativeWindow_next_frame_number(
    void* opaque) {
  auto* window = static_cast<DarwinAndroidNativeWindow*>(opaque);
  if (window == nullptr) return 0;
  std::lock_guard<std::mutex> lock(window->mutex);
  return window->gpu_queue != nullptr ? window->gpu_queue->NextFrame() : 0;
}

extern "C" void darwin_art_android_ANativeWindow_release_consumer_slot(
    void* opaque, int32_t slot_index, int release_fence) {
  auto* window = static_cast<DarwinAndroidNativeWindow*>(opaque);
  ReleaseConsumerSlot(window, slot_index, release_fence);
}

extern "C" void darwin_art_android_ANativeWindow_release_consumer_frame(
    void* opaque, int32_t slot_index, uint64_t generation, uint64_t frame,
    int release_fence) {
  auto* window = static_cast<DarwinAndroidNativeWindow*>(opaque);
  if (window == nullptr) {
    CloseFence(release_fence);
    return;
  }
  if (window->gpu_queue == nullptr) { CloseFence(release_fence); return; }
  window->gpu_queue->Return({slot_index, generation, frame}, release_fence, false);
}

extern "C" void darwin_art_android_ANativeWindow_set_surface_control(
    void* opaque, void* opaque_control) {
  auto* window = static_cast<DarwinAndroidNativeWindow*>(opaque);
  auto* control = static_cast<ASurfaceControl*>(opaque_control);
  if (window == nullptr) return;
  if (control != nullptr) ASurfaceControl_acquire(control);
  ASurfaceControl* previous = nullptr;
  {
    std::lock_guard<std::mutex> lock(window->mutex);
    previous = window->surface_control;
    window->surface_control = control;
  }
  if (previous != nullptr) ASurfaceControl_release(previous);
}

extern "C" bool darwin_art_android_ANativeWindow_get_surface_control_identity(
    void* opaque, uint32_t* owner_process_id, uint32_t* layer_id) {
  if (opaque == nullptr || owner_process_id == nullptr || layer_id == nullptr) {
    return false;
  }
  DarwinAndroidNativeWindow* window = nullptr;
  {
    std::lock_guard<std::mutex> global_lock(g_android_native_window_mutex);
    const auto found = g_android_native_windows_by_surface.find(
        reinterpret_cast<jlong>(opaque));
    if (found == g_android_native_windows_by_surface.end()) return false;
    window = found->second;
  }
  std::lock_guard<std::mutex> lock(window->mutex);
  return window->surface_control != nullptr &&
         darwin_art_android_surface_control_get_identity(
             window->surface_control, owner_process_id, layer_id);
}

extern "C" void
darwin_art_android_ANativeWindow_register_imported_surface_identity(
    int64_t surface_identity, uint32_t owner_process_id, uint32_t layer_id,
    int32_t width, int32_t height, int32_t format) {
  if (surface_identity == 0 || owner_process_id == 0 || layer_id == 0) return;
  std::lock_guard<std::mutex> lock(g_android_native_window_mutex);
  // Reserve registry storage before mutating queue or facade geometry. This
  // void ABI cannot report allocation/busy failure, so preserve the prior
  // publication rather than expose metadata that its producer cannot honor.
  decltype(g_imported_surface_identities)::iterator imported;
  bool inserted = false;
  try {
    auto result = g_imported_surface_identities.try_emplace(surface_identity);
    imported = result.first;
    inserted = result.second;
  } catch (const std::bad_alloc&) {
    return;
  }
  const auto found = g_android_native_windows_by_surface.find(surface_identity);
  if (found != g_android_native_windows_by_surface.end()) {
    auto* window = found->second;
    std::lock_guard<std::mutex> window_lock(window->mutex);
    if (width > 0 && height > 0 &&
        (window->gpu_queue == nullptr ||
         window->gpu_queue->UpdateGeometry(width, height) != 0)) {
      if (inserted) g_imported_surface_identities.erase(imported);
      return;
    }
    window->imported_surface_owner_process_id = owner_process_id;
    window->imported_surface_layer_id = layer_id;
    if (width > 0 && height > 0) {
      window->width.store(width, std::memory_order_relaxed);
      window->height.store(height, std::memory_order_relaxed);
    }
    window->format.store(format, std::memory_order_relaxed);
  }
  imported->second = {
      .owner_process_id = owner_process_id,
      .layer_id = layer_id,
      .width = width,
      .height = height,
      .format = format,
  };
}

extern "C" bool
darwin_art_android_ANativeWindow_get_imported_surface_identity(
    void* opaque, uint32_t* owner_process_id, uint32_t* layer_id) {
  auto* window = static_cast<DarwinAndroidNativeWindow*>(opaque);
  if (window == nullptr || owner_process_id == nullptr || layer_id == nullptr) {
    return false;
  }
  std::lock_guard<std::mutex> lock(window->mutex);
  if (window->imported_surface_owner_process_id == 0 ||
      window->imported_surface_layer_id == 0) {
    return false;
  }
  *owner_process_id = window->imported_surface_owner_process_id;
  *layer_id = window->imported_surface_layer_id;
  return true;
}

extern "C" bool darwin_art_android_ANativeWindow_release_if_managed(
    void* opaque) {
  if (opaque == nullptr) return false;
  auto* window = static_cast<DarwinAndroidNativeWindow*>(opaque);
  {
    std::lock_guard<std::mutex> lock(g_android_native_window_mutex);
    const auto found = g_android_native_windows_by_surface.find(
        reinterpret_cast<jlong>(opaque));
    if (found == g_android_native_windows_by_surface.end() ||
        found->second != window) {
      return false;
    }
  }
  ReleaseNativeWindow(window);
  return true;
}

extern "C" bool darwin_art_android_ANativeWindow_is_managed(void* opaque) {
  if (opaque == nullptr) return false;
  std::lock_guard<std::mutex> lock(g_android_native_window_mutex);
  // fromSurface uses Surface.mNativeObject as the registry key, which is not
  // required to equal the returned ANativeWindow address. Validate the value
  // as well as the token so arbitrary pointers cannot pass this check.
  return std::any_of(
      g_android_native_windows_by_surface.begin(),
      g_android_native_windows_by_surface.end(),
      [opaque](const auto& entry) {
        return entry.second == static_cast<DarwinAndroidNativeWindow*>(opaque);
      });
}

extern "C" int32_t
darwin_art_android_ANativeWindow_dequeue_hardware_buffer(
    void* opaque, AHardwareBuffer** out_buffer, void** out_native_buffer,
    int* out_fence) {
  if (opaque == nullptr || out_buffer == nullptr ||
      out_native_buffer == nullptr || out_fence == nullptr) {
    return -EINVAL;
  }
  *out_buffer = nullptr;
  *out_native_buffer = nullptr;
  *out_fence = -1;
  auto* window = static_cast<DarwinAndroidNativeWindow*>(opaque);
  const int result = NativeWindowDequeue(&window->abi, out_native_buffer,
                                         out_fence);
  if (result != 0) return result;
  *out_buffer = darwin_art_android_hardware_buffer_from_client_buffer(
      *out_native_buffer);
  if (*out_buffer == nullptr) {
    (void)NativeWindowCancel(&window->abi, *out_native_buffer, *out_fence);
    *out_native_buffer = nullptr;
    *out_fence = -1;
    return -EINVAL;
  }
  return 0;
}

extern "C" int32_t darwin_art_android_ANativeWindow_queue_hardware_buffer(
    void* opaque, void* native_buffer, int fence) {
  if (opaque == nullptr) return -EINVAL;
  auto* window = static_cast<DarwinAndroidNativeWindow*>(opaque);
  return NativeWindowQueue(&window->abi, native_buffer, fence);
}

extern "C" int32_t darwin_art_android_ANativeWindow_cancel_hardware_buffer(
    void* opaque, void* native_buffer, int fence) {
  if (opaque == nullptr) return -EINVAL;
  auto* window = static_cast<DarwinAndroidNativeWindow*>(opaque);
  return NativeWindowCancel(&window->abi, native_buffer, fence);
}

extern "C" int32_t darwin_art_android_ANativeWindow_lock(
    void* opaque, void* buffer, void*) {
  auto* window = static_cast<DarwinAndroidNativeWindow*>(opaque);
  auto* native_buffer = static_cast<AndroidNativeWindowBufferAbi*>(buffer);
  if (window == nullptr || native_buffer == nullptr) return -22;
  const int32_t width = window->width.load(std::memory_order_relaxed);
  const int32_t height = window->height.load(std::memory_order_relaxed);
  const int32_t format = window->format.load(std::memory_order_relaxed);
  if (width <= 0 || height <= 0 ||
      (format != 1 && format != 2 && format != 4)) {
    return -22;
  }
  const uint32_t stride =
      (static_cast<uint32_t>(width) + 15u) & ~uint32_t{15};
  const size_t row_bytes =
      static_cast<size_t>(stride) * AndroidNativeWindowBytesPerPixel(format);
  if (row_bytes > SIZE_MAX / static_cast<size_t>(height)) return -12;
  std::shared_ptr<DarwinAndroidNativeWindowBuffer> storage;
  try {
    storage = std::make_shared<DarwinAndroidNativeWindowBuffer>();
    storage->width = static_cast<uint32_t>(width);
    storage->height = static_cast<uint32_t>(height);
    storage->stride_pixels = stride;
    storage->format = format;
    storage->pixels.resize(row_bytes * static_cast<size_t>(height));
  } catch (const std::bad_alloc&) {
    return -12;
  }
  {
    std::lock_guard<std::mutex> lock(window->mutex);
    if (window->locked != nullptr) return -16;
    window->locked = storage;
  }
  *native_buffer = AndroidNativeWindowBufferAbi{
      .width = width,
      .height = height,
      .stride = static_cast<int32_t>(stride),
      .format = format,
      .bits = storage->pixels.data(),
      .reserved = {},
  };
  if (DebugAndroidNativeWindow()) {
    std::cerr << "ART Android ANativeWindow: lock " << width << "x" << height
              << " stride=" << stride << " format=" << format << "\n";
  }
  return 0;
}

extern "C" int32_t darwin_art_android_ANativeWindow_unlockAndPost(
    void* opaque) {
  auto* window = static_cast<DarwinAndroidNativeWindow*>(opaque);
  if (window == nullptr) return -22;
  {
    std::lock_guard<std::mutex> lock(window->mutex);
    if (window->locked == nullptr) return -22;
    window->locked->generation =
        g_android_native_window_generation.fetch_add(
            1, std::memory_order_acq_rel) +
        1;
    window->published = std::move(window->locked);
  }
  {
    std::lock_guard<std::mutex> global_lock(g_android_native_window_mutex);
    g_android_native_window_published = window;
  }
  darwin_art_surface_gpu_publish_embedded(darwin_art_surface_active_gpu());
  if (DebugAndroidNativeWindow()) {
    std::cerr << "ART Android ANativeWindow: post generation="
              << g_android_native_window_generation.load(
                     std::memory_order_relaxed)
              << "\n";
  }
  return 0;
}

extern "C" int32_t darwin_art_android_ANativeWindow_prepare_swapchain(
    void* opaque, int32_t width, int32_t height) {
  auto* window = static_cast<DarwinAndroidNativeWindow*>(opaque);
  if (window == nullptr) return -EINVAL;
  std::lock_guard<std::mutex> lock(window->mutex);
  return PrepareGpuSwapchainLocked(window, width, height);
}

extern "C" int32_t darwin_art_android_ANativeWindow_setBuffersGeometry(
    void* opaque, int32_t width, int32_t height, int32_t format) {
  auto* window = static_cast<DarwinAndroidNativeWindow*>(opaque);
  if (window == nullptr) return -22;
  if (width < 0 || height < 0) return -22;
  std::lock_guard<std::mutex> lock(window->mutex);
  const int32_t new_width = width > 0
                                ? width
                                : window->width.load(std::memory_order_relaxed);
  const int32_t new_height =
      height > 0
          ? height
          : window->height.load(std::memory_order_relaxed);
  const int32_t new_format =
      format != 0 ? format : window->format.load(std::memory_order_relaxed);
  if (new_width <= 0 || new_height <= 0) return -22;
  const int32_t old_width = window->width.load(std::memory_order_relaxed);
  const int32_t old_height = window->height.load(std::memory_order_relaxed);
  const int32_t old_format = window->format.load(std::memory_order_relaxed);
  if (new_width != old_width || new_height != old_height ||
      new_format != old_format) {
    const int result = PrepareGpuSwapchainLocked(window, new_width, new_height);
    if (result != 0) return result;
  }
  window->width.store(new_width, std::memory_order_relaxed);
  window->height.store(new_height, std::memory_order_relaxed);
  window->format.store(new_format, std::memory_order_relaxed);
  if (DebugAndroidNativeWindow()) {
    std::cerr << "ART Android ANativeWindow: geometry " << width << "x"
              << height << " format=" << format << "\n";
  }
  return 0;
}

extern "C" bool darwin_art_android_ANativeWindow_acquire_frame(
    DarwinArtAndroidNativeWindowFrame* frame) {
  if (frame == nullptr) return false;
  *frame = DarwinArtAndroidNativeWindowFrame{};
  std::shared_ptr<DarwinAndroidNativeWindowBuffer> storage;
  {
    std::lock_guard<std::mutex> global_lock(g_android_native_window_mutex);
    DarwinAndroidNativeWindow* window = g_android_native_window_published;
    if (window == nullptr) return false;
    std::lock_guard<std::mutex> lock(window->mutex);
    storage = window->published;
  }
  if (storage == nullptr || storage->pixels.empty()) return false;
  auto* owner = new (std::nothrow)
      std::shared_ptr<DarwinAndroidNativeWindowBuffer>(std::move(storage));
  if (owner == nullptr) return false;
  const auto& held = **owner;
  *frame = DarwinArtAndroidNativeWindowFrame{
      .pixels = held.pixels.data(),
      .size = held.pixels.size(),
      .width = held.width,
      .height = held.height,
      .stride_pixels = held.stride_pixels,
      .format = held.format,
      .generation = held.generation,
      .owner = owner,
  };
  return true;
}

extern "C" void darwin_art_android_ANativeWindow_release_frame(
    DarwinArtAndroidNativeWindowFrame* frame) {
  if (frame == nullptr) return;
  delete static_cast<
      std::shared_ptr<DarwinAndroidNativeWindowBuffer>*>(frame->owner);
  *frame = DarwinArtAndroidNativeWindowFrame{};
}
