#include "compat/graphics/metal_display_backing.h"
#import <Foundation/Foundation.h>

#include <cassert>
#include <cstdio>
#include <initializer_list>
#include <utility>

using namespace darwin_art::graphics;

// Test-only Metal failure boundary; the provider still creates a real IOSurface.
@interface FailingTextureDevice : NSObject {
 @public
  unsigned calls_;
}
- (id<MTLTexture>)newTextureWithDescriptor:(MTLTextureDescriptor*)descriptor
                               iosurface:(IOSurfaceRef)surface
                                   plane:(NSUInteger)plane;
@end
@implementation FailingTextureDevice
- (id<MTLTexture>)newTextureWithDescriptor:(MTLTextureDescriptor*)descriptor
                               iosurface:(IOSurfaceRef)surface
                                   plane:(NSUInteger)plane {
  ++calls_;
  assert(surface != nullptr && plane == 0 && descriptor.width == 360 &&
         descriptor.height == 640 && descriptor.pixelFormat == MTLPixelFormatBGRA8Unorm);
  return nil;
}
@end

int main() {
  @autoreleasepool {
    id<MTLDevice> device = MTLCreateSystemDefaultDevice();
    assert(device != nil);  // GPU provider acceptance must not skip to CPU.
    MetalDisplayBacking backing;
    assert(AllocateMetalDisplayBacking(device, 720, 1280, &backing) ==
           MetalDisplayBackingResult::kOk);
    assert(backing.surface() != nullptr && backing.texture() != nil);
    assert(IOSurfaceGetWidth(backing.surface()) == 720 &&
           IOSurfaceGetHeight(backing.surface()) == 1280);
    assert(backing.texture().width == 720 && backing.texture().height == 1280 &&
           backing.texture().pixelFormat == MTLPixelFormatBGRA8Unorm &&
           backing.texture().storageMode == MTLStorageModeShared &&
           backing.texture().usage == MTLTextureUsageShaderRead);
    assert(backing.bytes_per_row() >= 720 * 4 && backing.bytes_per_row() % 64 == 0 &&
           IOSurfaceGetAllocSize(backing.surface()) >= backing.bytes_per_row() * 1280);
    IOSurfaceRef original = backing.surface();
    id<MTLTexture> original_texture = backing.texture();
    for (auto extent : {0u, 16385u}) {
      assert(AllocateMetalDisplayBacking(device, extent, 640, &backing) ==
             MetalDisplayBackingResult::kInvalidArgument);
      assert(AllocateMetalDisplayBacking(device, 360, extent, &backing) ==
             MetalDisplayBackingResult::kInvalidArgument);
      assert(backing.surface() == original && backing.texture() == original_texture);
    }
    assert(AllocateMetalDisplayBacking(nil, 360, 640, &backing) ==
           MetalDisplayBackingResult::kInvalidArgument);
    assert(AllocateMetalDisplayBacking(device, 360, 640, nullptr) ==
           MetalDisplayBackingResult::kInvalidArgument);
    assert(backing.surface() == original);
    FailingTextureDevice* failing = [FailingTextureDevice new];
    assert(AllocateMetalDisplayBacking((id<MTLDevice>)failing, 360, 640, &backing) ==
           MetalDisplayBackingResult::kMetalUnavailable);
    assert(failing->calls_ == 1 && backing.surface() == original &&
           backing.texture() == original_texture);

    MetalDisplayBacking moved(std::move(backing));
    assert(backing.surface() == nullptr && backing.texture() == nil &&
           backing.bytes_per_row() == 0 && moved.surface() == original);
    MetalDisplayBacking replacement;
    assert(AllocateMetalDisplayBacking(device, 360, 640, &replacement) ==
           MetalDisplayBackingResult::kOk);
    replacement = std::move(moved);
    assert(moved.surface() == nullptr && moved.texture() == nil &&
           replacement.surface() == original && replacement.texture() == original_texture);
    auto* self = &replacement;
    replacement = std::move(*self);
    assert(replacement.surface() == original && replacement.texture() == original_texture);
    // A successful tuple commit transfers the CF +1, not a borrowed reference.
    id<MTLTexture> committed_texture = replacement.texture();
    IOSurfaceRef committed = replacement.ReleaseSurface();
    assert(committed == original && replacement.surface() == nullptr);
    replacement = MetalDisplayBacking{};
    assert(IOSurfaceGetWidth(committed) == 720 && committed_texture.width == 720);
    original_texture = nil;
    committed_texture = nil;
    CFRelease(committed);
  }
  std::puts("Metal display backing: real GPU/Retina extent/stride/failure/move/handoff PASS");
}
