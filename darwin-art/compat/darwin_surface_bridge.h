#pragma once

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

// Opaque owner of one persistent NSWindow, CAMetalLayer, IOSurface, and Metal
// texture. AppKit lifecycle/presentation calls require the macOS main thread;
// pointer/key packet dequeue is worker-safe and does not touch AppKit objects.
typedef struct DarwinArtSurface DarwinArtSurface;
typedef struct DarwinArtGpuFrame DarwinArtGpuFrame;

// Worker-safe wake hook used by the Android owner reactor. The producer is
// AppKit (input) while the consumer is the ART owner Looper; only this POD
// callback crosses the boundary, never an NSView/JNIEnv/SurfaceSession.
typedef void (*DarwinArtSurfaceOwnerWakeCallback)(void* context);

typedef enum DarwinArtSurfaceResult {
  DARWIN_ART_SURFACE_OK = 0,
  DARWIN_ART_SURFACE_INVALID_ARGUMENT = 1,
  DARWIN_ART_SURFACE_NOT_MAIN_THREAD = 2,
  DARWIN_ART_SURFACE_ALLOCATION_FAILED = 3,
  DARWIN_ART_SURFACE_METAL_UNAVAILABLE = 4,
  DARWIN_ART_SURFACE_DRAWABLE_UNAVAILABLE = 5,
  DARWIN_ART_SURFACE_GPU_SUBMISSION_FAILED = 6,
  DARWIN_ART_SURFACE_WINDOW_CLOSED = 7,
  DARWIN_ART_SURFACE_PRODUCER_ALREADY_MAPPED = 8,
  DARWIN_ART_SURFACE_PRODUCER_NOT_MAPPED = 9,
} DarwinArtSurfaceResult;

typedef struct DarwinArtSurfaceCreateInfo {
  uint32_t width;
  uint32_t height;
  // UTF-8, copied during creation. Null selects a default title.
  const char* title;
  // When false, no persistent window is created. A true request is still
  // sanitized at the AppKit boundary for Android system/service processes,
  // which may need a headless Metal target but never own desktop UI.
  bool visible;
  // Treat width/height as Android logical display pixels and allocate the
  // scanout backing at the active macOS screen scale. Offscreen and legacy
  // callers leave this false and continue to request physical pixels.
  bool scale_to_display;
} DarwinArtSurfaceCreateInfo;

typedef struct DarwinArtSurfaceProducerMapping {
  // Stable IOSurface CPU mapping. Valid only between a successful producer
  // map and its matching unmap.
  void* base_address;
  size_t bytes_per_row;
  size_t allocation_size;
  uint32_t width;
  uint32_t height;
} DarwinArtSurfaceProducerMapping;

typedef enum DarwinArtPointerAction {
  DARWIN_ART_POINTER_DOWN = 0,
  DARWIN_ART_POINTER_UP = 1,
  DARWIN_ART_POINTER_MOVE = 2,
  DARWIN_ART_POINTER_CANCEL = 3,
} DarwinArtPointerAction;

typedef enum DarwinArtPointerFlags {
  // The packet originated from an AppKit mouse/trackpad button stream. The
  // Android bridge exposes it as SOURCE_MOUSE instead of forcing the window
  // into touchscreen mode. Zero remains the synthetic touchscreen default.
  DARWIN_ART_POINTER_FLAG_MOUSE = 1u << 0,
} DarwinArtPointerFlags;

typedef struct DarwinArtPointerEvent {
  uint32_t action;
  float x;
  float y;
} DarwinArtPointerEvent;

// Versioned, pointer-free event packet used by the Rust ingress. The legacy
// three-field packet remains exported for older probes, while new hosts get
// ordering and Android timing metadata without crossing JNI or borrowing an
// AppKit object.
typedef struct DarwinArtPointerEventV2 {
  uint32_t version;
  uint32_t size;
  uint32_t action;
  uint32_t flags;
  uint64_t sequence;
  uint64_t event_time_nanos;
  uint64_t down_time_nanos;
  uint32_t pointer_id;
  uint32_t pointer_count;
  float x;
  float y;
  float raw_x;
  float raw_y;
  float pressure;
  float size_value;
} DarwinArtPointerEventV2;

// Host keyboard packet. macOS key events are modeled as an external Android
// physical keyboard (device 1, SOURCE_KEYBOARD), while the framework's
// VIRTUAL_KEYBOARD id remains only a default KeyCharacterMap fallback.
typedef struct DarwinArtKeyEventV1 {
  uint32_t version;
  uint32_t size;
  uint32_t action;
  uint32_t flags;
  uint64_t sequence;
  uint64_t event_time_nanos;
  uint64_t down_time_nanos;
  uint32_t key_code;
  uint32_t scan_code;
  uint32_t meta_state;
  uint32_t repeat_count;
  int32_t device_id;
  uint32_t source;
  uint32_t unicode_char;
} DarwinArtKeyEventV1;

