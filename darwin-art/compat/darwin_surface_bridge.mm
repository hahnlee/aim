#import <AppKit/AppKit.h>
#import <CoreFoundation/CoreFoundation.h>
#import <Metal/Metal.h>
#import <QuartzCore/CAMetalLayer.h>
#include "darwin_surface_internal.h"
#include "window/display_output.h"
#include "window/appkit_window_delegate.h"
#include "window/application_identity.h"
#include "window/desktop_root_surface.h"
#include "window/root_geometry.h"
#include "graphics/metal_display_backing.h"
#include "graphics/scanout_diagnostic_capture.h"

#include <algorithm>
#include <cmath>
#include <cerrno>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <dispatch/dispatch.h>
#include <iostream>
#include <limits>
#include <new>
#include <time.h>
#include <vector>

namespace {

static_assert(sizeof(bool) == 1);
static_assert(sizeof(DarwinArtSurfaceResult) == 4);
static_assert(sizeof(DarwinArtSurfaceCreateInfo) == 24);
static_assert(offsetof(DarwinArtSurfaceCreateInfo, width) == 0);
static_assert(offsetof(DarwinArtSurfaceCreateInfo, height) == 4);
static_assert(offsetof(DarwinArtSurfaceCreateInfo, title) == 8);
static_assert(offsetof(DarwinArtSurfaceCreateInfo, visible) == 16);
static_assert(offsetof(DarwinArtSurfaceCreateInfo, scale_to_display) == 17);

constexpr uint32_t kBytesPerPixel = 4;
constexpr uint32_t kMaximumDimension = 16384;

bool IsMainThread() {
  return [NSThread isMainThread];
}

void CompositionFenceReady(void* context, uint64_t generation) noexcept {
  auto* surface = static_cast<DarwinArtSurface*>(context);
  if (surface == nullptr) return;
  if (std::getenv("DARWIN_ART_DEBUG_SURFACE_TRANSACTIONS") != nullptr) {
    std::fprintf(stderr,
                 "ART SurfaceFlinger: scanout generation ready "
                 "surface=%p generation=%llu\n",
                 surface, static_cast<unsigned long long>(generation));
  }
  // A fence may signal between display edges. Request one latest-wins AppKit
  // turn immediately; the independent FrameClock continues regular edges.
  (void)darwin_art_surface_present_async(surface);
}

// Surface lifecycle and scanout are AppKit-owned, but ART will eventually run
// on a dedicated owner pthread.  Keep the marshalling primitive in this small
// bridge so callers never invoke AppKit from that worker.  The block is
// synchronous by design for create/resize/present/destroy ordering; the main
// thread must continue servicing its NSApplication run loop while a worker is
// alive (a blocking join would deadlock this path).
template <typename Function>
auto RunOnMainSync(Function&& function) -> decltype(function()) {
  if (IsMainThread()) return function();
  using Result = decltype(function());
  __block Result result{};
  dispatch_sync(dispatch_get_main_queue(), ^{
    result = function();
  });
  return result;
}

bool IsValidDimension(uint32_t value) {
  return value > 0 && value <= kMaximumDimension;
}


CGFloat WindowScale(bool visible, bool automatic) {
  const char* value = std::getenv("DARWIN_ART_WINDOW_SCALE");
  if (!visible) return 1.0;
  if (value != nullptr && std::strcmp(value, "2") == 0) return 2.0;
  if (value != nullptr && std::strcmp(value, "1") == 0) return 1.0;
  if (!automatic) return 1.0;
  NSScreen* screen = NSScreen.mainScreen;
  const CGFloat scale = screen == nil ? 1.0 : screen.backingScaleFactor;
  return scale > 0.0 ? scale : 1.0;
}

}  // namespace



DarwinArtSurface::~DarwinArtSurface() {
    // Retained NSViews/cancellation callbacks cannot keep a dangling owner.
    [view setOwnerSurface:nullptr];
    // Stop the private scanout owner before any surface-owned callback context
    // or Apple object is released. Stop joins polling/callback work before
    // the owner closes its remaining descriptors.
    // The monitor's admitted callback reads this published owner. Keep it
    // immutable until StopAndJoin has quiesced every borrowed-surface tail.
    if (this->scanout_owner != nullptr) this->scanout_owner->StopAndJoin();
    auto scanout_owner = std::move(this->scanout_owner);
    RetireDisplayOutput(this);
    // Metal textures created from an IOSurface may consult that IOSurface
    // while they are being released. ARC destroys C++ fields after the
    // destructor body, so release the Objective-C owners explicitly before
    // dropping our Core Foundation reference.
    last_command_buffer = nil;
    if (window != nil) window.delegate = nil;
    window_delegate = nil;
    (void)backing_owner.Close();
    backing.reset();
    mapped_backing.reset();
    command_queue = nil;
    device = nil;
    view = nil;
    window = nil;
}

std::atomic<DarwinArtSurface*> g_active_gpu_surface{nullptr};

__attribute__((weak)) void darwin_art_surface_gpu_forget(
    DarwinArtSurface*) {}

// CPU/runtime links retain the surface ABI but deliberately do not link
// Ganesh or Metal.  Keep the optional GPU capability closed in that flavor;
// the graphics link supplies the strong implementations from
// darwin_surface_gpu_bridge.mm.
__attribute__((weak)) DarwinArtSurface* darwin_art_surface_active_gpu() {
  return nullptr;
}

__attribute__((weak)) void darwin_art_surface_set_active_gpu(
    DarwinArtSurface*) {}

bool darwin_art_surface_gpu_acquire_iosurface(
    DarwinArtSurface* surface, void** iosurface, uint32_t* width,
    uint32_t* height) {
  if (surface == nullptr || iosurface == nullptr || width == nullptr ||
      height == nullptr) {
    return false;
  }
  const auto backing = surface->backing_owner.Acquire();
  if (!backing) return false;
  CFRetain(backing->surface());
  *iosurface = backing->surface();
  *width = backing->physical_width();
  *height = backing->physical_height();
  return true;
}

uint32_t darwin_art_surface_gpu_iosurface_id(DarwinArtSurface* surface) {
  if (surface == nullptr) return 0;
  const auto backing = surface->backing_owner.Acquire();
  return backing ? IOSurfaceGetID(backing->surface()) : 0;
}

bool darwin_art_surface_gpu_lookup_iosurface(
    uint32_t surface_id, void** iosurface, uint32_t* width,
    uint32_t* height) {
  if (surface_id == 0 || iosurface == nullptr || width == nullptr ||
      height == nullptr) {
    return false;
  }
  IOSurfaceRef found = IOSurfaceLookup(surface_id);
  if (found == nullptr) return false;
  const size_t found_width = IOSurfaceGetWidth(found);
  const size_t found_height = IOSurfaceGetHeight(found);
  if (found_width == 0 || found_height == 0 ||
      found_width > UINT32_MAX || found_height > UINT32_MAX) {
    CFRelease(found);
    return false;
  }
  *iosurface = found;
  *width = static_cast<uint32_t>(found_width);
  *height = static_cast<uint32_t>(found_height);
  return true;
}

