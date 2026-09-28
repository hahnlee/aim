#import <AppKit/AppKit.h>
#import <Metal/Metal.h>
#import <QuartzCore/CAMetalLayer.h>

#include <cstdio>
#include <cstdlib>
#include <memory>

#include "include/core/SkColor.h"
#include "include/core/SkSurface.h"
#include "include/gpu/ganesh/GrBackendSurface.h"
#include "include/gpu/ganesh/GrDirectContext.h"
#include "include/gpu/ganesh/mtl/GrMtlDirectContext.h"
#include "include/gpu/ganesh/mtl/GrMtlBackendSurface.h"
#include "include/gpu/ganesh/mtl/GrMtlBackendContext.h"
#include "include/gpu/ganesh/mtl/SkSurfaceMetal.h"
#define private public
#include "RenderNode.h"
#undef private
#include "SkiaRecordingCanvas.h"
#include "pipeline/skia/RenderNodeDrawable.h"
#include "hwui/Paint.h"

@interface HwuiMetalReplayView : NSView
@end

@implementation HwuiMetalReplayView
- (BOOL)isFlipped { return YES; }
@end

namespace {

constexpr int kWidth = 360;
constexpr int kHeight = 640;

struct SurfaceFrame {
    sk_sp<SkSurface> surface;
    id<CAMetalDrawable> drawable;
};

SurfaceFrame beginFrame(CAMetalLayer* layer, GrDirectContext* context) {
    id<CAMetalDrawable> drawable = [layer nextDrawable];
    if (!drawable) return {};
    GrMtlTextureInfo texture_info;
    texture_info.fTexture.retain((__bridge GrMTLHandle)drawable.texture);
    GrBackendRenderTarget render_target = GrBackendRenderTargets::MakeMtl(
            kWidth, kHeight, texture_info);
    sk_sp<SkSurface> surface = SkSurfaces::WrapBackendRenderTarget(
            context, render_target, kTopLeft_GrSurfaceOrigin,
            kBGRA_8888_SkColorType, nullptr, nullptr);
    if (!surface) return {};
    return {std::move(surface), drawable};
}

void submitFrame(id<MTLCommandQueue> queue, id<CAMetalDrawable> drawable,
                 GrDirectContext* context) {
    context->flushAndSubmit();
    id<MTLCommandBuffer> command = [queue commandBuffer];
    [command presentDrawable:drawable];
    [command commit];
    [command waitUntilCompleted];
}

bool recordAndReplay(GrDirectContext* context, CAMetalLayer* layer, id<MTLCommandQueue> queue,
                     int frame, bool pressed) {
    auto frame_target = beginFrame(layer, context);
    if (!frame_target.surface) return false;

    android::uirenderer::skiapipeline::SkiaRecordingCanvas recorder(nullptr, kWidth, kHeight);
    android::Paint background;
    background.setColor(SK_ColorWHITE);
    recorder.drawColor(SK_ColorWHITE, SkBlendMode::kSrcOver);

    android::Paint card;
    card.setAntiAlias(true);
    card.setColor(SkColorSetARGB(255, 245, 243, 250));
    static_cast<android::Canvas&>(recorder).drawRoundRect(24, 96, 336, 248, 28, 28, card);

    android::Paint button;
    button.setAntiAlias(true);
    button.setColor(SkColorSetARGB(255, 103, 80, 164));
    static_cast<android::Canvas&>(recorder).drawRoundRect(48, 292, 312, 372, 40, 40, button);

    android::Paint ripple;
    ripple.setAntiAlias(true);
    ripple.setColor(pressed ? SkColorSetARGB(70, 255, 255, 255)
                           : SkColorSetARGB(0, 255, 255, 255));
    static_cast<android::Canvas&>(recorder).drawCircle(
            180, 332, pressed ? 62.0f + frame * 3.0f : 0.0f, ripple);

    auto display_list = recorder.finishRecording();
    if (!display_list || display_list->isEmpty()) return false;
    android::sp<android::uirenderer::RenderNode> node =
            new android::uirenderer::RenderNode();
    node->mDisplayList = android::uirenderer::DisplayList(std::move(display_list));
    node->mValid = true;
    node->mProperties.setLeftTopRightBottom(0, 0, kWidth, kHeight);
    android::uirenderer::skiapipeline::RenderNodeDrawable root(
            node.get(), frame_target.surface->getCanvas(), false);
    root.forceDraw(frame_target.surface->getCanvas());
    submitFrame(queue, frame_target.drawable, context);
    return true;
}

}  // namespace

int main() {
    @autoreleasepool {
        NSApplication* app = [NSApplication sharedApplication];
        [app setActivationPolicy:NSApplicationActivationPolicyAccessory];
        id<MTLDevice> device = MTLCreateSystemDefaultDevice();
        if (!device) {
            std::fprintf(stderr, "hwui-metal-replay: no Metal device\n");
            return 2;
        }
        id<MTLCommandQueue> queue = [device newCommandQueue];
        CAMetalLayer* layer = [CAMetalLayer layer];
        layer.device = device;
        layer.pixelFormat = MTLPixelFormatBGRA8Unorm;
        layer.drawableSize = CGSizeMake(kWidth, kHeight);

        NSRect rect = NSMakeRect(0, 0, kWidth, kHeight);
        NSWindow* window = [[NSWindow alloc] initWithContentRect:rect
            styleMask:NSWindowStyleMaskBorderless backing:NSBackingStoreBuffered defer:NO];
        HwuiMetalReplayView* view = [[HwuiMetalReplayView alloc] initWithFrame:rect];
        view.wantsLayer = YES;
        view.layer = layer;
        window.contentView = view;
        [window orderFront:nil];
        [app updateWindows];

        GrMtlBackendContext backend = {};
        backend.fDevice.retain((__bridge GrMTLHandle)device);
        backend.fQueue.retain((__bridge GrMTLHandle)queue);
        sk_sp<GrDirectContext> context = GrDirectContexts::MakeMetal(backend);
        if (!context) {
            std::fprintf(stderr, "hwui-metal-replay: MakeMetal failed\n");
            return 2;
        }

        constexpr int kFrames = 8;
        for (int frame = 0; frame < kFrames; ++frame) {
            if (!recordAndReplay(context.get(), layer, queue, frame, frame >= 2)) {
                std::fprintf(stderr, "hwui-metal-replay: frame %d failed\n", frame);
                return 2;
            }
        }
        std::printf("hwui-metal-replay: frames=%d recording=1 rendernode=1 ripple=1 "
                    "cpu-readback=0 full-frame-blits=0 drawable-direct=1\n", kFrames);
        return 0;
    }
}
