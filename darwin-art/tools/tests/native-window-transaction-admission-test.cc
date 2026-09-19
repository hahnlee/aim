#include "compat/window/surface_transaction_builder.h"
#include "compat/window/surface_transaction_lifetime.h"

#include <android/hardware_buffer.h>
#include <android/surface_control.h>

#include <cassert>
#include <cstdint>
#include <cstring>
#include <cstdio>
#include <cstdlib>
#include <new>
#include <mutex>
#include <thread>
#include <unordered_map>

// This test includes the actual NativeWindowQueue owner below. The small
// platform/resource stubs keep the test isolated from the shared runtime
// product while exercising the production slot rollback and checked-builder
// call site.
struct AHardwareBuffer {
  int references = 1;
  uint32_t width = 0;
  uint32_t height = 0;
};
struct ASurfaceControl {
  int references = 1;
  uint32_t owner_process = 1;
  uint32_t layer = 0;
};

bool g_fail_next_allocation = false;
thread_local int g_allocation_failure_after = -1;
thread_local bool g_countdown_failure_triggered = false;
void* operator new(std::size_t size) {
  if (g_allocation_failure_after == 0) {
    g_allocation_failure_after = -1;
    g_countdown_failure_triggered = true;
    throw std::bad_alloc();
  }
  if (g_allocation_failure_after > 0) --g_allocation_failure_after;
  if (g_fail_next_allocation) {
    g_fail_next_allocation = false;
    throw std::bad_alloc();
  }
  if (void* result = std::malloc(size == 0 ? 1 : size)) return result;
  throw std::bad_alloc();
}
void* operator new[](std::size_t size) { return ::operator new(size); }
void operator delete(void* pointer) noexcept { std::free(pointer); }
void operator delete[](void* pointer) noexcept { std::free(pointer); }
void operator delete(void* pointer, std::size_t) noexcept {
  std::free(pointer);
}
void operator delete[](void* pointer, std::size_t) noexcept {
  std::free(pointer);
}

#include "compat/darwin_android_native_window.cc"

namespace {

int g_close_count = 0;
int g_dup_count = 0;
int g_callback_registration_count = 0;
int g_apply_count = 0;
bool g_dup_fails = false;
bool g_signal_fence = false;
bool g_create_transaction_fails = false;
bool g_builder_fails = true;
bool g_registration_fails = false;
bool g_complete_on_apply = false;
bool g_completion_allocation_fails = false;
bool g_control_retention_fails = false;
std::mutex g_latch_mutex;
std::unordered_map<uint64_t, AHardwareBuffer*> g_latched_buffers;
std::unordered_map<uint64_t, uint64_t> g_latched_submission_cookies;
struct RegistrationPause {
  std::atomic<bool> entered{false};
  std::atomic<bool> release{false};
};
thread_local RegistrationPause* g_registration_pause = nullptr;
thread_local RegistrationPause* g_observer_pause = nullptr;

// Slot state is intentionally private to NativeWindowBufferQueue.  Probe the
// public dequeue/cancel contract instead: a held or fenced frame cannot be
// dequeued, while a returned frame can be observed and immediately cancelled.
bool CanDequeueNative(DarwinAndroidNativeWindow* window, void* native,
                      bool cancel = true) {
  if (window == nullptr || window->gpu_queue == nullptr || native == nullptr) {
    return false;
  }
  for (size_t attempt = 0; attempt != 4; ++attempt) {
    darwin_art::window::NativeWindowDequeuedBuffer dequeued;
    if (window->gpu_queue->Dequeue(&dequeued) != 0) return false;
    const bool found = dequeued.native_buffer == native;
    if (cancel || !found) {
      assert(window->gpu_queue->Cancel(dequeued.native_buffer,
                                       dequeued.acquire_fence) == 0);
    }
    if (found) return true;
  }
  return false;
}

jclass SurfaceClass = reinterpret_cast<jclass>(0x61);
jobject SurfaceLock = reinterpret_cast<jobject>(0x62);
jfieldID SurfaceLockField = reinterpret_cast<jfieldID>(0x63);
jfieldID SurfaceNativeField = reinterpret_cast<jfieldID>(0x64);
jlong imported_surface_identity = 0;

jclass SurfaceGetObjectClass(JNIEnv*, jobject) {
  return SurfaceClass;
}
jfieldID SurfaceGetFieldID(JNIEnv*, jclass, const char* name, const char*) {
  if (std::strcmp(name, "mLock") == 0) return SurfaceLockField;
  if (std::strcmp(name, "mNativeObject") == 0) return SurfaceNativeField;
  return nullptr;
}
jobject SurfaceGetObjectField(JNIEnv*, jobject, jfieldID field) {
  return field == SurfaceLockField ? SurfaceLock : nullptr;
}
jlong SurfaceGetLongField(JNIEnv*, jobject, jfieldID field) {
  return field == SurfaceNativeField ? imported_surface_identity : 0;
}
jint SurfaceMonitorEnter(JNIEnv*, jobject) { return JNI_OK; }
jint SurfaceMonitorExit(JNIEnv*, jobject) { return JNI_OK; }
jboolean SurfaceExceptionCheck(JNIEnv*) { return JNI_FALSE; }
void SurfaceDeleteLocalRef(JNIEnv*, jobject) {}

}  // namespace