void darwin_art_surface_gpu_release_iosurface(void* iosurface) {
  if (iosurface != nullptr) CFRelease(static_cast<CFTypeRef>(iosurface));
}

void darwin_art_surface_gpu_configure_embedded(
    DarwinArtSurface* surface, int32_t x, int32_t y, uint32_t width,
    uint32_t height) {
  if (surface == nullptr) return;
  surface->embedded_surface_x.store(x, std::memory_order_relaxed);
  surface->embedded_surface_y.store(y, std::memory_order_relaxed);
  surface->embedded_surface_width.store(width, std::memory_order_relaxed);
  surface->embedded_surface_height.store(height, std::memory_order_release);
}

bool darwin_art_surface_gpu_get_embedded_geometry(
    DarwinArtSurface* surface, int32_t* x, int32_t* y, uint32_t* width,
    uint32_t* height) {
  if (surface == nullptr || x == nullptr || y == nullptr || width == nullptr ||
      height == nullptr) {
    return false;
  }
  *height = surface->embedded_surface_height.load(std::memory_order_acquire);
  *width = surface->embedded_surface_width.load(std::memory_order_relaxed);
  *x = surface->embedded_surface_x.load(std::memory_order_relaxed);
  *y = surface->embedded_surface_y.load(std::memory_order_relaxed);
  return *width > 0 && *height > 0;
}

void darwin_art_surface_gpu_set_iosurface_composition_active(
    void* iosurface, bool active) {
  if (iosurface == nullptr) return;
  IOSurfaceSetValue(static_cast<IOSurfaceRef>(iosurface),
                    CFSTR("dev.darwinart.surface-composition-active"),
                    active ? kCFBooleanTrue : kCFBooleanFalse);
}

bool darwin_art_surface_gpu_is_iosurface_composition_active(void* iosurface) {
  if (iosurface == nullptr) return true;
  CFTypeRef value = IOSurfaceCopyValue(
      static_cast<IOSurfaceRef>(iosurface),
      CFSTR("dev.darwinart.surface-composition-active"));
  if (value == nullptr) return true;
  const bool active = value == kCFBooleanTrue ||
                      (CFGetTypeID(value) == CFBooleanGetTypeID() &&
                       CFBooleanGetValue(static_cast<CFBooleanRef>(value)));
  CFRelease(value);
  return active;
}

void darwin_art_surface_gpu_publish_embedded(DarwinArtSurface* surface) {
  if (surface == nullptr) return;
  surface->embedded_surface_frame.fetch_add(1, std::memory_order_release);
}

void darwin_art_surface_gpu_set_embedded_buffer_extent(
    DarwinArtSurface* surface, uint32_t width, uint32_t height) {
  if (surface == nullptr) return;
  surface->embedded_buffer_width.store(width, std::memory_order_relaxed);
  surface->embedded_buffer_height.store(height, std::memory_order_release);
}

__attribute__((weak)) bool darwin_art_surface_gpu_composite_embedded(
    DarwinArtSurface*, void*) {
  return false;
}

DarwinArtSurfaceResult darwin_art_surface_map_producer(
    DarwinArtSurface* surface,
    DarwinArtSurfaceProducerMapping* out_mapping) {
  if (!IsMainThread()) {
    return DARWIN_ART_SURFACE_NOT_MAIN_THREAD;
  }
  if (surface == nullptr || out_mapping == nullptr) {
    return DARWIN_ART_SURFACE_INVALID_ARGUMENT;
  }
  *out_mapping = DarwinArtSurfaceProducerMapping{};
  if (surface->producer_mapped) {
    return DARWIN_ART_SURFACE_PRODUCER_ALREADY_MAPPED;
  }

  [surface->last_command_buffer waitUntilCompleted];
  id<MTLCommandBuffer> last_gpu_command_buffer = nil;
  {
    std::lock_guard<std::mutex> lock(surface->submission_mutex);
    last_gpu_command_buffer = surface->last_gpu_command_buffer;
  }
  [last_gpu_command_buffer waitUntilCompleted];
  if (surface->last_command_buffer != nil &&
      surface->last_command_buffer.status == MTLCommandBufferStatusError) {
    return DARWIN_ART_SURFACE_GPU_SUBMISSION_FAILED;
  }
  if (last_gpu_command_buffer != nil &&
      last_gpu_command_buffer.status == MTLCommandBufferStatusError) {
    return DARWIN_ART_SURFACE_GPU_SUBMISSION_FAILED;
  }
  const auto mapped = surface->backing_owner.Acquire();
  if (!mapped) return DARWIN_ART_SURFACE_ALLOCATION_FAILED;
  if (IOSurfaceLock(mapped->surface(), 0, nullptr) != kIOReturnSuccess) {
    return DARWIN_ART_SURFACE_ALLOCATION_FAILED;
  }
  void* base_address = IOSurfaceGetBaseAddress(mapped->surface());
  if (base_address == nullptr) {
    IOSurfaceUnlock(mapped->surface(), 0, nullptr);
    return DARWIN_ART_SURFACE_ALLOCATION_FAILED;
  }

  surface->mapped_backing = mapped;
  surface->producer_mapped = true;
  *out_mapping = DarwinArtSurfaceProducerMapping{
      .base_address = base_address,
      .bytes_per_row = mapped->bytes_per_row(),
      .allocation_size = IOSurfaceGetAllocSize(mapped->surface()),
      .width = mapped->physical_width(),
      .height = mapped->physical_height(),
  };
  return DARWIN_ART_SURFACE_OK;
}

DarwinArtSurfaceResult darwin_art_surface_unmap_producer(
    DarwinArtSurface* surface) {
  if (!IsMainThread()) {
    return DARWIN_ART_SURFACE_NOT_MAIN_THREAD;
  }
  if (surface == nullptr) {
    return DARWIN_ART_SURFACE_INVALID_ARGUMENT;
  }
  if (!surface->producer_mapped) {
    return DARWIN_ART_SURFACE_PRODUCER_NOT_MAPPED;
  }
  if (!surface->mapped_backing ||
      IOSurfaceUnlock(surface->mapped_backing->surface(), 0, nullptr) != kIOReturnSuccess) {
    return DARWIN_ART_SURFACE_ALLOCATION_FAILED;
  }
  surface->producer_mapped = false;
  surface->mapped_backing.reset();
  return DARWIN_ART_SURFACE_OK;
}

