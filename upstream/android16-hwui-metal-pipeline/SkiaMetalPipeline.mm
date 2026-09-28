#import <AppKit/AppKit.h>
#import <Metal/Metal.h>
#import <QuartzCore/CAMetalLayer.h>

#include "SkiaMetalPipeline.h"
#include "renderthread/RenderThread.h"
#include "include/core/SkSurface.h"
#include "include/gpu/ganesh/GrBackendSurface.h"
#include "include/gpu/ganesh/GrDirectContext.h"
#include "include/gpu/ganesh/mtl/GrMtlBackendContext.h"
#include "include/gpu/ganesh/mtl/GrMtlBackendSurface.h"
#include "include/gpu/ganesh/mtl/GrMtlDirectContext.h"

namespace android::uirenderer::skiapipeline {

@interface DarwinHwuiMetalView : NSView
@end
@implementation DarwinHwuiMetalView
- (BOOL)isFlipped { return YES; }
@end

struct SkiaMetalPipeline::State {
    id<MTLDevice> device = nil;
    id<MTLCommandQueue> queue = nil;
    CAMetalLayer* layer = nil;
    NSWindow* window = nil;
    sk_sp<GrDirectContext> context;
    int width = 360;
    int height = 640;
};

static sk_sp<GrDirectContext> makeContext(id<MTLDevice> device, id<MTLCommandQueue> queue) {
    GrMtlBackendContext backend = {};
    backend.fDevice.retain((__bridge GrMTLHandle)device);
    backend.fQueue.retain((__bridge GrMTLHandle)queue);
    return GrDirectContexts::MakeMetal(backend);
}

SkiaMetalPipeline::SkiaMetalPipeline(renderthread::RenderThread& thread)
        : SkiaGpuPipeline(thread), mState(std::make_unique<State>()) {
    mState->device = MTLCreateSystemDefaultDevice();
    if (mState->device) mState->queue = [mState->device newCommandQueue];
    if (mState->device && mState->queue) {
        mState->context = makeContext(mState->device, mState->queue);
        if (mState->context) thread.setGrContext(mState->context);
    }
}

SkiaMetalPipeline::~SkiaMetalPipeline() {
    if (mState && mState->context) mState->context->flushAndSubmit();
    if (mState && mState->window) [mState->window close];
}

bool SkiaMetalPipeline::setSurface(ANativeWindow* window, renderthread::SwapBehavior) {
    if (!mState->device || !mState->queue) return false;
    if (window) {
        mState->width = std::max(1, ANativeWindow_getWidth(window));
        mState->height = std::max(1, ANativeWindow_getHeight(window));
    }
    if (!mState->layer) {
        mState->layer = [CAMetalLayer layer];
        mState->layer.device = mState->device;
        mState->layer.pixelFormat = MTLPixelFormatBGRA8Unorm;
        mState->layer.drawableSize = CGSizeMake(mState->width, mState->height);
        NSRect rect = NSMakeRect(0, 0, mState->width, mState->height);
        mState->window = [[NSWindow alloc] initWithContentRect:rect
            styleMask:NSWindowStyleMaskBorderless backing:NSBackingStoreBuffered defer:NO];
        DarwinHwuiMetalView* view = [[DarwinHwuiMetalView alloc] initWithFrame:rect];
        view.wantsLayer = YES;
        view.layer = mState->layer;
        mState->window.contentView = view;
        [mState->window orderFront:nil];
        [[NSApplication sharedApplication] updateWindows];
    }
    return mState->context != nullptr;
}

renderthread::MakeCurrentResult SkiaMetalPipeline::makeCurrent() {
    return mState && mState->context ? renderthread::MakeCurrentResult::AlreadyCurrent
                                      : renderthread::MakeCurrentResult::Failed;
}

renderthread::Frame SkiaMetalPipeline::getFrame() {
    return renderthread::Frame(mState ? mState->width : 0, mState ? mState->height : 0, 0);
}

renderthread::IRenderPipeline::DrawResult SkiaMetalPipeline::draw(
        const renderthread::Frame&, const SkRect& dirty, const SkRect&, const LightGeometry&,
        LayerUpdateQueue* layers, const Rect& bounds, bool opaque, const LightInfo&,
        const std::vector<sp<RenderNode>>& nodes, FrameInfoVisualizer*,
        const renderthread::HardwareBufferRenderParams&, std::mutex&) {
    if (!mState || !mState->context || !mState->layer || nodes.empty()) return {false};
    id<CAMetalDrawable> drawable = [mState->layer nextDrawable];
    if (!drawable) return {false};
    GrMtlTextureInfo texture = {};
    texture.fTexture.retain((__bridge GrMTLHandle)drawable.texture);
    auto target = GrBackendRenderTargets::MakeMtl(mState->width, mState->height, texture);
    auto surface = SkSurfaces::WrapBackendRenderTarget(mState->context.get(), target,
            kTopLeft_GrSurfaceOrigin, kBGRA_8888_SkColorType, nullptr, nullptr);
    if (!surface) return {false};
    renderFrame(*layers, dirty, nodes, opaque, bounds, surface, SkMatrix::I());
    mState->context->flushAndSubmit();
    id<MTLCommandBuffer> command = [mState->queue commandBuffer];
    [command presentDrawable:drawable];
    [command commit];
    return {true, renderthread::IRenderPipeline::DrawResult::kUnknownTime,
            android::base::unique_fd(-1)};
}

bool SkiaMetalPipeline::swapBuffers(const renderthread::Frame&, IRenderPipeline::DrawResult&,
                                    const SkRect&, FrameInfo*, bool* requireSwap) {
    if (requireSwap) *requireSwap = false;
    return true;
}

android::base::unique_fd SkiaMetalPipeline::flush() {
    if (mState && mState->context) mState->context->flushAndSubmit();
    return android::base::unique_fd(-1);
}
bool SkiaMetalPipeline::isSurfaceReady() { return mState && mState->layer != nil; }
bool SkiaMetalPipeline::isContextReady() { return mState && mState->context != nullptr; }

}  // namespace android::uirenderer::skiapipeline
