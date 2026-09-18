#pragma once

#include <jni.h>

#include <cstdint>

#include "darwin_android_native_window.h"
#include "surfaceflinger/commit_receipt.h"

namespace darwin_art {

// Registers the Android EGL10 facade and the small GLES20 bootstrap surface
// used by GLSurfaceView. The implementation delegates to ANGLE's Darwin
// dylibs and keeps Android's Java handle objects as the public ABI.
bool RegisterDarwinAngleEglNatives(JNIEnv* env);
const char* EglQueryStringAndroid(void* display, std::int32_t name);

// Standard EGL entry points and JNI EGLImpl use the same window backend
// lifecycle owner. The platform dispatch TU forwards its C ABI wrappers here;
// it must not create a second display/window registry.
std::uint32_t EglInitializeHost(void* display, std::int32_t* major,
                                std::int32_t* minor);
std::uint32_t TerminateHostDisplay(void* display);

extern "C" void* darwin_art_angle_dso_symbol(const char* soname,
                                              const char* symbol);

// Receives generic SurfaceView window geometry from the framework bridge.
// This is intentionally independent of any APK class.
void ConfigureDarwinAngleHostSurface(jint x, jint y, jint width, jint height);
void ConfigureDarwinAngleDisplayTarget(jint width, jint height);
jint DarwinAngleHostSurfaceWidth();
jint DarwinAngleHostSurfaceHeight();

}  // namespace darwin_art

struct AHardwareBuffer;