static DarwinArtSurface* CreateSurfaceOnMain(
    const DarwinArtSurfaceCreateInfo* create_info,
    DarwinArtSurfaceResult* out_result) {
  auto finish = [out_result](DarwinArtSurfaceResult result,
                             DarwinArtSurface* surface) {
    if (out_result != nullptr) {
      *out_result = result;
    }
    return surface;
  };

  if (!IsMainThread()) {
    return finish(DARWIN_ART_SURFACE_NOT_MAIN_THREAD, nullptr);
  }
  if (create_info == nullptr || !IsValidDimension(create_info->width) ||
      !IsValidDimension(create_info->height)) {
    return finish(DARWIN_ART_SURFACE_INVALID_ARGUMENT, nullptr);
  }

  // Android service and system processes may initialize HWUI/Metal, but they
  // are not Activity display owners. Sanitize the request at the AppKit
  // boundary so older service launch paths cannot allocate an onscreen
  // NSWindow either.
  const bool visible = create_info->visible;
  const CGFloat window_scale = WindowScale(visible, create_info->scale_to_display);
  const uint32_t backing_width = create_info->scale_to_display
      ? static_cast<uint32_t>(std::ceil(create_info->width * window_scale))
      : create_info->width;
  const uint32_t backing_height = create_info->scale_to_display
      ? static_cast<uint32_t>(std::ceil(create_info->height * window_scale))
      : create_info->height;
  if (!IsValidDimension(backing_width) || !IsValidDimension(backing_height)) {
    return finish(DARWIN_ART_SURFACE_INVALID_ARGUMENT, nullptr);
  }

  @autoreleasepool {
    id<MTLDevice> device = MTLCreateSystemDefaultDevice();
    if (device == nil) {
      return finish(DARWIN_ART_SURFACE_METAL_UNAVAILABLE, nullptr);
    }
    id<MTLCommandQueue> command_queue = [device newCommandQueue];
    if (command_queue == nil) {
      return finish(DARWIN_ART_SURFACE_METAL_UNAVAILABLE, nullptr);
    }

    darwin_art::graphics::MetalDisplayBacking backing;
    const auto backing_result = darwin_art::graphics::AllocateMetalDisplayBacking(
        device, backing_width, backing_height, &backing);
    if (backing_result != darwin_art::graphics::MetalDisplayBackingResult::kOk) {
      return finish(
          backing_result == darwin_art::graphics::MetalDisplayBackingResult::kMetalUnavailable
              ? DARWIN_ART_SURFACE_METAL_UNAVAILABLE
              : DARWIN_ART_SURFACE_ALLOCATION_FAILED,
          nullptr);
    }

    DarwinArtSurface* surface = new (std::nothrow) DarwinArtSurface();
    if (surface == nullptr) {
      return finish(DARWIN_ART_SURFACE_ALLOCATION_FAILED, nullptr);
    }
    surface->scale_to_display = create_info->scale_to_display;
    auto candidate = darwin_art::graphics::RetainSurfaceBacking(backing.surface(),
        backing.texture(), backing_width, backing_height, create_info->width,
        create_info->height, backing.bytes_per_row(), 1);
    if (!candidate || !surface->backing_owner.Publish(nullptr, candidate)) {
      delete surface;
      return finish(DARWIN_ART_SURFACE_ALLOCATION_FAILED, nullptr);
    }
    surface->backing = std::move(candidate);
    surface->device = device;
    surface->command_queue = command_queue;
    IOSurfaceRef io_surface = surface->backing->surface();
    const size_t actual_bytes_per_row = surface->backing->bytes_per_row();
    surface->visible = visible;
    surface->window_closed.store(false, std::memory_order_release);
    surface->scanout_owner = darwin_art::window::SurfaceScanoutOwner::Create(
        &CompositionFenceReady, surface);
    if (surface->scanout_owner == nullptr) {
      delete surface;
      return finish(DARWIN_ART_SURFACE_ALLOCATION_FAILED, nullptr);
    }
    if (visible) {
      const char* endpoint = std::getenv("DARWIN_ART_SURFACEFLINGER_SOCKET");
      if (endpoint != nullptr && endpoint[0] != '\0') {
        const auto attached = AttachDisplayOutput(
            surface, endpoint, create_info->width, create_info->height);
        if (attached != DARWIN_ART_SURFACE_OK) {
          delete surface;
          return finish(attached, nullptr);
        }
      }
    }
    (void)surface->scanout_owner->RebindBacking(
        surface->backing->surface() == nullptr ? 0 : IOSurfaceGetID(surface->backing->surface()));

    NSApplication* application = NSApplication.sharedApplication;
    if (visible) {
      [application setActivationPolicy:NSApplicationActivationPolicyRegular];
    }
    NSRect frame = NSMakeRect(
        0, 0,
        create_info->scale_to_display
            ? static_cast<CGFloat>(create_info->width)
            : static_cast<CGFloat>(backing_width) / window_scale,
        create_info->scale_to_display
            ? static_cast<CGFloat>(create_info->height)
            : static_cast<CGFloat>(backing_height) / window_scale);
    // Headless surfaces still need a Metal view/layer for GPU work, but must
    // not allocate an AppKit window. A large number of probes and hidden
    // rendering surfaces are created with visible=false; creating an
    // NSWindow for each one makes tests leak apparent windows and needlessly
    // couples offscreen rendering to AppKit window management.
    if (visible) {
      surface->window = [[NSWindow alloc]
          initWithContentRect:frame
                    styleMask:(NSWindowStyleMaskTitled |
                               NSWindowStyleMaskClosable |
                               NSWindowStyleMaskMiniaturizable |
                               NSWindowStyleMaskResizable)
                      backing:NSBackingStoreBuffered
                        defer:NO];
      if (surface->window == nil) {
        delete surface;
        return finish(DARWIN_ART_SURFACE_ALLOCATION_FAILED, nullptr);
      }
      // The opaque handle, not AppKit's close operation, owns the window.
      surface->window.releasedWhenClosed = NO;
      surface->window.title =
          darwin_art::window::DecodeSurfaceWindowTitle(create_info->title);
      // The green button and View > Enter Full Screen take the root to macOS
      // fullscreen; the resulting content resize is a host geometry fact.
      surface->window.collectionBehavior |= NSWindowCollectionBehaviorFullScreenPrimary;
      darwin_art::window::ApplySurfaceApplicationIdentity(application, surface->window);
      darwin_art::window::InstallSurfaceApplicationMenu(application, surface->window.title);
      darwin_art::window::InstallSurfaceApplicationDelegate(application);
      const auto root_result = darwin_art::window::InitializeDesktopRoot(surface);
      if (root_result != DARWIN_ART_SURFACE_OK) {
        delete surface;
        return finish(root_result, nullptr);
      }
    }
    surface->view = [[DarwinArtMetalView alloc]
        initWithFrame:frame
               device:device
            pixelSize:CGSizeMake(backing_width, backing_height)
         contentScale:window_scale];
    if (surface->view == nil || surface->view.metalLayer == nil) {
      delete surface;
      return finish(DARWIN_ART_SURFACE_ALLOCATION_FAILED, nullptr);
    }
    [surface->view setOwnerSurface:surface];
    if (visible) {
      id<NSWindowDelegate> delegate =
          darwin_art::window::CreateSurfaceWindowDelegate(surface, &ResizeSurfaceBackingOnMain);
      if (delegate == nil) {
        delete surface;
        return finish(DARWIN_ART_SURFACE_ALLOCATION_FAILED, nullptr);
      }
      surface->window_delegate = delegate;
      surface->window.delegate = delegate;
      surface->window.contentView = surface->view;
      // The creation extent is not a user resize fact for Android geometry.
      darwin_art::window::RootGeometryReports::Process().NoteKnownExtent(
          static_cast<uint32_t>(std::ceil(NSWidth(frame))),
          static_cast<uint32_t>(std::ceil(NSHeight(frame))),
          darwin_art::window::SurfaceRasterScale(surface->window),
          darwin_art::window::SurfaceDisplayId(surface->window));
      [surface->window makeFirstResponder:surface->view];
      [surface->window center];
      [surface->window makeKeyAndOrderFront:nil];
      [application activateIgnoringOtherApps:YES];
    }

    if (IOSurfaceLock(io_surface, 0, nullptr) == kIOReturnSuccess) {
      void* base_address = IOSurfaceGetBaseAddress(io_surface);
      if (base_address != nullptr) {
        std::memset(base_address, 0,
                    actual_bytes_per_row *
                        static_cast<size_t>(backing_height));
      }
      IOSurfaceUnlock(io_surface, 0, nullptr);
    }
    if (visible) {
      // A visible surface is this Android application process's display
      // target. Publish it before ActivityThread enters its permanent Looper
      // so BLAST/HWUI and SurfaceFlinger can resolve the backing IOSurface
      // without waiting for the process entrypoint to return.
      darwin_art_surface_set_active_gpu(surface);
      {
        // Publish every visible display, including callers already supplying
        // physical Android pixels. Scaling policy does not control whether
        // SurfaceFlinger receives the target identity and scanout extent.
        darwin_art_surface_gpu_configure_embedded(
            surface, 0, 0, backing_width, backing_height);
        darwin_art_surface_gpu_set_embedded_buffer_extent(
            surface, backing_width, backing_height);
        darwin_art_surface_gpu_publish_embedded(surface);
        const uint32_t surface_id = IOSurfaceGetID(io_surface);
        if (surface_id != 0) {
          const std::string encoded = std::to_string(surface_id);
          setenv("DARWIN_ART_HOST_IOSURFACE_ID", encoded.c_str(), 1);
        }
      }
    }
    return finish(DARWIN_ART_SURFACE_OK, surface);
  }
}