extern "C" void AHardwareBuffer_acquire(AHardwareBuffer* buffer) {
  if (buffer != nullptr) ++buffer->references;
}
extern "C" void AHardwareBuffer_release(AHardwareBuffer* buffer) {
  if (buffer != nullptr && --buffer->references == 0) delete buffer;
}
// Software Canvas mapping is outside this transaction admission fixture.
// Keep its newly linked resource boundaries explicit and fail on unexpected use.
extern "C" void AHardwareBuffer_describe(const AHardwareBuffer*,
                                          AHardwareBuffer_Desc*) {
  std::abort();
}
extern "C" int AHardwareBuffer_lock(AHardwareBuffer*, uint64_t, int32_t,
                                     const ARect*, void**) {
  std::abort();
}
extern "C" int AHardwareBuffer_unlock(AHardwareBuffer*, int32_t*) {
  std::abort();
}
extern "C" void darwin_art_android_hardware_buffer_mark_cpu_rgba(
    AHardwareBuffer*) {
  std::abort();
}
extern "C" int AHardwareBuffer_allocate(const AHardwareBuffer_Desc* description,
                                          AHardwareBuffer** out) {
  if (description == nullptr || out == nullptr) return -22;
  *out = new (std::nothrow) AHardwareBuffer();
  if (*out != nullptr) {
    (*out)->width = description->width;
    (*out)->height = description->height;
  }
  return *out == nullptr ? -12 : 0;
}
extern "C" void ASurfaceControl_acquire(ASurfaceControl* control) {
  if (control != nullptr) ++control->references;
}
extern "C" void ASurfaceControl_release(ASurfaceControl* control) {
  if (control != nullptr && --control->references == 0) delete control;
}
extern "C" void* darwin_art_android_hardware_buffer_native_window_buffer(
    AHardwareBuffer* buffer) {
  return buffer;
}
namespace darwin_art {
int32_t DarwinAngleHostSurfaceWidth() { return 32; }
int32_t DarwinAngleHostSurfaceHeight() { return 24; }
}  // namespace darwin_art
extern "C" AHardwareBuffer* darwin_art_android_hardware_buffer_from_client_buffer(
    void* buffer) {
  return static_cast<AHardwareBuffer*>(buffer);
}
extern "C" void* darwin_art_android_hardware_buffer_iosurface(
    AHardwareBuffer*) {
  return nullptr;
}
extern "C" void* darwin_art_android_surface_control_create_root(const char*) {
  static uint32_t next_layer = 1;
  auto* control = new (std::nothrow) ASurfaceControl();
  if (control != nullptr) control->layer = next_layer++;
  return control;
}
extern "C" bool darwin_art_android_surface_control_get_identity(
    void* opaque, uint32_t* owner, uint32_t* layer) {
  auto* control = static_cast<ASurfaceControl*>(opaque);
  if (control == nullptr || owner == nullptr || layer == nullptr) return false;
  *owner = control->owner_process;
  *layer = control->layer;
  if (g_control_retention_fails) g_fail_next_allocation = true;
  return control->layer != 0;
}
extern "C" int sync_wait(int, int) { return g_signal_fence ? 0 : -1; }
extern "C" int darwin_art_bionic_socket_broker_dup(int fd) {
  ++g_dup_count;
  return g_dup_fails ? -1 : fd + 1000;
}
extern "C" int darwin_art_bionic_socket_broker_close(int fd) {
  if (fd >= 0) ++g_close_count;
  return 0;
}

extern "C" bool darwin_art_android_surface_transaction_set_buffer_with_cookie_checked(
    void* transaction, void* control, AHardwareBuffer* buffer, int fence,
    uint64_t cookie) {
  g_fail_next_allocation = g_builder_fails;
  return darwin_art::window::SurfaceTransactionBuilder(
             reinterpret_cast<darwin_art::window::SurfaceTransaction*>(
                 transaction))
      .SetBufferWithSubmissionCookie(reinterpret_cast<ASurfaceControl*>(control),
                                     buffer, fence, cookie);
}
extern "C" ASurfaceTransaction* TestLifetimeCreate();
extern "C" void TestLifetimeDelete(ASurfaceTransaction*);
extern "C" bool TestLifetimeCallbacksChecked(
    void*, void*, void*, void (*)(void*, ASurfaceTransactionStats*),
    void (*)(void*, int));
