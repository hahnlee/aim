#include "compat/graphics/borrowed_texture_backing.h"
#include "compat/graphics/metal_display_backing.h"
#include "include/core/SkCanvas.h"
#include "include/core/SkColorSpace.h"
#include "include/core/SkImage.h"
#include "include/core/SkSurface.h"
#include "include/gpu/ganesh/GrDirectContext.h"
#include "include/gpu/ganesh/GrBackendSurface.h"
#include "include/gpu/ganesh/SkImageGanesh.h"
#include "include/gpu/ganesh/SkSurfaceGanesh.h"
#include "include/gpu/ganesh/mtl/GrMtlBackendContext.h"
#include "include/gpu/ganesh/mtl/GrMtlBackendSurface.h"
#include "include/gpu/ganesh/mtl/GrMtlDirectContext.h"
#include <cassert>
#include <cstdio>

using namespace darwin_art::graphics;

int main() {
  @autoreleasepool {
    id<MTLDevice> device = MTLCreateSystemDefaultDevice();
    assert(device != nil);
    id<MTLCommandQueue> queue = [device newCommandQueue];
    assert(queue != nil);
    GrMtlBackendContext backend{};
    backend.fDevice.retain((__bridge GrMTLHandle)device);
    backend.fQueue.retain((__bridge GrMTLHandle)queue);
    auto context = GrDirectContexts::MakeMetal(backend);
    assert(context);
    MetalDisplayBacking allocation;
    assert(AllocateMetalDisplayBacking(device, 64, 48, &allocation) == MetalDisplayBackingResult::kOk);
    auto snapshot = RetainSurfaceBacking(allocation.surface(), allocation.texture(),
        64, 48, 32, 24, allocation.bytes_per_row(), 1);
    assert(snapshot);
    SurfaceBackingOwner owner;
    assert(owner.Publish(nullptr, snapshot));
    std::weak_ptr<const SurfaceBackingHandle> weak = snapshot;
    GrMtlTextureInfo texture{};
    texture.fTexture.retain((__bridge GrMTLHandle)snapshot->texture());
    auto borrowed = GrBackendTextures::MakeMtl(64, 48, skgpu::Mipmapped::kNo, texture);
    auto image = SkImages::BorrowTextureFrom(context.get(), borrowed,
        kTopLeft_GrSurfaceOrigin, kBGRA_8888_SkColorType, kPremul_SkAlphaType,
        SkColorSpace::MakeSRGB(), BorrowedTextureBacking::Release,
        new BorrowedTextureBacking{snapshot});
    assert(image);
    borrowed = GrBackendTexture{};
    texture = GrMtlTextureInfo{};
    allocation.Reset();
    assert(owner.Close());
    snapshot.reset();
    assert(!weak.expired());
    auto target = SkSurfaces::RenderTarget(context.get(), skgpu::Budgeted::kNo,
        SkImageInfo::MakeN32Premul(64, 48));
    assert(target);
    target->getCanvas()->drawImage(image, 0, 0);
    context->flushAndSubmit(target.get(), GrSyncCpu::kYes);
    assert(!weak.expired());
    image.reset();
    target.reset();
    context->flushAndSubmit(GrSyncCpu::kYes);
    context->freeGpuResources();
    context.reset();
    assert(weak.expired());

    // Pinned factory invokes the same production callback even on failure.
    assert(AllocateMetalDisplayBacking(device, 64, 48, &allocation) == MetalDisplayBackingResult::kOk);
    auto failure_snapshot = RetainSurfaceBacking(allocation.surface(), allocation.texture(),
        64, 48, 32, 24, allocation.bytes_per_row(), 2);
    assert(failure_snapshot);
    std::weak_ptr<const SurfaceBackingHandle> failure_weak = failure_snapshot;
    allocation.Reset();
    GrMtlTextureInfo failure_texture{};
    failure_texture.fTexture.retain((__bridge GrMTLHandle)failure_snapshot->texture());
    auto failure_backend = GrBackendTextures::MakeMtl(64, 48, skgpu::Mipmapped::kNo, failure_texture);
    auto failed = SkImages::BorrowTextureFrom(nullptr, failure_backend,
        kTopLeft_GrSurfaceOrigin, kBGRA_8888_SkColorType, kPremul_SkAlphaType,
        SkColorSpace::MakeSRGB(), BorrowedTextureBacking::Release,
        new BorrowedTextureBacking{failure_snapshot});
    assert(!failed);
    failure_snapshot.reset();
    assert(failure_weak.expired());
    std::puts("Skia borrowed backing: real Metal draw/retention after owner Close/release after GPU retirement PASS");
  }
}