// Creates persistent Apple graphics objects. The returned handle owns them
// until darwin_art_surface_destroy(). Returns null on failure and writes the
// reason to out_result when it is non-null.
DarwinArtSurface* darwin_art_surface_create(
    const DarwinArtSurfaceCreateInfo* create_info,
    DarwinArtSurfaceResult* out_result);

// Updates the IOSurface/CAMetalLayer backing and pixel dimensions. Calls from
// an ART worker are synchronously marshalled to AppKit's main queue and wait
// for the prior GPU submission before swapping the backing.
DarwinArtSurfaceResult darwin_art_surface_resize(
    DarwinArtSurface* surface,
    uint32_t width,
    uint32_t height);

bool darwin_art_surface_get_size(
    DarwinArtSurface* surface,
    uint32_t* width,
    uint32_t* height);

bool darwin_art_surface_get_logical_size(
    DarwinArtSurface* surface,
    uint32_t* width,
    uint32_t* height);

DarwinArtSurfaceResult darwin_art_surface_set_title(
    DarwinArtSurface* surface,
    const char* title);

// Publish framework-resolved task metadata without exposing or transferring
// ownership of the application process's active desktop display target.
DarwinArtSurfaceResult darwin_art_surface_set_active_title(const char* title);

// UTF-16 variant for Android CharSequence metadata. This preserves embedded
// non-ASCII and supplementary characters without JNI modified-UTF-8 loss.
DarwinArtSurfaceResult darwin_art_surface_set_active_title_utf16(
    const uint16_t* title,
    size_t length);

// Presents the native macOS document picker on the AppKit main thread.
// The returned UTF-8 filesystem path is allocated with malloc and must be
// released with darwin_art_host_document_path_free(). A null return means
// cancellation or an invalid calling thread. Android code never sees this
// host path directly; the runtime stages it behind a content:// provider.
char* darwin_art_host_open_document(const char* mime_type);
char* darwin_art_host_save_document(const char* mime_type,
                                    const char* suggested_name);
void darwin_art_host_document_path_free(char* path);

// Waits for the previous GPU presentation to finish, then locks the IOSurface
// for a CPU producer. Only one producer mapping may be active per surface.
DarwinArtSurfaceResult darwin_art_surface_map_producer(
    DarwinArtSurface* surface,
    DarwinArtSurfaceProducerMapping* out_mapping);

// Flush producer writes to the IOSurface and ends the active CPU mapping.
DarwinArtSurfaceResult darwin_art_surface_unmap_producer(
    DarwinArtSurface* surface);

// Copies one BGRA8-premultiplied CPU frame into the already allocated
// IOSurface. source_bytes_per_row may exceed width * 4 but may not be smaller.
// This API is intentionally independent of Java arrays. A future Skia bridge
// should prefer map_producer()/unmap_producer() to avoid this staging copy.
DarwinArtSurfaceResult darwin_art_surface_update(
    DarwinArtSurface* surface,
    const void* bgra_pixels,
    size_t source_bytes_per_row);

// Blits the persistent IOSurface-backed Metal texture into the next drawable
// of the persistent CAMetalLayer and schedules it for presentation. Calls
// from an ART worker are synchronously marshalled to AppKit's main queue.
DarwinArtSurfaceResult darwin_art_surface_present(
    DarwinArtSurface* surface);

// Latest-wins scanout request for the Android RenderThread-equivalent. It
// returns after enqueueing at most one AppKit command, so the ART/UI owner is
// never blocked behind nextDrawable or a busy main queue. The main actor
// drains the command in order; callers must keep the surface alive until the
// normal main-thread destroy dispatch runs.
DarwinArtSurfaceResult darwin_art_surface_present_async(
    DarwinArtSurface* surface);

// Binds the owner Looper wake token to a surface's AppKit input mailbox.
// Setting a null callback unbinds it before the owner/runtime is torn down.
// The callback may be invoked from AppKit event methods and must be
// non-blocking and thread-safe.
DarwinArtSurfaceResult darwin_art_surface_set_owner_wake(
    DarwinArtSurface* surface,
    DarwinArtSurfaceOwnerWakeCallback callback,
    void* context);

// GPU-only frame path. The returned frame wraps the CAMetalLayer drawable
// directly; callers must submit it with darwin_art_surface_gpu_end(). No
// IOSurface mapping or CPU pixel buffer is involved.
DarwinArtGpuFrame* darwin_art_surface_gpu_begin(DarwinArtSurface* surface);
void* darwin_art_surface_gpu_canvas(DarwinArtGpuFrame* frame);
// Returns the SkCanvas owned by the current GPU frame on the calling thread.
// It is valid only between gpu_begin() and gpu_end(); no ownership is
// transferred. Native APK renderers may use this seam to draw directly into
// the CAMetalLayer drawable without a CPU readback or an intermediate image.
void* darwin_art_surface_gpu_active_canvas(void);
DarwinArtSurfaceResult darwin_art_surface_gpu_end(
    DarwinArtSurface* surface, DarwinArtGpuFrame* frame);

