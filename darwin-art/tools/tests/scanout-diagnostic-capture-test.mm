#import <AppKit/AppKit.h>
#include "graphics/metal_display_backing.h"
#include "graphics/scanout_diagnostic_capture.h"
#include <cassert>
#include <cstdio>
#include <cstdlib>
#include <cstring>

using namespace darwin_art::graphics;

int main(int argc, char** argv) {
  assert(argc == 3);
  @autoreleasepool {
    const bool disabled = std::strcmp(argv[1], "disabled") == 0;
    const bool failure = std::strcmp(argv[1], "failure") == 0;
    if (disabled) unsetenv("DARWIN_ART_DIAGNOSTIC_FRAME_PREFIX");
    else assert(setenv("DARWIN_ART_DIAGNOSTIC_FRAME_PREFIX", argv[2], 1) == 0);
    id<MTLDevice> device = MTLCreateSystemDefaultDevice();
    assert(device != nil);
    id<MTLCommandQueue> queue = [device newCommandQueue];
    assert(queue != nil);
    MetalDisplayBacking allocation;
    assert(AllocateMetalDisplayBacking(device, 720, 1280, &allocation) ==
           MetalDisplayBackingResult::kOk);
    assert(IOSurfaceLock(allocation.surface(), 0, nullptr) == kIOReturnSuccess);
    auto* bytes = static_cast<unsigned char*>(IOSurfaceGetBaseAddress(allocation.surface()));
    for (unsigned y = 0; y < 1280; ++y) {
      for (unsigned x = 0; x < 720; ++x) {
        auto* pixel = bytes + y * allocation.bytes_per_row() + x * 4;
        pixel[0] = x == 0 ? 41 : 7;
        pixel[1] = y == 0 ? 83 : 19;
        pixel[2] = 231;
        pixel[3] = 255;
      }
    }
    assert(IOSurfaceUnlock(allocation.surface(), 0, nullptr) == kIOReturnSuccess);
    auto snapshot = RetainSurfaceBacking(allocation.surface(), allocation.texture(),
        720, 1280, 360, 640, allocation.bytes_per_row(), 1);
    assert(snapshot);
    std::weak_ptr<const SurfaceBackingHandle> weak = snapshot;
    SurfaceBackingOwner owner;
    assert(owner.Publish({}, snapshot));
    id<MTLCommandBuffer> command = [queue commandBuffer];
    id<MTLBlitCommandEncoder> encoder = [command blitCommandEncoder];
    auto capture = ScanoutDiagnosticCapture::Encode(snapshot, device, encoder);
    assert(capture.valid() != disabled);
    auto throttled = ScanoutDiagnosticCapture::Encode(snapshot, device, encoder);
    assert(!throttled.valid());
    throttled.Complete(nil);
    auto moved = std::move(capture);
    assert(!capture.valid());
    capture.Complete(nil);
    [encoder endEncoding];
    [command commit];
    owner.Close();
    allocation.Reset();
    snapshot.reset();
    if (!disabled) assert(!weak.expired());
    moved.Complete(command);
    assert(!moved.valid());
    moved.Complete(command);
    assert(weak.expired());
    NSString* path = [NSString stringWithFormat:@"%s-000001.png", argv[2]];
    if (disabled || failure) {
      assert(![[NSFileManager defaultManager] fileExistsAtPath:path]);
    } else {
      NSBitmapImageRep* image = [[NSBitmapImageRep alloc]
          initWithData:[NSData dataWithContentsOfFile:path]];
      assert(image != nil && image.pixelsWide == 720 && image.pixelsHigh == 1280);
      for (unsigned y : {0u, 1279u}) for (unsigned x : {0u, 719u}) {
        NSUInteger pixel[4] = {};
        [image getPixel:pixel atX:x y:y];
        assert(pixel[0] == 231 && pixel[1] == (y == 0 ? 83u : 19u) &&
               pixel[2] == (x == 0 ? 41u : 7u) && pixel[3] == 255);
      }
    }
    std::fprintf(stderr, "capture %s: genuine Metal physical extent/pixels/lifetime PASS\n", argv[1]);
  }
}