DarwinArtSurface* darwin_art_surface_create(
    const DarwinArtSurfaceCreateInfo* create_info,
    DarwinArtSurfaceResult* out_result) {
  if (create_info == nullptr) {
    if (out_result != nullptr) *out_result = DARWIN_ART_SURFACE_INVALID_ARGUMENT;
    return nullptr;
  }
  // The caller owns this POD for the duration of the synchronous dispatch.
  // Copying it here prevents a worker from exposing a mutable request while
  // the AppKit block is queued.
  const DarwinArtSurfaceCreateInfo info = *create_info;
  return RunOnMainSync([&] { return CreateSurfaceOnMain(&info, out_result); });
}

DarwinArtSurfaceResult ResizeSurfaceBackingOnMain(DarwinArtSurface* surface,
                                                  uint32_t width,
                                                  uint32_t height,
                                                  bool update_window,
                                                  uint32_t logical_width,
                                                  uint32_t logical_height) {
  if (!IsMainThread()) return DARWIN_ART_SURFACE_NOT_MAIN_THREAD;
  if (surface == nullptr || !IsValidDimension(width) ||
      !IsValidDimension(height)) {
    return DARWIN_ART_SURFACE_INVALID_ARGUMENT;
  }
  if (logical_width == 0 || logical_height == 0) {
    const CGFloat scale = surface->scale_to_display && surface->view != nil &&
            surface->view.metalLayer.contentsScale > 0.0
        ? surface->view.metalLayer.contentsScale : 1.0;
    logical_width = static_cast<uint32_t>(std::max<CGFloat>(1.0, std::ceil(width / scale)));
    logical_height = static_cast<uint32_t>(std::max<CGFloat>(1.0, std::ceil(height / scale)));
  }
  if (surface->backing->physical_width() == width && surface->backing->physical_height() == height &&
      surface->backing->logical_width() == logical_width &&
      surface->backing->logical_height() == logical_height) return DARWIN_ART_SURFACE_OK;
  if (surface->producer_mapped) {
    return DARWIN_ART_SURFACE_PRODUCER_ALREADY_MAPPED;
  }
  [surface->last_command_buffer waitUntilCompleted];
  id<MTLCommandBuffer> last_gpu_command_buffer = nil;
  {
    std::lock_guard<std::mutex> lock(surface->submission_mutex);
    last_gpu_command_buffer = surface->last_gpu_command_buffer;
  }
  [last_gpu_command_buffer waitUntilCompleted];
  if (surface->last_command_buffer != nil &&
      surface->last_command_buffer.status == MTLCommandBufferStatusError) {
    return DARWIN_ART_SURFACE_GPU_SUBMISSION_FAILED;
  }
  if (last_gpu_command_buffer != nil &&
      last_gpu_command_buffer.status == MTLCommandBufferStatusError) {
    return DARWIN_ART_SURFACE_GPU_SUBMISSION_FAILED;
  }
  darwin_art::graphics::MetalDisplayBacking backing;
  if (darwin_art::graphics::AllocateMetalDisplayBacking(
          surface->device, width, height, &backing) !=
      darwin_art::graphics::MetalDisplayBackingResult::kOk) {
    return DARWIN_ART_SURFACE_ALLOCATION_FAILED;
  }
  // IPC is outside backing/presentation locks; the candidate has its own +1
  // IOSurface reference until commit or rejection. Producer snapshots retain
  // the former tuple independently while the server changes its epoch.
  auto candidate = darwin_art::graphics::RetainSurfaceBacking(backing.surface(),
      backing.texture(), width, height, logical_width, logical_height,
      backing.bytes_per_row(), surface->backing->revision() + 1);
  if (!candidate) return DARWIN_ART_SURFACE_ALLOCATION_FAILED;
  const auto replaced = ReplaceDisplayOutput(surface, backing.surface(),
      width, height, logical_width, logical_height);
  if (replaced != DARWIN_ART_SURFACE_OK) {
    return replaced;
  }
  if (!surface->backing_owner.Publish(surface->backing, candidate)) {
    RetireDisplayOutput(surface);
    surface->window_closed.store(true, std::memory_order_release);
    return DARWIN_ART_SURFACE_ALLOCATION_FAILED;
  }
  surface->backing = std::move(candidate);
  surface->last_command_buffer = nil;
  const uint32_t new_surface_id = IOSurfaceGetID(surface->backing->surface());
  // The new IOSurface backing has no presented composition generation yet;
  // reset dirty state and rebind the exact display notification after the
  // backing tuple is published, outside the backing mutex.
  if (surface->scanout_owner != nullptr)
    (void)surface->scanout_owner->RebindBacking(new_surface_id);
  if (surface->visible) {
    darwin_art_surface_gpu_configure_embedded(surface, 0, 0, width, height);
    darwin_art_surface_gpu_set_embedded_buffer_extent(surface, width, height);
    darwin_art_surface_gpu_publish_embedded(surface);
    const std::string encoded = std::to_string(IOSurfaceGetID(surface->backing->surface()));
    setenv("DARWIN_ART_HOST_IOSURFACE_ID", encoded.c_str(), 1);
  }
  if (surface->view != nil) {
    [surface->view updateDrawableSize];
    if (update_window && surface->window != nil) {
      const CGFloat scale = surface->view.metalLayer.contentsScale > 0.0
                                ? surface->view.metalLayer.contentsScale
                                : 1.0;
      // NSWindow's setContentSize: preserves the lower-left frame origin.
      // Android relayouts can transiently grow a Surface and then restore its
      // previous size (Chromium does this while creating a tab).  Anchoring
      // that sequence at the lower edge walks the window upward, eventually
      // leaving its title bar off-screen.  Desktop windows conventionally
      // keep their top-left placement across content-size changes, so retain
      // that point explicitly while the Android buffer is reallocated.
      const NSPoint top_left =
          NSMakePoint(NSMinX(surface->window.frame),
                      NSMaxY(surface->window.frame));
      [surface->window setContentSize:
                         NSMakeSize(static_cast<CGFloat>(width) / scale,
                                    static_cast<CGFloat>(height) / scale)];
      [surface->window setFrameTopLeftPoint:top_left];
    }
  }
  return DARWIN_ART_SURFACE_OK;
}

