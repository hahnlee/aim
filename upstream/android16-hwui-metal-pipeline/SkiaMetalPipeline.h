#pragma once

#include "pipeline/skia/SkiaGpuPipeline.h"

namespace android::uirenderer::skiapipeline {

class SkiaMetalPipeline final : public SkiaGpuPipeline {
public:
    explicit SkiaMetalPipeline(renderthread::RenderThread& thread);
    ~SkiaMetalPipeline() override;
    bool createOrUpdateLayer(RenderNode*, const DamageAccumulator&, ErrorHandler*) override { return false; }
    void renderLayersImpl(const LayerUpdateQueue&, bool) override {}
    void setHardwareBuffer(AHardwareBuffer*) override {}
    bool hasHardwareBuffer() override { return false; }
    renderthread::MakeCurrentResult makeCurrent() override;
    renderthread::Frame getFrame() override;
    renderthread::IRenderPipeline::DrawResult draw(
            const renderthread::Frame&, const SkRect&, const SkRect&, const LightGeometry&,
            LayerUpdateQueue*, const Rect&, bool, const LightInfo&,
            const std::vector<sp<RenderNode>>&, FrameInfoVisualizer*,
            const renderthread::HardwareBufferRenderParams&, std::mutex&) override;
    bool swapBuffers(const renderthread::Frame&, IRenderPipeline::DrawResult&,
                     const SkRect&, FrameInfo*, bool*) override;
    DeferredLayerUpdater* createTextureLayer() override { return nullptr; }
    bool setSurface(ANativeWindow*, renderthread::SwapBehavior) override;
    [[nodiscard]] android::base::unique_fd flush() override;
    void onStop() override {}
    bool isSurfaceReady() override;
    bool isContextReady() override;
    const SkM44& getPixelSnapMatrix() const override {
        static const SkM44 identity = SkM44();
        return identity;
    }
private:
    struct State;
    std::unique_ptr<State> mState;
};

}  // namespace android::uirenderer::skiapipeline
