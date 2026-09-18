#pragma once

#include "graphics/hardware_buffer_owner.h"

#include <cstddef>
#include <cstdint>

// Exact, closed libandroid.so provider used by the Android ELF resolver.
// Returns null for every symbol not implemented by this compatibility layer.
extern "C" void* darwin_art_android_platform_symbol(const char* symbol);

// Drains callbacks registered on the current thread's Android native ALooper
// without blocking. The detached host calls this at the same boundary where
// Android MessageQueue.nativePollOnce would normally service ALooper fds.
extern "C" int darwin_art_android_platform_poll_current_looper();

// Android's Java MessageQueue and native ALooper are two views of one poll
// loop. These helpers let the framework JNI bridge share that loop without
// exposing the private host-side ALooper representation.
extern "C" void* darwin_art_android_platform_prepare_current_looper();
extern "C" int darwin_art_android_platform_poll_current_looper_timeout(
    int timeout_ms);
// Waits for one owner-Looper wake/native callback for at most timeout_ms.
// The result is normalized to zero for timeout/wake/callback and -1 only for
// a poll error, so host scheduling can use it without exposing ALooper codes.
extern "C" int darwin_art_android_platform_wait_current_looper(
    int timeout_ms);
extern "C" void darwin_art_android_platform_wake_looper(void* looper);

// Internal InputChannel transport hooks. The opaque Looper handle remains
// owned by the ART owner thread; these wrappers keep the public APK ABI
// independent from the compatibility layer's private ALooper type.
using DarwinArtLooperCallback = int (*)(int, int, void*);
using DarwinArtLooperOwnerRelease = void (*)(void*);
extern "C" int darwin_art_android_platform_add_fd(
    void* looper, int fd, int ident, int events,
    DarwinArtLooperCallback callback, void* data);
// Takes ownership of `owner` on every return path. A successful registration
// retains it through every in-flight poll snapshot, then invokes `release`
// after the last snapshot or registration drops it.
extern "C" int darwin_art_android_platform_add_fd_owned(
    void* looper, int fd, int ident, int events,
    DarwinArtLooperCallback callback, void* data, void* owner,
    DarwinArtLooperOwnerRelease release);
extern "C" int darwin_art_android_platform_remove_fd(void* looper, int fd);

// SCM_RIGHTS transports the Darwin file description but not Android ashmem's
// region-wide protection metadata. Binder carries this sidecar between app
// processes and re-adopts the received descriptor into the local provider.
extern "C" int darwin_art_android_shared_memory_get_info(
    int fd, size_t* size, int* protection);
extern "C" int darwin_art_android_shared_memory_adopt(
    int fd, size_t size, int protection);

// Creates a Metal shared event on the supplied MTLDevice and reserves the
// next monotonically increasing signal value. The returned event is retained
// until release. A fence descriptor created from it becomes readable only
// when ANGLE/Metal signals that exact value.
extern "C" void* darwin_art_android_metal_shared_event_create(
    void* metal_device, uint64_t* signal_value);
extern "C" int darwin_art_android_metal_shared_event_fence_fd(
    void* shared_event, uint64_t signal_value);
// Returns the next unsignaled Metal event value. The caller uses this value
// for one Vulkan binary-semaphore operation.
extern "C" uint64_t darwin_art_android_metal_shared_event_next_value(
    void* shared_event);
// Consumes an Android sync fence descriptor without blocking the Vulkan
// caller. The shared event is signaled from a background waiter after the
// fence becomes readable, allowing MoltenVK to keep the wait on the GPU.
extern "C" int darwin_art_android_metal_shared_event_import_fence(
    void* shared_event, uint64_t signal_value, int fence_fd);
extern "C" void darwin_art_android_metal_shared_event_release(
    void* shared_event);
// Framework SurfaceControl JNI and NDK libandroid clients share one retained
// compositor graph. These entry points create the display-root identity and
// reset a reusable transaction without exposing the private implementation.
extern "C" void* darwin_art_android_surface_control_create_root(
    const char* name);
// Reconstructs the process-local client object for a SurfaceControl whose
// authoritative layer is owned by another Android process. The pair is the
// compositor capability carried by SurfaceControl's framework Parcel JNI;
// transactions retain it unchanged when they cross into SurfaceFlinger.
extern "C" void* darwin_art_android_surface_control_create_imported(
    uint32_t owner_process_id, uint32_t layer_id, const char* name);
extern "C" bool darwin_art_android_surface_control_get_identity(
    void* control, uint32_t* owner_process_id, uint32_t* layer_id);
extern "C" void darwin_art_android_surface_transaction_clear(
    void* transaction);
extern "C" void darwin_art_android_surface_transaction_merge(
    void* destination, void* source);
// Internal two-phase merge hook. The caller supplies an empty operation-local
// disposal transaction and destroys it only after leaving any state mutex.
// Structural merge performs no resource release or callback invocation.
extern "C" bool darwin_art_android_surface_transaction_merge_deferred(
    void* destination, void* source, void* disposal);
// Internal ownership hook: runs only if an unapplied transaction is cleared
// or deleted. A successful apply consumes it without invoking the callback.
extern "C" void darwin_art_android_surface_transaction_set_on_discard(
    void* transaction, void* context, void (*callback)(void*));
// Stores the bounded transparent-region hint carried by a SurfaceControl
// transaction. `rects` is a flattened [left, top, right, bottom] array.
extern "C" void darwin_art_android_surface_transaction_set_transparent_region_hint(
    void* transaction, void* control, const int32_t* rects, size_t count);
// Producer-side accessor for the compositor flattening stage. Passing null
// output with zero capacity queries the number of stored rectangles.
extern "C" size_t darwin_art_android_surface_control_copy_transparent_region(
    void* control, int32_t* rects, size_t capacity);
struct ASurfaceTransactionStats;
struct AHardwareBuffer;
struct ASurfaceControl;
extern "C" bool ASurfaceTransactionStats_getPreviousBufferMetadata(
    ASurfaceTransactionStats* stats, ASurfaceControl* control,
    AHardwareBuffer** buffer, uint64_t* submission_cookie);
extern "C" bool
darwin_art_android_surface_transaction_set_buffer_with_cookie_checked(
    void* transaction, void* control, AHardwareBuffer* buffer, int fence_fd,
    uint64_t submission_cookie);
// discard owns its fence; -1 is ready, -2 requires quarantine after fence failure.
// A false checked result transfers no callback-context ownership.
extern "C" bool darwin_art_android_surface_transaction_set_buffer_callbacks_checked(
    void* transaction, void* control, void* context,
    void (*complete)(void*, ASurfaceTransactionStats*),
    void (*discard)(void*, int));
extern "C" void darwin_art_android_surface_transaction_set_buffer_callbacks(
    void* transaction, void* control, void* context,
    void (*complete)(void*, ASurfaceTransactionStats*),
    void (*discard)(void*, int));
extern "C" void darwin_art_android_surface_transaction_set_relative_layer(
    void* transaction, void* control, void* relative_to, int32_t z);