DarwinArtSurfaceResult darwin_art_surface_resize(
    DarwinArtSurface* surface, uint32_t width, uint32_t height) {
  return RunOnMainSync([&] {
    return ResizeSurfaceBackingOnMain(surface, width, height, true, 0, 0);
  });
}

DarwinArtSurfaceResult darwin_art_surface_attach_output(
    DarwinArtSurface* surface, const char* ready_endpoint,
    uint32_t android_width, uint32_t android_height) {
  return RunOnMainSync([&] {
    return AttachDisplayOutput(surface, ready_endpoint, android_width, android_height);
  });
}

DarwinArtSurfaceResult darwin_art_surface_configure_display_extent(
    DarwinArtSurface* surface, uint32_t android_width, uint32_t android_height) {
  return RunOnMainSync([&] {
    return ConfigureDisplayOutputExtent(surface, android_width, android_height);
  });
}

DarwinArtSurfaceResult darwin_art_surface_set_title(
    DarwinArtSurface* surface, const char* title) {
  return RunOnMainSync([&] {
    if (surface == nullptr || title == nullptr || title[0] == '\0') {
      return DARWIN_ART_SURFACE_INVALID_ARGUMENT;
    }
    if (surface->window != nil)
      surface->window.title = darwin_art::window::DecodeSurfaceWindowTitle(title);
    return DARWIN_ART_SURFACE_OK;
  });
}

DarwinArtSurfaceResult darwin_art_surface_set_active_title(const char* title) {
  return RunOnMainSync([&] {
    DarwinArtSurface* surface =
        g_active_gpu_surface.load(std::memory_order_acquire);
    return darwin_art_surface_set_title(surface, title);
  });
}

DarwinArtSurfaceResult darwin_art_surface_set_active_title_utf16(
    const uint16_t* title, size_t length) {
  return RunOnMainSync([&] {
    DarwinArtSurface* surface =
        g_active_gpu_surface.load(std::memory_order_acquire);
    if (surface == nullptr || title == nullptr || length == 0) {
      return DARWIN_ART_SURFACE_INVALID_ARGUMENT;
    }
    NSString* value = [[NSString alloc]
        initWithCharacters:reinterpret_cast<const unichar*>(title)
                    length:length];
    if (value == nil) return DARWIN_ART_SURFACE_INVALID_ARGUMENT;
    if (surface->window != nil) surface->window.title = value;
    return DARWIN_ART_SURFACE_OK;
  });
}

bool darwin_art_surface_get_size(DarwinArtSurface* surface,
                                 uint32_t* width, uint32_t* height) {
  if (surface == nullptr || width == nullptr || height == nullptr) return false;
  const auto backing = surface->backing_owner.Acquire();
  if (!backing) return false;
  *width = backing->physical_width();
  *height = backing->physical_height();
  return true;
}

bool darwin_art_surface_get_logical_size(DarwinArtSurface* surface,
                                         uint32_t* width, uint32_t* height) {
  if (surface == nullptr || width == nullptr || height == nullptr) return false;
  const auto backing = surface->backing_owner.Acquire();
  if (!backing) return false;
  *width = backing->logical_width();
  *height = backing->logical_height();
  return *width > 0 && *height > 0;
}

DarwinArtSurfaceResult darwin_art_surface_update(
    DarwinArtSurface* surface,
    const void* bgra_pixels,
    size_t source_bytes_per_row) {
  if (!IsMainThread()) {
    return DARWIN_ART_SURFACE_NOT_MAIN_THREAD;
  }
  if (surface == nullptr || bgra_pixels == nullptr) {
    return DARWIN_ART_SURFACE_INVALID_ARGUMENT;
  }
  const size_t copy_bytes = static_cast<size_t>(surface->backing->physical_width()) *
                            kBytesPerPixel;
  if (source_bytes_per_row < copy_bytes) {
    return DARWIN_ART_SURFACE_INVALID_ARGUMENT;
  }
  DarwinArtSurfaceProducerMapping mapping{};
  DarwinArtSurfaceResult result =
      darwin_art_surface_map_producer(surface, &mapping);
  if (result != DARWIN_ART_SURFACE_OK) {
    return result;
  }

  const auto* source = static_cast<const uint8_t*>(bgra_pixels);
  auto* destination = static_cast<uint8_t*>(mapping.base_address);
  for (uint32_t row = 0; row < surface->backing->physical_height(); ++row) {
    std::memcpy(destination + static_cast<size_t>(row) * mapping.bytes_per_row,
                source + static_cast<size_t>(row) * source_bytes_per_row,
                copy_bytes);
  }
  return darwin_art_surface_unmap_producer(surface);
}