extern "C" void darwin_art_android_mark_hardware_buffer_released(void*) {}
extern "C" ASurfaceTransaction* ASurfaceTransaction_create() {
  if (g_create_transaction_fails) return nullptr;
  return TestLifetimeCreate();
}
extern "C" void ASurfaceTransaction_delete(ASurfaceTransaction* transaction) {
  TestLifetimeDelete(transaction);
}
extern "C" void ASurfaceTransaction_apply(ASurfaceTransaction* opaque) {
  ++g_apply_count;
  if (g_complete_on_apply) {
    darwin_art::window::SurfaceTransactionStats stats;
    auto* transaction =
        reinterpret_cast<darwin_art::window::SurfaceTransaction*>(opaque);
    {
      std::lock_guard<std::mutex> lock(g_latch_mutex);
      for (const auto& update : transaction->updates) {
        if (!update.has_buffer) continue;
        const uint64_t identity =
            (static_cast<uint64_t>(update.opaque->owner_process) << 32) |
            update.opaque->layer;
        stats.controls.push_back(update.opaque);
        auto found = g_latched_buffers.find(identity);
        if (found != g_latched_buffers.end()) {
          AHardwareBuffer_acquire(found->second);
          stats.previous_buffers.emplace(update.opaque, found->second);
          stats.previous_submission_cookies.emplace(
              update.opaque, g_latched_submission_cookies.at(identity));
          AHardwareBuffer_release(found->second);
        }
        AHardwareBuffer_acquire(update.buffer);
        g_latched_buffers[identity] = update.buffer;
        g_latched_submission_cookies[identity] = update.submission_cookie;
      }
    }
    if (g_completion_allocation_fails) g_fail_next_allocation = true;
    darwin_art::window::CompleteSurfaceTransaction(
        transaction, &stats);
    g_fail_next_allocation = false;
  }
}
extern "C" void darwin_art_android_surface_transaction_set_buffer_callbacks(
    void*, void*, void*, void (*)(void*, ASurfaceTransactionStats*),
    void (*)(void*, int)) {
  ++g_callback_registration_count;
}
extern "C" bool darwin_art_android_surface_transaction_set_buffer_callbacks_checked(
    void* transaction, void* control, void* context,
    void (*complete)(void*, ASurfaceTransactionStats*),
    void (*discard)(void*, int)) {
  ++g_callback_registration_count;
  if (g_registration_pause != nullptr) {
    g_registration_pause->entered.store(true);
    while (!g_registration_pause->release.load()) std::this_thread::yield();
    // This admission fails after a successor has completed. The original
    // acquire fence must still protect this unsubmitted frame's producer work.
    return false;
  }
  if (g_registration_fails) g_fail_next_allocation = true;
  return TestLifetimeCallbacksChecked(transaction, control, context, complete,
                                     discard);
}