extern "C" {
void* darwin_art_android_ANativeWindow_fromSurface(void* env, void* surface);
void* darwin_art_android_ANativeWindow_create(int32_t width, int32_t height,
                                               int32_t format);
void darwin_art_android_ANativeWindow_acquire(void* window);
int32_t darwin_art_android_ANativeWindow_getFormat(void* window);
int32_t darwin_art_android_ANativeWindow_getWidth(void* window);
int32_t darwin_art_android_ANativeWindow_getHeight(void* window);
void* darwin_art_android_ANativeWindow_toSurface(void* env, void* window);
void darwin_art_android_ANativeWindow_release(void* window);
using DarwinArtAndroidNativeWindowQueueCallback = void (*)(
    void* context, AHardwareBuffer* buffer, int32_t slot, int fence,
    int32_t dataspace);
void darwin_art_android_ANativeWindow_set_queue_callback(
    void* window, DarwinArtAndroidNativeWindowQueueCallback callback,
    void* context);
// Installs an owned consumer callback. Replacement/unregistration stops new
// captures without waiting; already captured observers retain their context.
// release_context runs after the last capture retires, outside the producer
// mutex. Callers must retain the producer if their cleanup reenters it.
// This mirrors BufferQueue's strong listener ownership and prevents a producer
// queue racing Java consumer teardown from dereferencing a freed context.
bool darwin_art_android_ANativeWindow_set_owned_queue_callback(
    void* window, DarwinArtAndroidNativeWindowQueueCallback callback,
    void* context, void (*release_context)(void*));
// BLAST receives the actual buffer transaction before it is applied. Returning
// true transfers transaction ownership to the callback. Registration context
// is owned only on success and released after replacement and in-flight calls.
using DarwinArtAndroidNativeWindowTransactionCallback = bool (*)(
    void* context, void* transaction, uint64_t frame_number);
bool darwin_art_android_ANativeWindow_set_transaction_callback(
    void* window, DarwinArtAndroidNativeWindowTransactionCallback callback,
    void* context, void (*release_context)(void*));
uint64_t darwin_art_android_ANativeWindow_next_frame_number(void* window);
void darwin_art_android_ANativeWindow_release_consumer_slot(
    void* window, int32_t slot, int release_fence);
// Generation/frame-validated return endpoint used by the SurfaceControl
// fallback. The endpoint consumes fence and rejects stale identities without
// touching a newer slot occupant.
void darwin_art_android_ANativeWindow_release_consumer_frame(
    void* window, int32_t slot, uint64_t generation, uint64_t frame,
    int release_fence);
void darwin_art_android_ANativeWindow_set_surface_control(void* window,
                                                          void* control);
bool darwin_art_android_ANativeWindow_get_surface_control_identity(
    void* window, uint32_t* owner_process_id, uint32_t* layer_id);
void darwin_art_android_ANativeWindow_register_imported_surface_identity(
    int64_t surface_identity, uint32_t owner_process_id, uint32_t layer_id,
    int32_t width, int32_t height, int32_t format);
bool darwin_art_android_ANativeWindow_get_imported_surface_identity(
    void* window, uint32_t* owner_process_id, uint32_t* layer_id);
bool darwin_art_android_ANativeWindow_release_if_managed(void* window);
// Returns true only while the pointer is present in the native-window
// registry. Callers that need to use the handle after this check must acquire
// their own native-window reference.
bool darwin_art_android_ANativeWindow_is_managed(void* window);
// Forces a bounded 3-slot generation transition, including when dimensions
// are unchanged. Returns -EBUSY while a borrowed/dequeued slot or prior
// retired generation cannot be safely preserved.
int32_t darwin_art_android_ANativeWindow_prepare_swapchain(
    void* window, int32_t width, int32_t height);
int32_t darwin_art_android_ANativeWindow_dequeue_hardware_buffer(
    void* window, AHardwareBuffer** buffer, void** native_buffer, int* fence);
int32_t darwin_art_android_ANativeWindow_queue_hardware_buffer(
    void* window, void* native_buffer, int fence);
int32_t darwin_art_android_ANativeWindow_cancel_hardware_buffer(
    void* window, void* native_buffer, int fence);
int32_t darwin_art_android_ANativeWindow_lock(void* window, void* buffer,
                                              void* dirty_bounds);
int32_t darwin_art_android_ANativeWindow_unlockAndPost(void* window);
int32_t darwin_art_android_ANativeWindow_setBuffersGeometry(
    void* window, int32_t width, int32_t height, int32_t format);

void* darwin_art_android_eglCreateWindowSurface(void* display, void* config,
                                                void* window,
                                                const int32_t* attributes);
int32_t darwin_art_android_eglGetError();
uint32_t darwin_art_android_eglSwapBuffers(void* display, void* surface);
uint32_t darwin_art_android_eglDestroySurface(void* display, void* surface);
uint32_t darwin_art_android_eglMakeCurrent(void* display, void* draw,
                                          void* read, void* context);
uint32_t darwin_art_android_eglQuerySurface(void* display, void* surface,
                                           int32_t attribute, int32_t* value);
uint32_t darwin_art_android_eglSurfaceAttrib(void* display, void* surface,
                                            int32_t attribute, int32_t value);
uint32_t darwin_art_android_eglSwapInterval(void* display, int32_t interval);
uint32_t darwin_art_android_eglSetDamageRegion(void* display, void* surface,
                                              const int32_t* rects,
                                              int32_t count);

// Debug-only GLES forwarding hooks selected by the ELF resolver when
// DARWIN_ART_DEBUG_ANGLE is enabled. They preserve the Android GLES ABI while
// exposing whether a native renderer reaches ANGLE's texture/draw stages.
void darwin_art_android_glTexImage2D(uint32_t target, int32_t level,
                                    int32_t internal_format, int32_t width,
                                    int32_t height, int32_t border,
                                    uint32_t format, uint32_t type,
                                    const void* pixels);
void darwin_art_android_glTexSubImage2D(uint32_t target, int32_t level,
                                       int32_t x, int32_t y, int32_t width,
                                       int32_t height, uint32_t format,
                                       uint32_t type, const void* pixels);
void darwin_art_android_glDrawArrays(uint32_t mode, int32_t first,
                                     int32_t count);
void darwin_art_android_glDrawElements(uint32_t mode, int32_t count,
                                       uint32_t type, const void* indices);
void darwin_art_android_glUseProgram(uint32_t program);
bool darwin_art_android_begin_hardware_buffer_composition(void* buffer,
                                                          bool clear,
                                                          uint64_t transaction_id);
// False is a terminal restoration/nested-turn failure, not fallback permission.
bool darwin_art_android_begin_hardware_buffer_composition_checked(
    void* buffer, bool clear, uint64_t transaction_id, bool* started);
void darwin_art_android_set_hardware_buffer_composition_active(bool active);
// Finishes the current retained-layer submission and returns an Android-owned
// fence descriptor that becomes readable when ANGLE's Metal queue completes.
// The caller owns the descriptor. Returns -1 when no GPU work was submitted.
int darwin_art_android_end_hardware_buffer_composition();
// False is a terminal EGL restoration failure, potentially after publication;
// it must not be retried as an unsubmitted transaction. On true, -1 in the
// output is lossy; callers must never retry a publication based on it alone.
bool darwin_art_android_end_hardware_buffer_composition_checked(int* present_fence);
// Restoration health is orthogonal to the exact owned commit receipt.
bool darwin_art_android_end_hardware_buffer_composition_receipt(
    DarwinArtSurfaceFlingerReceipt* receipt);
// Marks the exact BufferQueue slot displaced by a SurfaceControl transaction.
// Its persistent IOSurface remains canonical until the producer reacquires the
// slot; the next draw-FBO bind restores it into ANGLE's 2D staging texture.
void darwin_art_android_mark_hardware_buffer_released(void* buffer);
void darwin_art_android_present_hardware_buffer(
    void* queue, uint32_t owner_process_id, uint32_t layer_id,
    uint32_t parent_owner_process_id, uint32_t parent_id, uint64_t what,
    uint32_t relative_parent_owner_process_id, uint32_t relative_parent_id,
    int32_t z, void* buffer, uint32_t transform, int32_t source_left,
    int32_t source_top,
    int32_t source_right, int32_t source_bottom, int32_t destination_left,
    int32_t destination_top, int32_t destination_right,
    int32_t destination_bottom, bool has_damage, int32_t damage_left,
    int32_t damage_top, int32_t damage_right, int32_t damage_bottom,
    float alpha);
void darwin_art_android_present_surface_control_state(
    uint32_t owner_process_id, uint32_t layer_id,
    uint32_t parent_owner_process_id, uint32_t parent_id,
    uint32_t relative_parent_owner_process_id, uint32_t relative_parent_id,
    uint64_t what, uint32_t flags, uint32_t mask, uint32_t transform,
    int32_t destination_left,
    int32_t destination_top, int32_t destination_right,
    int32_t destination_bottom, int32_t position_x, int32_t position_y,
    float scale_x, float scale_y, bool has_crop, int32_t crop_left,
    int32_t crop_top, int32_t crop_right, int32_t crop_bottom, int32_t z,
    float alpha,
    const int32_t* transparent_region_rects,
    uint32_t transparent_region_count);
}