static DarwinArtSurfaceResult PresentSurfaceOnMain(
    DarwinArtSurface* surface) {
  if (!IsMainThread()) {
    return DARWIN_ART_SURFACE_NOT_MAIN_THREAD;
  }
  if (surface == nullptr) {
    return DARWIN_ART_SURFACE_INVALID_ARGUMENT;
  }
  if (surface->producer_mapped) {
    return DARWIN_ART_SURFACE_PRODUCER_ALREADY_MAPPED;
  }
  if (!surface->visible) {
    return DARWIN_ART_SURFACE_OK;
  }

  @autoreleasepool {
    // A structural composition is submitted by system_server on a different
    // Metal command queue/process. Its completion notification is the acquire
    // boundary for the display consumer. Refresh the IOSurface texture import
    // at that backing epoch so the app process cannot keep sampling an older
    // Metal resource view after WMS moved or detached a layer.
    if (surface->scanout_owner != nullptr &&
        surface->scanout_owner->TakeReimportRequest()) {
      if (IOSurfaceLock(surface->backing->surface(), kIOSurfaceLockReadOnly, nullptr) ==
          kIOReturnSuccess) {
        IOSurfaceUnlock(surface->backing->surface(), kIOSurfaceLockReadOnly, nullptr);
      }
      MTLTextureDescriptor* descriptor =
          [MTLTextureDescriptor texture2DDescriptorWithPixelFormat:
                                    MTLPixelFormatBGRA8Unorm
                                                            width:surface->backing->physical_width()
                                                           height:surface->backing->physical_height()
                                                        mipmapped:NO];
      descriptor.storageMode = MTLStorageModeShared;
      descriptor.usage = MTLTextureUsageShaderRead;
      id<MTLTexture> refreshed =
          [surface->device newTextureWithDescriptor:descriptor
                                          iosurface:surface->backing->surface()
                                              plane:0];
      if (refreshed == nil) {
        return DARWIN_ART_SURFACE_GPU_SUBMISSION_FAILED;
      }
      auto candidate = darwin_art::graphics::RetainSurfaceBacking(
          surface->backing->surface(), refreshed,
          surface->backing->physical_width(), surface->backing->physical_height(),
          surface->backing->logical_width(), surface->backing->logical_height(),
          surface->backing->bytes_per_row(), surface->backing->revision() + 1);
      if (!candidate || !surface->backing_owner.Publish(surface->backing, candidate))
        return DARWIN_ART_SURFACE_GPU_SUBMISSION_FAILED;
      surface->backing = std::move(candidate);
    }
    id<CAMetalDrawable> drawable = [surface->view.metalLayer nextDrawable];
    if (drawable == nil) {
      return DARWIN_ART_SURFACE_DRAWABLE_UNAVAILABLE;
    }
    if (drawable.texture.width < surface->backing->physical_width() ||
        drawable.texture.height < surface->backing->physical_height()) {
      return DARWIN_ART_SURFACE_DRAWABLE_UNAVAILABLE;
    }
    id<MTLCommandBuffer> command_buffer =
        [surface->command_queue commandBuffer];
    if (command_buffer == nil) {
      return DARWIN_ART_SURFACE_GPU_SUBMISSION_FAILED;
    }
    id<MTLBlitCommandEncoder> encoder =
        [command_buffer blitCommandEncoder];
    if (encoder == nil) {
      return DARWIN_ART_SURFACE_GPU_SUBMISSION_FAILED;
    }

    MTLOrigin origin = MTLOriginMake(0, 0, 0);
    MTLSize size = MTLSizeMake(surface->backing->physical_width(), surface->backing->physical_height(), 1);
    [encoder copyFromTexture:surface->backing->texture()
                 sourceSlice:0
                 sourceLevel:0
                sourceOrigin:origin
                  sourceSize:size
                   toTexture:drawable.texture
            destinationSlice:0
            destinationLevel:0
           destinationOrigin:origin];
    auto diagnostic = darwin_art::graphics::ScanoutDiagnosticCapture::Encode(
        surface->backing, surface->device, encoder);
    [encoder endEncoding];
    [command_buffer presentDrawable:drawable];
    [command_buffer commit];
    surface->last_command_buffer = command_buffer;
    diagnostic.Complete(command_buffer);
  }
  return DARWIN_ART_SURFACE_OK;
}

DarwinArtSurfaceResult darwin_art_surface_present(DarwinArtSurface* surface) {
  return RunOnMainSync([&] { return PresentSurfaceOnMain(surface); });
}

static void RunAsyncPresentOnMain(DarwinArtSurface* surface);

static void ScheduleAsyncPresentOnMain(DarwinArtSurface* surface) {
  // The host deliberately keeps AppKit on the process main thread while ART
  // owns a separate Android UI thread. Publish only a pending generation and
  // wake that actor here. PumpSurfaceEventsOnMain is the sole consumer; using
  // a second GCD/CFRunLoop callback would both race teardown through a raw
  // surface pointer and allow a final structural frame to remain queued.
  (void)surface;
  CFRunLoopRef run_loop = CFRunLoopGetMain();
  CFRunLoopWakeUp(run_loop);
}

bool darwin_art_surface_gpu_track_composition_fence(
    DarwinArtSurface* surface, int fence_fd) {
  if (surface == nullptr || fence_fd < 0) return false;
  if (surface->scanout_owner == nullptr) {
    darwin_art::window::CompositionFenceMonitor::CloseRejected(fence_fd);
    return false;
  }
  const bool tracked = surface->scanout_owner->TrackCompositionFence(fence_fd);
  if (std::getenv("DARWIN_ART_DEBUG_SURFACE_TRANSACTIONS") != nullptr) {
    std::fprintf(stderr,
                 "ART SurfaceFlinger: scanout generation submitted surface=%p "
                 "generation=%llu fence=%d accepted=%d\n",
                 surface,
                 static_cast<unsigned long long>(
                     surface->scanout_owner->SubmittedGeneration()), fence_fd,
                 tracked ? 1 : 0);
  }
  return tracked;
}

bool darwin_art_surface_gpu_scanout_ready(DarwinArtSurface* surface) {
  return surface != nullptr && surface->scanout_owner != nullptr &&
         surface->scanout_owner->ScanoutReady();
}

void MaybeLogScanoutStats(DarwinArtSurface* surface) {
  if (surface == nullptr || surface->scanout_owner == nullptr ||
      std::getenv("DARWIN_ART_DEBUG_SURFACE_TRANSACTIONS") == nullptr) {
    return;
  }
  const auto counters = surface->scanout_owner->SnapshotCounters();
  if (counters.requests != 1 && (counters.requests % 600) != 0) return;
  std::fprintf(
      stderr,
      "ART SurfaceFlinger: scanout stats surface=%p requests=%llu "
      "fence_gated=%llu coalesced=%llu dirty_skipped=%llu "
      "present_calls=%llu\n",
      surface, static_cast<unsigned long long>(counters.requests),
      static_cast<unsigned long long>(counters.fence_gated),
      static_cast<unsigned long long>(counters.coalesced),
      static_cast<unsigned long long>(counters.dirty_skipped),
      static_cast<unsigned long long>(counters.present_calls));
}

