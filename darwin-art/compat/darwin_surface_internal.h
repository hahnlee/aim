#pragma once

#include "darwin_surface_bridge.h"
#include "input/surface_input_sink.h"
#include "window/surface_scanout_owner.h"
#include "window/appkit_content_view.h"
#include "window/desktop_root_events.h"
#include "window/desktop_root_target.h"
#include "surfaceflinger/output_owner.h"
#include "graphics/surface_backing_owner.h"

#import <AppKit/AppKit.h>
#import <IOSurface/IOSurface.h>
#import <Metal/Metal.h>
#import <QuartzCore/CAMetalLayer.h>

#include <cstddef>
#include <atomic>
#include <memory>
#include <mutex>

// Shared host-owned surface state. GPU-only state is deliberately opaque so
// this header can be compiled by the CPU surface bridge without any Skia
// dependency. The GPU bridge owns the object stored in gpu_state.
struct DarwinArtSurface {
  // Only the AppKit surface owner changes this control connection. GPU/child
  // producers never receive it; EOF is authoritative remote retirement.
  std::unique_ptr<darwin_art::surfaceflinger::OutputOwner> output_owner;
  darwin_art::graphics::SurfaceBackingOwner backing_owner;
  // AppKit-only cached immutable publication. Off-actor consumers Acquire()
  // from backing_owner; they never read this shared_ptr variable directly.
  darwin_art::graphics::SurfaceBackingOwner::Handle backing;
  darwin_art::graphics::SurfaceBackingOwner::Handle mapped_backing;
  // Completion metadata only; not allocation ownership or GPU admission.
  mutable std::mutex submission_mutex;
  bool scale_to_display = false;
  id<MTLDevice> device = nil;
  id<MTLCommandQueue> command_queue = nil;
  id<MTLCommandBuffer> last_command_buffer = nil;
  // Direct RenderThread GPU submissions are produced off the AppKit actor;
  // keep their completion handle separate from the main-actor IOSurface blit.
  id<MTLCommandBuffer> last_gpu_command_buffer = nil;
  // A single opaque owner serializes notify/fence readiness, dirty-generation
  // claims, and latest-wins presentation requests without exposing those
  // containers to the surface bridge.
  std::unique_ptr<darwin_art::window::SurfaceScanoutOwner> scanout_owner;
  // Worker-thread frame pulses request scanout through a single latest-wins
  // AppKit command. The generation/mutex pair bounds the main-queue backlog
  // to one block while guaranteeing that a request arriving during a blit
  // schedules one trailing main turn instead of being lost.
  NSWindow* window = nil;
  id window_delegate = nil;
  // Host lifecycle facts only; Android WMS owns focus selection. Initialized
  // before publication, immutable until destruction; owner-thread acquisition
  // must exclude concurrent surface destruction.
  std::shared_ptr<darwin_art::window::DesktopRootEvents> desktop_root_events;
  // Exact process-root capability retained independently of the raw surface
  // pointer so an owner Looper can bind after surface publication.
  std::shared_ptr<darwin_art::window::DesktopRootTarget> desktop_root_target;
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
  mutable std::mutex input_sink_mutex;
  std::shared_ptr<DarwinArtSurfaceInputSinkBinding> input_sink;
  std::atomic<bool> producer_mapped{false};
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

// AppKit main thread only. Reallocates the scanout backing to `width`x`height`
// physical pixels publishing `logical_*` Android pixels to SurfaceFlinger
// (0 derives them from the backing scale); `update_window` resizes the window.
DarwinArtSurfaceResult ResizeSurfaceBackingOnMain(DarwinArtSurface* surface,
    uint32_t width, uint32_t height, bool update_window, uint32_t logical_width,
    uint32_t logical_height);

// Weak no-op in the CPU bridge; the GPU bridge supplies the strong cleanup
// implementation when it is linked into the graphics runtime.
void darwin_art_surface_gpu_forget(DarwinArtSurface* surface);