int main() {
  {
    // A SurfaceTexture parcel has producer geometry but no SurfaceControl.
    // The remote Vulkan WSI must see that geometry without inventing a layer.
    imported_surface_identity = 0x4441534600000041ll;
    darwin_art_android_ANativeWindow_register_imported_surface_identity(
        imported_surface_identity, 0, 0, 720, 1280, 1);
    JNINativeInterface table{};
    table.GetObjectClass = SurfaceGetObjectClass;
    table.GetFieldID = SurfaceGetFieldID;
    table.GetObjectField = SurfaceGetObjectField;
    table.GetLongField = SurfaceGetLongField;
    table.MonitorEnter = SurfaceMonitorEnter;
    table.MonitorExit = SurfaceMonitorExit;
    table.ExceptionCheck = SurfaceExceptionCheck;
    table.DeleteLocalRef = SurfaceDeleteLocalRef;
    JNIEnv env{&table};
    void* imported = darwin_art_android_ANativeWindow_fromSurface(
        &env, reinterpret_cast<void*>(0x64));
    assert(imported != nullptr);
    assert(darwin_art_android_ANativeWindow_getWidth(imported) == 720);
    assert(darwin_art_android_ANativeWindow_getHeight(imported) == 1280);
    uint32_t owner = 99;
    uint32_t layer = 99;
    assert(!darwin_art_android_ANativeWindow_get_imported_surface_identity(
        imported, &owner, &layer));
    assert(owner == 99 && layer == 99);
    darwin_art_android_ANativeWindow_release(imported);
  }
  {
    // Surface parcel metadata is registered before the producer facade is
    // created.  The real fromSurface path must republish those dimensions
    // into the queue, then expose matching AHardwareBuffer dimensions on the
    // first dequeue.
    imported_surface_identity = 0x4441534600000042ll;
    darwin_art_android_ANativeWindow_register_imported_surface_identity(
        imported_surface_identity, 17, 23, 16, 8, 1);
    JNINativeInterface table{};
    table.GetObjectClass = SurfaceGetObjectClass;
    table.GetFieldID = SurfaceGetFieldID;
    table.GetObjectField = SurfaceGetObjectField;
    table.GetLongField = SurfaceGetLongField;
    table.MonitorEnter = SurfaceMonitorEnter;
    table.MonitorExit = SurfaceMonitorExit;
    table.ExceptionCheck = SurfaceExceptionCheck;
    table.DeleteLocalRef = SurfaceDeleteLocalRef;
    JNIEnv env{&table};
    // Fail each allocation through publication, not just the first owner
    // allocation. Every unsuccessful creation leaves no registered facade.
    const auto registry_size = g_android_native_windows_by_surface.size();
    int creation_failures = 0;
    for (int failure = 0; failure < 16; ++failure) {
      g_countdown_failure_triggered = false;
      g_allocation_failure_after = failure;
      void* candidate = darwin_art_android_ANativeWindow_fromSurface(
          &env, reinterpret_cast<void*>(0x65));
      const bool injected = g_countdown_failure_triggered;
      g_allocation_failure_after = -1;
      if (injected) {
        ++creation_failures;
        assert(candidate == nullptr);
        assert(g_android_native_windows_by_surface.size() == registry_size);
        assert(!g_android_native_windows_by_surface.contains(imported_surface_identity));
      } else {
        assert(candidate != nullptr);
        darwin_art_android_ANativeWindow_release(candidate);
        assert(g_android_native_windows_by_surface.size() == registry_size);
        break;
      }
    }
    assert(creation_failures >= 4);
    g_fail_next_allocation = true;
    assert(darwin_art_android_ANativeWindow_fromSurface(
               &env, reinterpret_cast<void*>(0x65)) == nullptr);
    assert(!g_fail_next_allocation);
    void* imported = darwin_art_android_ANativeWindow_fromSurface(
        &env, reinterpret_cast<void*>(0x65));
    assert(imported != nullptr);
    uint32_t owner = 0;
    uint32_t layer = 0;
    assert(darwin_art_android_ANativeWindow_get_imported_surface_identity(
        imported, &owner, &layer));
    assert(owner == 17 && layer == 23);
    assert(darwin_art_android_ANativeWindow_getWidth(imported) == 16);
    assert(darwin_art_android_ANativeWindow_getHeight(imported) == 8);
    assert(darwin_art_android_ANativeWindow_getFormat(imported) == 1);
    // Republish AFTER facade construction but BEFORE lazy buffer allocation.
    darwin_art_android_ANativeWindow_register_imported_surface_identity(
        imported_surface_identity, 17, 23, 32, 24, 1);
    assert(darwin_art_android_ANativeWindow_getWidth(imported) == 32);
    assert(darwin_art_android_ANativeWindow_getHeight(imported) == 24);
    AHardwareBuffer* imported_buffer = nullptr;
    void* imported_native = nullptr;
    int imported_fence = -1;
    assert(darwin_art_android_ANativeWindow_dequeue_hardware_buffer(
               imported, &imported_buffer, &imported_native,
               &imported_fence) == 0);
    assert(imported_buffer != nullptr && imported_native != nullptr);
    assert(imported_buffer->width == 32 && imported_buffer->height == 24);
    assert(darwin_art_android_ANativeWindow_cancel_hardware_buffer(
               imported, imported_native, imported_fence) == 0);
    darwin_art_android_ANativeWindow_register_imported_surface_identity(
        imported_surface_identity, 17, 23, 40, 20, 1);
    assert(darwin_art_android_ANativeWindow_getWidth(imported) == 40);
    assert(darwin_art_android_ANativeWindow_getHeight(imported) == 20);
    imported_buffer = nullptr;
    imported_native = nullptr;
    imported_fence = -1;
    assert(darwin_art_android_ANativeWindow_dequeue_hardware_buffer(
               imported, &imported_buffer, &imported_native,
               &imported_fence) == 0);
    assert(imported_buffer->width == 40 && imported_buffer->height == 20);
    assert(darwin_art_android_ANativeWindow_cancel_hardware_buffer(
               imported, imported_native, imported_fence) == 0);
    darwin_art_android_ANativeWindow_release(imported);
  }
  {
    void* opaque = darwin_art_android_ANativeWindow_create(16, 16, 1);
    auto* window = static_cast<DarwinAndroidNativeWindow*>(opaque);
    assert(window);
    // A plain SurfaceControl producer has no consumer callback capable of
    // returning displaced frames, so MAILBOX must remain unavailable.
    assert(!darwin_art_android_ANativeWindow_supports_mailbox(opaque));
    assert(darwin_art_android_ANativeWindow_set_present_mode(
               opaque, DARWIN_ART_ANDROID_PRESENT_MODE_MAILBOX) ==
           -ENOTSUP);
    AndroidNativeWindowBufferAbi output{};
    g_fail_next_allocation = true;
    assert(darwin_art_android_ANativeWindow_lock(opaque, &output, nullptr) ==
           -ENOMEM);
    assert(!g_fail_next_allocation && !window->locked && !window->published);
    assert(output.bits == nullptr);
    darwin_art_android_ANativeWindow_release(opaque);
  }
  {
    void* opaque = darwin_art_android_ANativeWindow_create(16, 16, 1);
    assert(opaque != nullptr);
    auto* window = static_cast<DarwinAndroidNativeWindow*>(opaque);
    AHardwareBuffer* buffer = nullptr;
    void* native = nullptr;
    int fence = -1;
    assert(darwin_art_android_ANativeWindow_dequeue_hardware_buffer(
               opaque, &buffer, &native, &fence) == 0);
    darwin_art::window::NativeWindowQueuedBuffer queued;
    assert(window->gpu_queue->Queue(native, &queued) == 0);
    const darwin_art::window::NativeWindowTransactionFrame frame{
        .slot = queued.token.slot,
        .generation = queued.token.generation,
        .frame = queued.token.frame};
    int closes = g_close_count;
    darwin_art_android_ANativeWindow_release_consumer_frame(
        opaque, queued.token.slot, queued.token.generation + 1,
        queued.token.frame, 60);
    assert(g_close_count == ++closes && !CanDequeueNative(window, native));
    ReturnNativeWindowFrame(window, {.slot = queued.token.slot,
                                    .generation = queued.token.generation,
                                    .frame = queued.token.frame - 1}, 61,
                            false);
    assert(g_close_count == ++closes && !CanDequeueNative(window, native));
    ReturnNativeWindowFrame(window, frame, -1, false);
    assert(CanDequeueNative(window, native));
    // Duplicate completion must not attach a late fence to a returned slot,
    // even when the first completion had no fence.
    darwin_art_android_ANativeWindow_release_consumer_frame(
        opaque, queued.token.slot, queued.token.generation, queued.token.frame,
        62);
    assert(g_close_count == ++closes && CanDequeueNative(window, native));
    ReturnNativeWindowFrame(window, frame, 63, true);
    assert(g_close_count == ++closes && CanDequeueNative(window, native));
    darwin_art_android_ANativeWindow_release(opaque);
  }
  {
    void* rejected = darwin_art_android_ANativeWindow_create(16, 16, 1);
    assert(rejected != nullptr);
    AHardwareBuffer* buffer = nullptr;
    void* native = nullptr;
    int fence = -1;
    assert(darwin_art_android_ANativeWindow_dequeue_hardware_buffer(
               rejected, &buffer, &native, &fence) == 0);
    const int closes = g_close_count;
    g_create_transaction_fails = true;
    assert(darwin_art_android_ANativeWindow_queue_hardware_buffer(
               rejected, native, 40) == -ENOMEM);
    g_create_transaction_fails = false;
    auto* window = static_cast<DarwinAndroidNativeWindow*>(rejected);
    assert(!CanDequeueNative(window, native));
    assert(g_close_count == closes);
    assert(sync_wait(1040, 0) != 0);
    void* next = nullptr;
    assert(darwin_art_android_ANativeWindow_dequeue_hardware_buffer(
               rejected, &buffer, &next, &fence) == 0);
    assert(next != native);
    assert(g_callback_registration_count == 0 && g_apply_count == 0);
    darwin_art_android_ANativeWindow_release(rejected);
  }
  void* opaque = darwin_art_android_ANativeWindow_create(16, 16, 1);
  assert(opaque != nullptr);
  auto* window = static_cast<DarwinAndroidNativeWindow*>(opaque);
  AHardwareBuffer* buffer = nullptr;
  void* native_buffer = nullptr;
  int fence = -1;
  assert(darwin_art_android_ANativeWindow_dequeue_hardware_buffer(
             opaque, &buffer, &native_buffer, &fence) == 0);
  assert(buffer != nullptr && native_buffer != nullptr);

  const int closes_before = g_close_count;
  g_dup_fails = false;
  assert(darwin_art_android_ANativeWindow_queue_hardware_buffer(
             opaque, native_buffer, 41) == -ENOMEM);
  assert(g_callback_registration_count == 0 && g_apply_count == 0);
  assert(!CanDequeueNative(window, native_buffer));
  assert(g_close_count == closes_before + 1);  // original producer fence

  // The unsignaled rollback fence blocks this slot; it becomes available only
  // after the fence signal is observed.
  assert(!g_signal_fence);
  assert(sync_wait(1041, 0) != 0);
  void* other_native = nullptr;
  int other_fence = -1;
  AHardwareBuffer* other_buffer = nullptr;
  assert(darwin_art_android_ANativeWindow_dequeue_hardware_buffer(
             opaque, &other_buffer, &other_native, &other_fence) == 0);
  assert(other_native != native_buffer);
  assert(darwin_art_android_ANativeWindow_cancel_hardware_buffer(
             opaque, other_native, -1) == 0);
  g_signal_fence = true;
  assert(sync_wait(1041, 0) == 0);
  void* signaled_native = nullptr;
  int signaled_fence = -1;
  AHardwareBuffer* signaled_buffer = nullptr;
  for (int attempt = 0; attempt != 3; ++attempt) {
    assert(darwin_art_android_ANativeWindow_dequeue_hardware_buffer(
               opaque, &signaled_buffer, &signaled_native, &signaled_fence) ==
           0);
    if (signaled_native == native_buffer) break;
    assert(darwin_art_android_ANativeWindow_cancel_hardware_buffer(
               opaque, signaled_native, -1) == 0);
  }
  assert(signaled_native == native_buffer);
  assert(darwin_art_android_ANativeWindow_cancel_hardware_buffer(
             opaque, signaled_native, -1) == 0);

  // A duplicate failure quarantines the held slot rather than releasing it
  // with an unverifiable fence.
  void* second_native = nullptr;
  int second_fence = -1;
  AHardwareBuffer* second_buffer = nullptr;
  assert(darwin_art_android_ANativeWindow_dequeue_hardware_buffer(
             opaque, &second_buffer, &second_native, &second_fence) == 0);
  g_dup_fails = true;
  assert(darwin_art_android_ANativeWindow_queue_hardware_buffer(
             opaque, second_native, 42) == -ENOMEM);
  assert(!CanDequeueNative(window, second_native));
  assert(g_callback_registration_count == 0 && g_apply_count == 0);
  darwin_art_android_ANativeWindow_release(opaque);
  g_dup_fails = false;
  g_builder_fails = false;
  g_complete_on_apply = true;
  {
    void* successful = darwin_art_android_ANativeWindow_create(16, 16, 1);
    auto* producer = static_cast<DarwinAndroidNativeWindow*>(successful);
    assert(producer != nullptr);
    auto submit = [successful](int fence) {
      AHardwareBuffer* buffer = nullptr;
      void* native = nullptr;
      int release_fence = -1;
      assert(darwin_art_android_ANativeWindow_dequeue_hardware_buffer(
                 successful, &buffer, &native, &release_fence) == 0);
      assert(darwin_art_android_ANativeWindow_queue_hardware_buffer(
                 successful, native, fence) == 0);
      return native;
    };
    void* first_native = submit(50);
    assert(!CanDequeueNative(producer, first_native));
    // Completion has exactly one predecessor to retire; memory pressure must
    // not lose that sole release opportunity after the latch already happened.
    g_completion_allocation_fails = true;
    void* second_native = submit(51);
    g_completion_allocation_fails = false;
    assert(second_native != first_native);
    assert(CanDequeueNative(producer, first_native));
    assert(!CanDequeueNative(producer, second_native));
    const uint64_t previous_generation = producer->gpu_queue->Generation();
    assert(darwin_art_android_ANativeWindow_setBuffersGeometry(
        successful, 24, 24, 1) == 0);
    assert(producer->gpu_queue->Generation() != previous_generation);
    assert(!CanDequeueNative(producer, second_native));
    void* resized_native = submit(55);
    // The predecessor was returned, then retired by the geometry change; the
    // public queue correctly keeps retired generations out of dequeue.
    assert(!CanDequeueNative(producer, second_native));
    assert(!CanDequeueNative(producer, resized_native));
    auto* alias = new ASurfaceControl();
    alias->owner_process = producer->surface_control->owner_process;
    alias->layer = producer->surface_control->layer;
    darwin_art_android_ANativeWindow_set_surface_control(successful, alias);
    ASurfaceControl_release(alias);
    void* alias_native = submit(56);
    // Distinct wrappers for one registry layer must share predecessor policy.
    assert(CanDequeueNative(producer, resized_native));
    assert(!CanDequeueNative(producer, alias_native));
    auto* different = static_cast<ASurfaceControl*>(
        darwin_art_android_surface_control_create_root("different-layer"));
    darwin_art_android_ANativeWindow_set_surface_control(successful, different);
    ASurfaceControl_release(different);
    (void)submit(57);
    // A different layer completion cannot release the old layer's resident.
    assert(!CanDequeueNative(producer, alias_native));
    assert(producer->references.load() == 1);  // no resident producer cycle
    darwin_art_android_ANativeWindow_release(successful);
  }
  {
    void* rejected = darwin_art_android_ANativeWindow_create(16, 16, 1);
    auto* producer = static_cast<DarwinAndroidNativeWindow*>(rejected);
    AHardwareBuffer* buffer = nullptr;
    void* native = nullptr;
    int fence = -1;
    assert(darwin_art_android_ANativeWindow_dequeue_hardware_buffer(
               rejected, &buffer, &native, &fence) == 0);
    const int applies = g_apply_count;
    g_registration_fails = true;
    assert(darwin_art_android_ANativeWindow_queue_hardware_buffer(
               rejected, native, 52) == -ENOMEM);
    g_registration_fails = false;
    assert(g_apply_count == applies);
    assert(CanDequeueNative(producer, native));
    assert(sync_wait(1052, 0) == 0);
    assert(producer->references.load() == 1);
    darwin_art_android_ANativeWindow_release(rejected);
  }
  {
    void* transferred = darwin_art_android_ANativeWindow_create(16, 16, 1);
    auto* producer = static_cast<DarwinAndroidNativeWindow*>(transferred);
    int observer_calls = 0;
    auto observer = [](void* context, void* transaction, uint64_t) -> bool {
      ++*static_cast<int*>(context);
      // BLAST owns and may synchronously delete the accepted transaction.
      ASurfaceTransaction_delete(static_cast<ASurfaceTransaction*>(transaction));
      return true;
    };
    assert(darwin_art_android_ANativeWindow_set_transaction_callback(
        transferred, observer, &observer_calls, nullptr));
    AHardwareBuffer* buffer = nullptr;
    void* native = nullptr;
    int fence = -1;
    assert(darwin_art_android_ANativeWindow_dequeue_hardware_buffer(
               transferred, &buffer, &native, &fence) == 0);
    const int applies = g_apply_count;
    assert(darwin_art_android_ANativeWindow_queue_hardware_buffer(
               transferred, native, 53) == 0);
    assert(observer_calls == 1 && g_apply_count == applies);
    assert(CanDequeueNative(producer, native));
    assert(sync_wait(1053, 0) == 0);
    assert(producer->references.load() == 1);
    darwin_art_android_ANativeWindow_release(transferred);
  }
  {
    struct ObserverContext {
      void* producer;
      int calls = 0;
      int releases = 0;
    } context{darwin_art_android_ANativeWindow_create(16, 16, 1)};
    void* producer = context.producer;
    auto observer = [](void* opaque, void*, uint64_t) -> bool {
      auto* context = static_cast<ObserverContext*>(opaque);
      ++context->calls;
      void* producer = std::exchange(context->producer, nullptr);
      darwin_art_android_ANativeWindow_release(producer);
      return false;
    };
    auto release = [](void* opaque) {
      ++static_cast<ObserverContext*>(opaque)->releases;
    };
    assert(darwin_art_android_ANativeWindow_set_transaction_callback(
        producer, observer, &context, release));
    AHardwareBuffer* buffer = nullptr;
    void* native = nullptr;
    int fence = -1;
    assert(darwin_art_android_ANativeWindow_dequeue_hardware_buffer(
               producer, &buffer, &native, &fence) == 0);
    // Completion runs synchronously, potentially releasing the last callback
    // pin. Submit must keep its own lease until transaction cleanup finishes.
    assert(darwin_art_android_ANativeWindow_queue_hardware_buffer(
               producer, native, 54) == 0);
    assert(context.producer == nullptr && context.calls == 1);
    assert(context.releases == 1);
  }
  {
    void* failed = darwin_art_android_ANativeWindow_create(16, 16, 1);
    auto* producer = static_cast<DarwinAndroidNativeWindow*>(failed);
    auto* control = static_cast<ASurfaceControl*>(
        darwin_art_android_surface_control_create_root("retention-oom"));
    darwin_art_android_ANativeWindow_set_surface_control(failed, control);
    assert(control->references == 2);  // window plus the test's reference
    AHardwareBuffer* buffer = nullptr;
    void* native = nullptr;
    int fence = -1;
    assert(darwin_art_android_ANativeWindow_dequeue_hardware_buffer(
               failed, &buffer, &native, &fence) == 0);
    g_control_retention_fails = true;
    assert(darwin_art_android_ANativeWindow_queue_hardware_buffer(
               failed, native, 58) == -ENOMEM);
    g_control_retention_fails = false;
    assert(!g_fail_next_allocation);
    assert(control->references == 2);
    assert(CanDequeueNative(producer, native));
    assert(sync_wait(58, 0) == 0);
    assert(producer->references.load() == 1);
    darwin_art_android_ANativeWindow_release(failed);
    assert(control->references == 1);
    ASurfaceControl_release(control);
  }
  {
    void* concurrent = darwin_art_android_ANativeWindow_create(16, 16, 1);
    auto* producer = static_cast<DarwinAndroidNativeWindow*>(concurrent);
    AHardwareBuffer* a_buffer = nullptr;
    void* a_native = nullptr;
    int fence = -1;
    assert(darwin_art_android_ANativeWindow_dequeue_hardware_buffer(
        concurrent, &a_buffer, &a_native, &fence) == 0);
    RegistrationPause pause;
    g_signal_fence = false;
    std::thread a([&] {
      g_registration_pause = &pause;
      assert(darwin_art_android_ANativeWindow_queue_hardware_buffer(
          concurrent, a_native, 60) == -ENOMEM);
      g_registration_pause = nullptr;
    });
    while (!pause.entered.load()) std::this_thread::yield();
    AHardwareBuffer* b_buffer = nullptr;
    void* b_native = nullptr;
    assert(darwin_art_android_ANativeWindow_dequeue_hardware_buffer(
        concurrent, &b_buffer, &b_native, &fence) == 0);
    assert(b_native != a_native);
    assert(darwin_art_android_ANativeWindow_queue_hardware_buffer(
        concurrent, b_native, 61) == 0);
    assert(!CanDequeueNative(producer, a_native));
    assert(!CanDequeueNative(producer, b_native));
    pause.release.store(true);
    a.join();
    // Once the fence is signaled, dequeue proves the failed admission
    // returned A rather than leaving it consumer-held. Leave it dequeued so
    // the following unsignaled fence check still models producer reuse.
    g_signal_fence = true;
    assert(CanDequeueNative(producer, a_native, false));
    g_signal_fence = false;
    assert(!CanDequeueNative(producer, b_native));
    assert(sync_wait(1060, 0) != 0);
    assert(sync_wait(1060, 0) != 0);
    AHardwareBuffer* c_buffer = nullptr;
    void* c_native = nullptr;
    assert(darwin_art_android_ANativeWindow_dequeue_hardware_buffer(
        concurrent, &c_buffer, &c_native, &fence) == 0);
    assert(c_native != a_native);
    assert(darwin_art_android_ANativeWindow_cancel_hardware_buffer(
        concurrent, c_native, -1) == 0);
    darwin_art_android_ANativeWindow_release(concurrent);
  }
  {
    void* concurrent = darwin_art_android_ANativeWindow_create(16, 16, 1);
    auto* producer = static_cast<DarwinAndroidNativeWindow*>(concurrent);
    auto observer = [](void*, void*, uint64_t) -> bool {
      if (g_observer_pause != nullptr) {
        g_observer_pause->entered.store(true);
        while (!g_observer_pause->release.load()) std::this_thread::yield();
      }
      return false;
    };
    assert(darwin_art_android_ANativeWindow_set_transaction_callback(
        concurrent, observer, nullptr, nullptr));
    AHardwareBuffer* a_buffer = nullptr;
    void* a_native = nullptr;
    int fence = -1;
    assert(darwin_art_android_ANativeWindow_dequeue_hardware_buffer(
        concurrent, &a_buffer, &a_native, &fence) == 0);
    RegistrationPause pause;
    std::thread a([&] {
      g_observer_pause = &pause;
      assert(darwin_art_android_ANativeWindow_queue_hardware_buffer(
          concurrent, a_native, 62) == 0);
      g_observer_pause = nullptr;
    });
    while (!pause.entered.load()) std::this_thread::yield();
    AHardwareBuffer* b_buffer = nullptr;
    void* b_native = nullptr;
    assert(darwin_art_android_ANativeWindow_dequeue_hardware_buffer(
        concurrent, &b_buffer, &b_native, &fence) == 0);
    assert(darwin_art_android_ANativeWindow_queue_hardware_buffer(
        concurrent, b_native, 63) == 0);
    // Callback-armed A has not reached the compositor. B cannot return it.
    assert(!CanDequeueNative(producer, a_native));
    assert(!CanDequeueNative(producer, b_native));
    pause.release.store(true);
    a.join();
    // Actual latch order is B then A, opposite their queued frame serials.
    assert(!CanDequeueNative(producer, a_native));
    assert(CanDequeueNative(producer, b_native));
    assert(producer->references.load() == 1);
    darwin_art_android_ANativeWindow_release(concurrent);
  }
  for (const auto& [identity, buffer] : g_latched_buffers) {
    (void)identity;
    AHardwareBuffer_release(buffer);
  }
  g_latched_buffers.clear();
  g_latched_submission_cookies.clear();
  std::puts("native-window transaction admission: PASS");
}