DarwinArtSurfaceResult darwin_art_surface_present_async(
    DarwinArtSurface* surface) {
  if (surface == nullptr) return DARWIN_ART_SURFACE_INVALID_ARGUMENT;
  if (surface->scanout_owner == nullptr)
    return DARWIN_ART_SURFACE_WINDOW_CLOSED;
  const auto request = surface->scanout_owner->Request(
      surface->embedded_surface_frame.load(std::memory_order_acquire),
      IsMainThread());
  MaybeLogScanoutStats(surface);
  using Request = darwin_art::window::SurfaceScanoutRequestResult;
  switch (request) {
    case Request::kGated:
    case Request::kNoWork:
    case Request::kAlreadyPending:
      return DARWIN_ART_SURFACE_OK;
    case Request::kPresentNow:
      return PresentSurfaceOnMain(surface);
    case Request::kWakeMain:
      ScheduleAsyncPresentOnMain(surface);
      return DARWIN_ART_SURFACE_OK;
    case Request::kClosed:
      return DARWIN_ART_SURFACE_WINDOW_CLOSED;
  }
  return DARWIN_ART_SURFACE_WINDOW_CLOSED;
}

DarwinArtSurfaceResult darwin_art_surface_set_owner_wake(
    DarwinArtSurface* surface,
    DarwinArtSurfaceOwnerWakeCallback callback,
    void* context) {
  if (surface == nullptr) return DARWIN_ART_SURFACE_INVALID_ARGUMENT;
  std::lock_guard<std::mutex> lock(surface->owner_wake_mutex);
  surface->owner_wake_callback = callback;
  surface->owner_wake_context = context;
  return DARWIN_ART_SURFACE_OK;
}

DarwinArtSurfaceResult darwin_art_surface_set_input_sink(
    DarwinArtSurface* surface, const DarwinArtSurfaceInputSink* sink) {
  if (surface == nullptr) return DARWIN_ART_SURFACE_INVALID_ARGUMENT;
  std::shared_ptr<DarwinArtSurfaceInputSinkBinding> replacement;
  if (sink != nullptr) {
    if (sink->version != 1 ||
        sink->size < sizeof(DarwinArtSurfaceInputSink) ||
        sink->pointer == nullptr || sink->key == nullptr ||
        (sink->context != nullptr &&
         (sink->retain_context == nullptr || sink->release_context == nullptr))) {
      return DARWIN_ART_SURFACE_INVALID_ARGUMENT;
    }
    try {
      replacement = MakeDarwinArtSurfaceInputSinkBinding(*sink);
    } catch (...) {
      return DARWIN_ART_SURFACE_ALLOCATION_FAILED;
    }
  }
  std::shared_ptr<DarwinArtSurfaceInputSinkBinding> retired;
  {
    std::lock_guard<std::mutex> lock(surface->input_sink_mutex);
    retired = std::move(surface->input_sink);
    surface->input_sink = std::move(replacement);
  }
  // Release the retired binding only after publication is unlocked; any
  // AppKit snapshot still holds its own shared reference until its callback
  // returns.
  retired.reset();
  return DARWIN_ART_SURFACE_OK;
}

static void RunAsyncPresentOnMain(DarwinArtSurface* surface) {
  if (surface == nullptr || !IsMainThread()) return;
  uint64_t request_generation = 0;
  if (surface->scanout_owner == nullptr ||
      !surface->scanout_owner->BeginDrain(&request_generation)) return;
  if (std::getenv("DARWIN_ART_DEBUG_SURFACE_TRANSACTIONS") != nullptr) {
    std::fprintf(stderr,
                 "ART SurfaceFlinger: AppKit scanout drain surface=%p "
                 "target=%u request=%llu\n",
                 surface,
                 surface->backing->surface() == nullptr
                     ? 0
                     : IOSurfaceGetID(surface->backing->surface()),
                 static_cast<unsigned long long>(request_generation));
  }
  const DarwinArtSurfaceResult result = PresentSurfaceOnMain(surface);
  const bool schedule_next = surface->scanout_owner->CompleteDrain(
      request_generation, result);
  if (result != DARWIN_ART_SURFACE_OK &&
      result != DARWIN_ART_SURFACE_DRAWABLE_UNAVAILABLE) {
    std::fprintf(stderr, "ART Android async scanout failed status=%d\n", result);
  }
  if (schedule_next) {
    ScheduleAsyncPresentOnMain(surface);
  }
}

static DarwinArtSurfaceResult PumpSurfaceEventsOnMain(
    DarwinArtSurface* surface,
    double seconds) {
  if (!IsMainThread()) {
    return DARWIN_ART_SURFACE_NOT_MAIN_THREAD;
  }
  if (surface == nullptr || !std::isfinite(seconds) || seconds < 0.0 ||
      seconds > 30.0) {
    return DARWIN_ART_SURFACE_INVALID_ARGUMENT;
  }
  if (surface->visible && !surface->window.visible) {
    surface->window_closed.store(true, std::memory_order_release);
    [surface->view cancelPointerStream];
    return DARWIN_ART_SURFACE_WINDOW_CLOSED;
  }
  if (seconds == 0.0) {
    return DARWIN_ART_SURFACE_OK;
  }

  @autoreleasepool {
    NSApplication* application = NSApplication.sharedApplication;
    // Window creation establishes the initial key window. Do not reassert
    // activation here: doing so every pump slice steals focus from other
    // macOS applications and, after a close event, can immediately bring the
    // closing Android window back in front. AppKit naturally makes the
    // surface key again when the user explicitly clicks it.
    NSDate* deadline = [NSDate dateWithTimeIntervalSinceNow:seconds];
    while (deadline.timeIntervalSinceNow > 0.0) {
      if (surface->visible && !surface->window.visible) {
        surface->window_closed.store(true, std::memory_order_release);
        [surface->view cancelPointerStream];
        return DARWIN_ART_SURFACE_WINDOW_CLOSED;
      }
      const NSTimeInterval slice_seconds =
          std::min<NSTimeInterval>(0.016, deadline.timeIntervalSinceNow);
      NSDate* slice_deadline =
          [NSDate dateWithTimeIntervalSinceNow:slice_seconds];
      NSEvent* event = [application nextEventMatchingMask:NSEventMaskAny
                                                untilDate:slice_deadline
                                                   inMode:NSDefaultRunLoopMode
                                                  dequeue:YES];
      if (event != nil) {
        [application sendEvent:event];
      }
      [application updateWindows];
      RunAsyncPresentOnMain(surface);
    }
  }
  if (surface->visible && !surface->window.visible) {
    surface->window_closed.store(true, std::memory_order_release);
    [surface->view cancelPointerStream];
    return DARWIN_ART_SURFACE_WINDOW_CLOSED;
  }
  return DARWIN_ART_SURFACE_OK;
}

DarwinArtSurfaceResult darwin_art_surface_pump_events(
    DarwinArtSurface* surface, double seconds) {
  if (!std::isfinite(seconds) || seconds < 0.0 || seconds > 30.0) {
    return DARWIN_ART_SURFACE_INVALID_ARGUMENT;
  }
  // In the split host architecture AppKit is pumped by the process main
  // actor. The ART owner must never synchronously wait for that queue: doing
  // so turns every Looper slice into a dispatch barrier and can stall Chrome
  // for seconds. Preserve the close-state result for worker callers while
  // retaining the full event pump for callers already on AppKit's thread.
  if (!IsMainThread()) {
    if (surface == nullptr) return DARWIN_ART_SURFACE_INVALID_ARGUMENT;
    return surface->window_closed.load(std::memory_order_acquire)
               ? DARWIN_ART_SURFACE_WINDOW_CLOSED
               : DARWIN_ART_SURFACE_OK;
  }
  return RunOnMainSync([&] { return PumpSurfaceEventsOnMain(surface, seconds); });
}

