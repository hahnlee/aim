#import <AppKit/AppKit.h>
#import <Metal/Metal.h>
#include "compat/window/appkit_content_view.h"

#include <cassert>
#include <cstdio>

int main() {
  @autoreleasepool {
    assert([NSThread isMainThread]);
    id<MTLDevice> device = MTLCreateSystemDefaultDevice();
    assert(device != nil);
    DarwinArtMetalView* view = [[DarwinArtMetalView alloc]
        initWithFrame:NSMakeRect(0, 0, 360, 640)
               device:device pixelSize:CGSizeMake(720, 1280) contentScale:2];
    assert(view != nil && view.ownerSurface == nullptr);
    assert(view.isFlipped && view.acceptsFirstResponder);
    assert(view.layer == view.metalLayer);
    assert(view.metalLayer.device == device);
    assert(view.metalLayer.pixelFormat == MTLPixelFormatBGRA8Unorm);
    assert(!view.metalLayer.framebufferOnly);
    assert(view.metalLayer.colorspace != nullptr);
    CGColorSpaceRef srgb = CGColorSpaceCreateWithName(kCGColorSpaceSRGB);
    assert(srgb != nullptr);
    assert(CFEqual(view.metalLayer.colorspace, srgb));
    CGColorSpaceRelease(srgb);
    assert(view.metalLayer.contentsScale == 2);
    assert(view.metalLayer.drawableSize.width == 720);
    assert(view.metalLayer.drawableSize.height == 1280);
    view.bounds = NSMakeRect(0, 0, 201.25, 100.25);
    [view updateDrawableSize];
    assert(view.metalLayer.drawableSize.width == 403);
    assert(view.metalLayer.drawableSize.height == 201);
    view.metalLayer.contentsScale = 1;
    [view updateDrawableSize];
    assert(view.metalLayer.drawableSize.width == 202);
    assert(view.metalLayer.drawableSize.height == 101);
    // An unbound view has no Android input/focus authority or wake recipient.
    [view cancelPointerStream];
    [view signalOwnerWake];
  }
  std::puts("AppKit content view: actual Metal layer/Retina resize/unbound lifecycle PASS");
}