// Runtime-owned GPU surface hand-off used by the in-process Android renderer.
DarwinArtSurface* darwin_art_surface_active_gpu(void);
void darwin_art_surface_set_active_gpu(DarwinArtSurface* surface);

// SurfaceFlinger completion/readiness bridge. `fence_fd` is an owned Android
// broker descriptor; the surface consumes it on a dedicated monitor and
// advances its ready generation only after the fence signals. Scanout callers
// can use the readiness query to avoid blitting a target while composition is
// still in flight.
bool darwin_art_surface_gpu_track_composition_fence(
    DarwinArtSurface* surface, int fence_fd);
bool darwin_art_surface_gpu_scanout_ready(DarwinArtSurface* surface);

// SurfaceView/ANGLE zero-copy bridge. The acquired handle is a retained
// IOSurfaceRef represented as an opaque pointer and must be released with the
// matching function. ANGLE renders into that IOSurface, then publishes a
// completed frame. HWUI samples the already-existing Metal texture in its
// normal GPU pass; no CPU mapping or pixel copy occurs.
bool darwin_art_surface_gpu_acquire_iosurface(
    DarwinArtSurface* surface,
    void** iosurface,
    uint32_t* width,
    uint32_t* height);
uint32_t darwin_art_surface_gpu_iosurface_id(DarwinArtSurface* surface);
bool darwin_art_surface_gpu_lookup_iosurface(
    uint32_t surface_id,
    void** iosurface,
    uint32_t* width,
    uint32_t* height);
void darwin_art_surface_gpu_release_iosurface(void* iosurface);
void darwin_art_surface_gpu_configure_embedded(
    DarwinArtSurface* surface,
    int32_t x,
    int32_t y,
    uint32_t width,
    uint32_t height);
bool darwin_art_surface_gpu_get_embedded_geometry(
    DarwinArtSurface* surface,
    int32_t* x,
    int32_t* y,
    uint32_t* width,
    uint32_t* height);
void darwin_art_surface_gpu_set_iosurface_composition_active(
    void* iosurface, bool active);
bool darwin_art_surface_gpu_is_iosurface_composition_active(void* iosurface);
void darwin_art_surface_gpu_publish_embedded(DarwinArtSurface* surface);
void darwin_art_surface_gpu_set_embedded_buffer_extent(
    DarwinArtSurface* surface, uint32_t width, uint32_t height);
bool darwin_art_surface_gpu_composite_embedded(
    DarwinArtSurface* surface,
    void* sk_canvas);

// Runs the NSApplication event pump in slices no longer than 16 ms. This is a
// main-thread API; worker callers receive only the atomic close-state result
// and must not use it as an event pump. seconds must be finite and in the
// inclusive range 0..30. A zero duration performs only a close-state check.
// WINDOW_CLOSED means the user closed the window; the handle remains valid
// and must still be destroyed by its owner.
DarwinArtSurfaceResult darwin_art_surface_pump_events(
    DarwinArtSurface* surface,
    double seconds);

// Services the process NSApplication run loop without requiring a surface.
// The host main thread uses this while ART is running on its owner worker and
// before the Activity has created its first Window/Surface.
int32_t darwin_art_appkit_pump_events(double seconds);

// Worker-safe close-state snapshot. This never touches AppKit and is intended
// for the ART owner while the process main actor owns event pumping.
bool darwin_art_surface_close_requested(DarwinArtSurface* surface);

// Removes the oldest pointer event captured by the surface's NSView mailbox.
// This dequeue is safe from a non-main worker; it touches no AppKit object.
// Event
// coordinates are expressed in backing pixels, matching the Android render
// target even when the host window uses a Retina content scale. Returns false
// when the queue is empty or either argument is null.
bool darwin_art_surface_next_pointer_event(
    DarwinArtSurface* surface,
    DarwinArtPointerEvent* out_event);

bool darwin_art_surface_next_pointer_event_v2(
    DarwinArtSurface* surface,
    DarwinArtPointerEventV2* out_event);

bool darwin_art_surface_next_key_event_v1(
    DarwinArtSurface* surface,
    DarwinArtKeyEventV1* out_event);

// Atomically observes the AppKit mailbox and clears Android's
// probablyHasInput hint only while it is empty. A producer that enqueues after
// this check publishes a new hint after the mailbox mutex is released.
bool darwin_art_surface_clear_input_hint_if_empty(DarwinArtSurface* surface);

// Waits for the most recently submitted command buffer, closes the window,
// and releases all owned Apple objects. Calls from an ART worker are
// synchronously marshalled to AppKit's main queue.
DarwinArtSurfaceResult darwin_art_surface_destroy(
    DarwinArtSurface* surface);

#ifdef __cplusplus
}  // extern "C"
#endif
