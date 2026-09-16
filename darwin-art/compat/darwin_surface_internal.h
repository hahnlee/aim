#pragma once

#include "darwin_surface_bridge.h"

#import <AppKit/AppKit.h>
#import <IOSurface/IOSurface.h>
#import <Metal/Metal.h>
#import <QuartzCore/CAMetalLayer.h>

#include <cstddef>
#include <atomic>
#include <condition_variable>
#include <deque>
#include <mutex>
#include <thread>

struct DarwinArtSurface;

@interface DarwinArtMetalView : NSView
@property(nonatomic, readonly) CAMetalLayer* metalLayer;
- (instancetype)initWithFrame:(NSRect)frame
                       device:(id<MTLDevice>)device
                    pixelSize:(CGSize)pixelSize
                 contentScale:(CGFloat)contentScale;
- (BOOL)nextPointerEvent:(DarwinArtPointerEvent*)outEvent;
- (BOOL)nextPointerEventV2:(DarwinArtPointerEventV2*)outEvent;
- (BOOL)nextKeyEventV1:(DarwinArtKeyEventV1*)outEvent;
- (BOOL)clearInputHintIfEmpty;
- (void)cancelPointerStream;
- (void)updateDrawableSize;
- (void)setOwnerSurface:(DarwinArtSurface*)surface;
- (void)signalOwnerWake;
@end

// Shared host-owned surface state. GPU-only state is deliberately opaque so
// this header can be compiled by the CPU surface bridge without any Skia
// dependency. The GPU bridge owns the object stored in gpu_state.
struct DarwinArtSurface {
  // Protects the IOSurface/Metal backing tuple while a native producer on a
  // separate Android GL thread refreshes its retained target during resize.
  mutable std::mutex backing_mutex;
  uint32_t width = 0;
  uint32_t height = 0;
  uint32_t logical_width = 0;
  uint32_t logical_height = 0;
  bool scale_to_display = false;
  size_t bytes_per_row = 0;
  IOSurfaceRef io_surface = nullptr;
  id<MTLDevice> device = nil;
  id<MTLCommandQueue> command_queue = nil;
  id<MTLTexture> io_surface_texture = nil;
  id<MTLCommandBuffer> last_command_buffer = nil;
  // Direct RenderThread GPU submissions are produced off the AppKit actor;
  // keep their completion handle separate from the main-actor IOSurface blit.
  id<MTLCommandBuffer> last_gpu_command_buffer = nil;
  mutable std::mutex presentation_mutex;
  bool presentation_closing = false;
  bool presentation_scheduled = false;
  uint64_t presentation_requested = 0;
  // Scheduler telemetry is intentionally atomic so display-clock and fence
  // monitor threads can account for requests without taking the AppKit
  // presentation mutex. These counters are diagnostic only and do not alter
  // the bounded latest-wins policy.
  std::atomic<uint64_t> scanout_requests{0};
  std::atomic<uint64_t> scanout_fence_gated{0};
  std::atomic<uint64_t> scanout_coalesced{0};
  // A display tick does not need to enqueue another AppKit blit when the
  // latest SurfaceFlinger composition or embedded IOSurface frame is already
  // in flight or has been presented. Surfaces with neither dirty source keep
  // the legacy every-tick behavior.
  std::atomic<uint64_t> scanout_last_requested_generation{0};
  std::atomic<uint64_t> scanout_last_requested_embedded_frame{0};
  // Central SurfaceFlinger can recompose this display because system_server
  // changed the retained layer tree without an app-owned present fence. A
  // Darwin notify check token is the platform display-consumer wakeup for
  // those cross-process compositions; it is polled only at display-clock
  // edges and therefore cannot call into AppKit from the service process.
  std::atomic<int> scanout_notification_token{-1};
  std::atomic<bool> scanout_reimport_backing{false};
  std::atomic<uint64_t> scanout_dirty_skipped{0};
  std::atomic<uint64_t> scanout_present_calls{0};
  std::atomic<int32_t> last_scanout_status{DARWIN_ART_SURFACE_OK};
  // SurfaceFlinger composition completion is a separate readiness boundary
  // from the display clock.  The producer registers each returned Android
  // fence here; a single monitor thread advances the ready generation only
  // after the broker reports that exact composition complete.
  struct CompositionFence {
    int descriptor = -1;
    uint64_t generation = 0;
    bool ready = false;
  };
  mutable std::mutex composition_mutex;
  std::condition_variable composition_available;
  std::deque<CompositionFence> composition_fences;
  std::thread composition_monitor;
  bool composition_monitor_started = false;
  bool composition_monitor_stop = false;
  uint64_t composition_next_generation = 0;
  std::atomic<uint64_t> composition_submitted_generation{0};
  std::atomic<uint64_t> composition_ready_generation{0};
  // Worker-thread frame pulses request scanout through a single latest-wins
  // AppKit command. The generation/mutex pair bounds the main-queue backlog
  // to one block while guaranteeing that a request arriving during a blit
  // schedules one trailing main turn instead of being lost.
  NSWindow* window = nil;
  id window_delegate = nil;
  DarwinArtMetalView* view = nil;
  bool visible = false;
  // Updated only by AppKit callbacks and read by the ART owner thread. This
  // lets the owner observe a user close without synchronously entering the
  // AppKit main queue (the main actor already owns event pumping).
  std::atomic<bool> window_closed{false};
  // AppKit input wakes the Android owner Looper through this POD hook. The
  // callback is copied under the mutex and invoked after releasing it.
  mutable std::mutex owner_wake_mutex;
  DarwinArtSurfaceOwnerWakeCallback owner_wake_callback = nullptr;
  void* owner_wake_context = nullptr;
  bool producer_mapped = false;
  void* gpu_state = nullptr;
  // Geometry and publication state for a SurfaceView whose EGL producer
  // renders directly into io_surface.  The Android GL thread publishes only
  // after eglWaitGL(), while the HWUI/Metal thread samples the same texture.
  std::atomic<int32_t> embedded_surface_x{0};
  std::atomic<int32_t> embedded_surface_y{0};
  std::atomic<uint32_t> embedded_surface_width{0};
  std::atomic<uint32_t> embedded_surface_height{0};
  std::atomic<uint32_t> embedded_buffer_width{0};
  std::atomic<uint32_t> embedded_buffer_height{0};
  std::atomic<uint64_t> embedded_surface_frame{0};

  ~DarwinArtSurface();
};

// Published with release/acquire because Android GL/child-surface threads
// may resolve the host surface while the AppKit owner creates or tears it
// down. Lifetime is still bounded by the owner-thread surface lease; the
// atomic only removes the raw-pointer data race at the publication boundary.
extern std::atomic<DarwinArtSurface*> g_active_gpu_surface;

// Weak no-op in the CPU bridge; the GPU bridge supplies the strong cleanup
// implementation when it is linked into the graphics runtime.
void darwin_art_surface_gpu_forget(DarwinArtSurface* surface);
