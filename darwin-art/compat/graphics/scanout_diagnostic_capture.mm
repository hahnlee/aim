#include "scanout_diagnostic_capture.h"

#if !__has_feature(objc_arc)
#error "ScanoutDiagnosticCapture requires ARC native resource ownership"
#endif

#import <AppKit/AppKit.h>
#import <QuartzCore/QuartzCore.h>

#include <cstdio>
#include <cstdlib>
#include <limits>
#include <mutex>
#include <utility>

namespace darwin_art::graphics {
namespace {

struct DiagnosticThrottle {
  std::mutex mutex;
  double last_time = 0;
  std::uint64_t sequence = 0;
};

DiagnosticThrottle& Throttle() {
  static DiagnosticThrottle throttle;
  return throttle;
}

const std::string& ConfiguredPrefix() noexcept {
  static const std::string prefix = []() noexcept {
    const char* configured = std::getenv("DARWIN_ART_DIAGNOSTIC_FRAME_PREFIX");
    try {
      return std::string(configured == nullptr ? "" : configured);
    } catch (...) {
      std::fprintf(stderr, "DARWIN_ART diagnostic scanout disabled: prefix allocation failed\n");
      return std::string();
    }
  }();
  return prefix;
}

}  // namespace

ScanoutDiagnosticCapture::ScanoutDiagnosticCapture(
    ScanoutDiagnosticCapture&& other) noexcept
    : backing_(std::move(other.backing_)), pixels_(other.pixels_),
      stride_(other.stride_), width_(other.width_), height_(other.height_),
      prefix_(std::move(other.prefix_)), sequence_(other.sequence_) {
  other.pixels_ = nil;
}

ScanoutDiagnosticCapture ScanoutDiagnosticCapture::Encode(
    SurfaceBackingOwner::Handle backing, id<MTLDevice> device,
    id<MTLBlitCommandEncoder> encoder) noexcept {
  const std::string& configured_prefix = ConfiguredPrefix();
  if (configured_prefix.empty() ||
      backing == nullptr || device == nil || encoder == nil) {
    return {};
  }
  std::string prefix;
  try {
    prefix = configured_prefix;
  } catch (...) {
    std::fprintf(stderr,
                 "DARWIN_ART diagnostic scanout disabled: prefix allocation failed\n");
    return {};
  }

  const std::uint32_t width = backing->physical_width();
  const std::uint32_t height = backing->physical_height();
  if (width == 0 || height == 0 ||
      static_cast<std::size_t>(width) >
          (std::numeric_limits<std::size_t>::max() - 255) / 4) {
    return {};
  }
  const std::size_t stride =
      (static_cast<std::size_t>(width) * 4 + 255) & ~std::size_t(255);
  if (height > 0 && stride > std::numeric_limits<std::size_t>::max() / height) {
    return {};
  }

  const double now = CACurrentMediaTime();
  std::uint64_t sequence = 0;
  {
    DiagnosticThrottle& throttle = Throttle();
    std::lock_guard<std::mutex> lock(throttle.mutex);
    if (now - throttle.last_time < 2.0) {
      return {};
    }
    // Preserve the existing behavior: throttle admission is consumed before
    // allocation, so repeated allocation failures cannot busy-loop.
    throttle.last_time = now;
    sequence = ++throttle.sequence;
  }

  id<MTLBuffer> pixels = [device
      newBufferWithLength:stride * height
                    options:MTLResourceStorageModeShared];
  if (pixels == nil) {
    std::fprintf(stderr,
                 "DARWIN_ART diagnostic scanout frame allocation failed\n");
    return {};
  }
  const MTLOrigin origin = MTLOriginMake(0, 0, 0);
  const MTLSize size = MTLSizeMake(width, height, 1);
  [encoder copyFromTexture:backing->texture()
               sourceSlice:0
               sourceLevel:0
              sourceOrigin:origin
                sourceSize:size
                   toBuffer:pixels
          destinationOffset:0
          destinationBytesPerRow:stride
          destinationBytesPerImage:stride * height];
  return ScanoutDiagnosticCapture(
      std::move(backing), pixels, stride, width, height,
      std::move(prefix), sequence);
}

void ScanoutDiagnosticCapture::Complete(
    id<MTLCommandBuffer> command_buffer) noexcept {
  if (pixels_ == nil || command_buffer == nil) {
    return;
  }
  id<MTLBuffer> pixels = pixels_;
  pixels_ = nil;
  [command_buffer waitUntilCompleted];
  // The command has finished sampling this exact backing. Release the
  // retained snapshot before CPU-side PNG work; Complete is the terminal job
  // transition and can be called only once for an active job.
  backing_.reset();
  if (command_buffer.status != MTLCommandBufferStatusCompleted) {
    std::fprintf(stderr,
                 "DARWIN_ART diagnostic scanout frame command failed\n");
    return;
  }

  @autoreleasepool {
    NSBitmapImageRep* bitmap = [[NSBitmapImageRep alloc]
        initWithBitmapDataPlanes:nullptr
                      pixelsWide:width_
                      pixelsHigh:height_
                   bitsPerSample:8
                 samplesPerPixel:4
                        hasAlpha:YES
                        isPlanar:NO
                  colorSpaceName:NSDeviceRGBColorSpace
                     bitmapFormat:0
                      bytesPerRow:static_cast<NSInteger>(width_) * 4
                     bitsPerPixel:32];
    if (bitmap == nil) {
      std::fprintf(stderr,
                   "DARWIN_ART diagnostic scanout bitmap allocation failed\n");
      return;
    }
    const auto* source = static_cast<const std::uint8_t*>(pixels.contents);
    std::uint8_t* destination = bitmap.bitmapData;
    for (std::uint32_t y = 0; y < height_; ++y) {
      for (std::uint32_t x = 0; x < width_; ++x) {
        const std::uint8_t* bgra = source + y * stride_ + x * 4;
        std::uint8_t* rgba = destination + (y * width_ + x) * 4;
        rgba[0] = bgra[2];
        rgba[1] = bgra[1];
        rgba[2] = bgra[0];
        rgba[3] = bgra[3];
      }
    }
    NSString* path = [NSString stringWithFormat:@"%s-%06llu.png",
                      prefix_.c_str(),
                      static_cast<unsigned long long>(sequence_)];
    NSData* png = [bitmap representationUsingType:NSBitmapImageFileTypePNG
                                         properties:@{}];
    const bool written = path != nil && png != nil &&
        [png writeToFile:path atomically:YES];
    const char* path_utf8 = path == nil ? nullptr : path.UTF8String;
    std::fprintf(stderr,
                 "DARWIN_ART diagnostic scanout frame=%llu size=%ux%u "
                 "written=%d path=%s\n",
                 static_cast<unsigned long long>(sequence_), width_, height_,
                 written, path_utf8 == nullptr ? "<invalid-path>" : path_utf8);
  }
}

}  // namespace darwin_art::graphics
