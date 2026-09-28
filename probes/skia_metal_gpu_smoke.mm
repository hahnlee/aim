#import <AppKit/AppKit.h>
#import <Metal/Metal.h>
#import <QuartzCore/CAMetalLayer.h>

#include <cstdint>
#include <iostream>

#include "include/core/SkCanvas.h"
#include "include/core/SkColor.h"
#include "include/core/SkColorSpace.h"
#include "include/core/SkPaint.h"
#include "include/core/SkSurface.h"
#include "include/gpu/ganesh/GrContextOptions.h"
#include "include/gpu/ganesh/GrBackendSurface.h"
#include "include/gpu/ganesh/GrDirectContext.h"
#include "include/gpu/ganesh/mtl/GrMtlBackendSurface.h"
#include "include/gpu/ganesh/mtl/GrMtlBackendContext.h"
#include "include/gpu/ganesh/mtl/GrMtlDirectContext.h"
#include "include/gpu/ganesh/mtl/SkSurfaceMetal.h"

@interface SkiaMetalSmokeView : NSView
@end

@implementation SkiaMetalSmokeView
- (BOOL)isFlipped {
  return YES;
}
@end

namespace {

constexpr int kWidth = 360;
constexpr int kHeight = 640;

}  // namespace

int main() {
  @autoreleasepool {
    id<MTLDevice> device = MTLCreateSystemDefaultDevice();
    if (device == nil) {
      std::cerr << "skia-metal-gpu: no-metal-device\n";
      return 1;
    }
    id<MTLCommandQueue> queue = [device newCommandQueue];
    if (queue == nil) {
      std::cerr << "skia-metal-gpu: no-command-queue\n";
      return 2;
    }
    CAMetalLayer* layer = [CAMetalLayer layer];
    layer.device = device;
    layer.pixelFormat = MTLPixelFormatBGRA8Unorm;
    layer.framebufferOnly = YES;
    layer.drawableSize = CGSizeMake(kWidth, kHeight);
    NSApplication* application = NSApplication.sharedApplication;
    [application setActivationPolicy:NSApplicationActivationPolicyRegular];
    NSWindow* window = [[NSWindow alloc]
        initWithContentRect:NSMakeRect(0, 0, kWidth, kHeight)
                  styleMask:NSWindowStyleMaskTitled
                    backing:NSBackingStoreBuffered
                      defer:NO];
    SkiaMetalSmokeView* view = [[SkiaMetalSmokeView alloc]
        initWithFrame:NSMakeRect(0, 0, kWidth, kHeight)];
    view.wantsLayer = YES;
    view.layer = layer;
    layer.frame = view.bounds;
    layer.contentsScale = 1.0;
    view.layerContentsRedrawPolicy = NSViewLayerContentsRedrawDuringViewResize;
    window.contentView = view;
    [application finishLaunching];
    [window makeKeyAndOrderFront:nil];
    [application activateIgnoringOtherApps:YES];
    [application updateWindows];
    [window displayIfNeeded];

    GrMtlBackendContext backend = {};
    backend.fDevice.retain((__bridge GrMTLHandle)device);
    backend.fQueue.retain((__bridge GrMTLHandle)queue);
    sk_sp<GrDirectContext> context = GrDirectContexts::MakeMetal(backend);
    if (!context) {
      std::cerr << "skia-metal-gpu: MakeMetal failed\n";
      return 3;
    }

    for (int frame = 0; frame < 8; ++frame) {
      id<CAMetalDrawable> drawable = [layer nextDrawable];
      if (drawable == nil) {
        std::cerr << "skia-metal-gpu: drawable surface failed\n";
        return 4;
      }
      GrMtlTextureInfo texture_info;
      texture_info.fTexture.retain((__bridge GrMTLHandle)drawable.texture);
      GrBackendRenderTarget render_target = GrBackendRenderTargets::MakeMtl(
          kWidth, kHeight, texture_info);
      sk_sp<SkSurface> surface = SkSurfaces::WrapBackendRenderTarget(
          context.get(), render_target, kTopLeft_GrSurfaceOrigin,
          kBGRA_8888_SkColorType, nullptr, nullptr);
      if (!surface) {
        std::cerr << "skia-metal-gpu: backend surface failed\n";
        return 4;
      }
      SkCanvas* canvas = surface->getCanvas();
      canvas->clear(SkColorSetARGB(255, 20, 24, 32));
      SkPaint paint;
      paint.setAntiAlias(true);
      paint.setColor(SkColorSetARGB(255, 103, 80, 164));
      canvas->drawCircle(180.0f, 320.0f, 40.0f + frame * 8.0f, paint);
      context->flushAndSubmit();

      id<MTLCommandBuffer> command = [queue commandBuffer];
      if (command == nil) {
        std::cerr << "skia-metal-gpu: command buffer failed\n";
        return 5;
      }
      [command presentDrawable:drawable];
      [command commit];
      [command waitUntilCompleted];
      if (command.status == MTLCommandBufferStatusError) {
        std::cerr << "skia-metal-gpu: command failed\n";
        return 6;
      }
    }
    context->flushAndSubmit();
    context->abandonContext();
    [window orderOut:nil];
    [window close];
    std::cout << "skia-metal-gpu: frames=8 cpu-readback=0 full-frame-blits=0 drawable-direct=1\n";
  }
  return 0;
}