int32_t darwin_art_appkit_pump_events(double seconds) {
  if (!IsMainThread() || !std::isfinite(seconds) || seconds < 0.0 ||
      seconds > 30.0) {
    return DARWIN_ART_SURFACE_INVALID_ARGUMENT;
  }
  @autoreleasepool {
    NSApplication* application = NSApplication.sharedApplication;
    static bool activation_policy_initialized = false;
    if (!activation_policy_initialized) {
      activation_policy_initialized = true;
      // Service children share the app bundle executable, but do not own a
      // desktop application/window. Keep their AppKit/Metal run loop without
      // publishing extra Dock or application-switcher entries.
      const char* presentation =
          std::getenv("DARWIN_ART_DESKTOP_PRESENTATION");
      if (presentation == nullptr || std::strcmp(presentation, "1") != 0) {
        [application setActivationPolicy:NSApplicationActivationPolicyProhibited];
      }
    }
    NSDate* deadline = [NSDate dateWithTimeIntervalSinceNow:seconds];
    while (deadline.timeIntervalSinceNow > 0.0) {
      const NSTimeInterval slice_seconds =
          std::min<NSTimeInterval>(0.016, deadline.timeIntervalSinceNow);
      NSDate* slice_deadline =
          [NSDate dateWithTimeIntervalSinceNow:slice_seconds];
      NSEvent* event = [application nextEventMatchingMask:NSEventMaskAny
                                                untilDate:slice_deadline
                                                   inMode:NSDefaultRunLoopMode
                                                  dequeue:YES];
      if (event != nil) [application sendEvent:event];
      [application updateWindows];
      // The process main thread is an explicit AppKit actor rather than a
      // returned-to-dispatch main queue. It is also the persistent display
      // consumer: a structural SurfaceFlinger composition can complete while
      // the Android owner is blocked indefinitely in MessageQueue.next(), so
      // checking the target notification cannot depend on a short-lived host
      // FrameClock. present_async is cheap when neither a completion
      // generation nor a structural notification is dirty, and executes the
      // Metal scanout directly because this is already AppKit's main actor.
      // ART/JNI remains on the Android UI owner and is never entered here.
      DarwinArtSurface* surface =
          g_active_gpu_surface.load(std::memory_order_acquire);
      if (surface != nullptr) {
        (void)darwin_art_surface_present_async(surface);
      }

      // Drain a latest-wins command that was published by the fence monitor or
      // another non-main producer before this AppKit turn.
      if (surface != nullptr) RunAsyncPresentOnMain(surface);
    }
  }
  return DARWIN_ART_SURFACE_OK;
}

bool darwin_art_surface_close_requested(DarwinArtSurface* surface) {
  if (surface == nullptr) return true;
  return surface->window_closed.load(std::memory_order_acquire);
}

static DarwinArtSurfaceResult DestroySurfaceOnMain(
    DarwinArtSurface* surface) {
  if (!IsMainThread()) {
    return DARWIN_ART_SURFACE_NOT_MAIN_THREAD;
  }
  if (surface == nullptr) {
    return DARWIN_ART_SURFACE_INVALID_ARGUMENT;
  }
  surface->window_closed.store(true, std::memory_order_release);
  RetireDisplayOutput(surface);
  darwin_art_surface_gpu_forget(surface);
  DarwinArtSurface* expected = surface;
  (void)g_active_gpu_surface.compare_exchange_strong(
      expected, nullptr, std::memory_order_acq_rel,
      std::memory_order_acquire);
  DarwinArtSurfaceResult unmap_result = DARWIN_ART_SURFACE_OK;
  if (surface->producer_mapped) {
    unmap_result = darwin_art_surface_unmap_producer(surface);
  }
  [surface->last_command_buffer waitUntilCompleted];
  id<MTLCommandBuffer> last_gpu_command_buffer = nil;
  {
    std::lock_guard<std::mutex> lock(surface->submission_mutex);
    last_gpu_command_buffer = surface->last_gpu_command_buffer;
  }
  [last_gpu_command_buffer waitUntilCompleted];
  const bool gpu_failed =
      surface->last_command_buffer != nil &&
      surface->last_command_buffer.status == MTLCommandBufferStatusError;
  const bool direct_gpu_failed =
      last_gpu_command_buffer != nil &&
      last_gpu_command_buffer.status == MTLCommandBufferStatusError;
  if (std::getenv("DARWIN_ART_DEBUG_SURFACE_TRANSACTIONS") != nullptr) {
    const auto counters = surface->scanout_owner == nullptr
        ? darwin_art::window::SurfaceScanoutCounters{}
        : surface->scanout_owner->SnapshotCounters();
    std::fprintf(
        stderr,
        "ART SurfaceFlinger: scanout stats surface=%p requests=%llu "
        "fence_gated=%llu coalesced=%llu dirty_skipped=%llu "
        "present_calls=%llu\n",
        surface,
        static_cast<unsigned long long>(counters.requests),
        static_cast<unsigned long long>(counters.fence_gated),
        static_cast<unsigned long long>(counters.coalesced),
        static_cast<unsigned long long>(counters.dirty_skipped),
        static_cast<unsigned long long>(counters.present_calls));
  }
  // Teardown was admitted once before entering this main-thread operation.
  // A close observer cannot recursively take ownership of deletion.
  // Retire the exact published root at the existing owner-close point.  The
  // target removes its catalog entry before dispatching the terminal host
  // fact, while preserving the established Close -> orderOut -> close order.
  auto root_target = surface->desktop_root_target;
  if (root_target != nullptr) {
    (void)root_target->Close();
  } else {
    auto root_events = surface->desktop_root_events;
    if (root_events != nullptr) (void)root_events->Close();
  }
  [surface->window orderOut:nil];
  [surface->window close];
  delete surface;
  if (unmap_result != DARWIN_ART_SURFACE_OK) {
    return unmap_result;
  }
  return (gpu_failed || direct_gpu_failed)
             ? DARWIN_ART_SURFACE_GPU_SUBMISSION_FAILED
                    : DARWIN_ART_SURFACE_OK;
}

DarwinArtSurfaceResult darwin_art_surface_destroy(DarwinArtSurface* surface) {
  if (surface == nullptr) return DARWIN_ART_SURFACE_INVALID_ARGUMENT;
  if (surface->scanout_owner == nullptr ||
      !surface->scanout_owner->BeginClose())
    return DARWIN_ART_SURFACE_WINDOW_CLOSED;
  return RunOnMainSync([&] { return DestroySurfaceOnMain(surface); });
}
