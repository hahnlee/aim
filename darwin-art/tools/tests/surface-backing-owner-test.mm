#import <Metal/Metal.h>

#include "graphics/metal_display_backing.h"
#include "graphics/surface_backing_owner.h"

#include <cassert>
#include <cstdint>
#include <memory>
#include <atomic>
#include <cstdio>
#include <thread>
#include <vector>

namespace {

using darwin_art::graphics::MetalDisplayBacking;
using darwin_art::graphics::MetalDisplayBackingResult;
using darwin_art::graphics::SurfaceBackingHandle;
using darwin_art::graphics::SurfaceBackingOwner;

SurfaceBackingOwner::Handle MakeHandle(
    IOSurfaceRef surface, id<MTLTexture> texture,
    std::uint32_t width, std::uint32_t height, std::size_t stride,
    std::uint64_t revision) {
  return std::make_shared<const SurfaceBackingHandle>(
      surface, texture, width, height, width / 2, height / 2, stride,
      revision);
}

SurfaceBackingOwner::Handle AllocateHandle(
    id<MTLDevice> device, std::uint32_t width, std::uint32_t height,
    std::uint64_t revision) {
  MetalDisplayBacking backing;
  assert(AllocateMetalDisplayBacking(device, width, height, &backing) ==
         MetalDisplayBackingResult::kOk);
  IOSurfaceRef surface = backing.surface();
  id<MTLTexture> texture = backing.texture();
  const std::size_t stride = backing.bytes_per_row();
  auto handle = MakeHandle(surface, texture, width, height, stride, revision);
  IOSurfaceRef transferred = backing.ReleaseSurface();
  assert(transferred == surface);
  CFRelease(transferred);
  return handle;
}

SurfaceBackingOwner::Handle ReimportHandle(
    id<MTLDevice> device, const SurfaceBackingOwner::Handle& prior,
    std::uint64_t revision) {
  MTLTextureDescriptor* descriptor =
      [MTLTextureDescriptor texture2DDescriptorWithPixelFormat:
                                MTLPixelFormatBGRA8Unorm
                                                        width:prior->physical_width()
                                                       height:prior->physical_height()
                                                    mipmapped:NO];
  descriptor.storageMode = MTLStorageModeShared;
  descriptor.usage = MTLTextureUsageShaderRead;
  id<MTLTexture> texture = [device newTextureWithDescriptor:descriptor
                                                    iosurface:prior->surface()
                                                        plane:0];
  assert(texture != nil);
  return MakeHandle(prior->surface(), texture,
                    prior->physical_width(), prior->physical_height(),
                    prior->bytes_per_row(), revision);
}

}  // namespace

int main() {
  @autoreleasepool {
    id<MTLDevice> device = MTLCreateSystemDefaultDevice();
    assert(device != nil);  // Missing GPU is a failure, never a skipped PASS.

    // Match production reimport: an ARC caller's temporary new texture must
    // outlive its scope through the separately compiled backing provider.
    // The actual product-object mode catches a non-ARC producer mismatch.
    auto stable = AllocateHandle(device, 64, 48, 1);
    SurfaceBackingOwner::Handle refreshed;
    __weak id<MTLTexture> observed = nil;
    @autoreleasepool {
      MTLTextureDescriptor* descriptor = [MTLTextureDescriptor
          texture2DDescriptorWithPixelFormat:MTLPixelFormatBGRA8Unorm
          width:64 height:48 mipmapped:NO];
      descriptor.storageMode = MTLStorageModeShared;
      descriptor.usage = MTLTextureUsageShaderRead;
      id<MTLTexture> texture = [device newTextureWithDescriptor:descriptor
          iosurface:stable->surface() plane:0];
      assert(texture != nil);
      observed = texture;
      refreshed = MakeHandle(stable->surface(), texture, 64, 48,
                             stable->bytes_per_row(), 2);
    }
    @autoreleasepool {
      assert(observed != nil);
      assert(refreshed->texture().width == 64);
    }
    refreshed.reset();
    assert(observed == nil);

    auto initial = AllocateHandle(device, 64, 48, 1);
    SurfaceBackingOwner owner;
    assert(owner.Acquire() == nullptr);
    assert(owner.Publish(nullptr, initial));
    auto published = owner.Acquire();
    assert(published == initial);
    assert(published->physical_width() == 64);
    assert(published->logical_height() == 24);

    // A stale expected handle cannot retire or replace the live tuple.
    auto stale_expected = ReimportHandle(device, published, 9);
    auto candidate = ReimportHandle(device, published, 2);
    assert(!owner.Publish(stale_expected, candidate));
    assert(owner.Acquire() == published);

    // Re-importing the same IOSurface with a newer texture/revision is the
    // valid resize/import path and preserves the physical backing identity.
    auto reimported = ReimportHandle(device, published, 2);
    assert(owner.Publish(published, reimported));
    auto current = owner.Acquire();
    assert(current == reimported);
    assert(current->surface() == published->surface());
    assert(current->revision() == 2);

    // A texture imported from another IOSurface is rejected even when its
    // dimensions and stride happen to match the current tuple.
    auto other = AllocateHandle(device, 64, 48, 3);
    auto wrong_texture = MakeHandle(current->surface(), other->texture(),
                                    64, 48, current->bytes_per_row(), 4);
    assert(!owner.Publish(current, wrong_texture));
    assert(owner.Acquire() == current);
    assert(!darwin_art::graphics::RetainSurfaceBacking(current->surface(),
        other->texture(), 64, 48, 32, 24, current->bytes_per_row(), 4));
    assert(darwin_art::graphics::RetainSurfaceBacking(current->surface(),
        current->texture(), 64, 48, 32, 24, current->bytes_per_row(), 4));

    for (unsigned mode = 0; mode < 4; ++mode) {
      auto invalid = std::make_shared<const SurfaceBackingHandle>(
          current->surface(), current->texture(), mode == 0 ? 0 : 64,
          mode == 1 ? 49 : 48, mode == 2 ? 0 : 32, 24,
          current->bytes_per_row() + (mode == 3 ? 1 : 0), 4);
      assert(!owner.Publish(current, invalid));
      assert(owner.Acquire() == current);
    }
    assert(!owner.Publish(current, current));  // Revision cannot repeat.
    auto logical_only = std::make_shared<const SurfaceBackingHandle>(
        current->surface(), current->texture(), 64, 48, 20, 14,
        current->bytes_per_row(), 3);
    assert(owner.Publish(current, logical_only));
    assert(current->logical_width() == 32 && current->logical_height() == 24);
    current = owner.Acquire();
    assert(current->logical_width() == 20 && current->logical_height() == 14);

    assert(owner.Close());
    assert(owner.Acquire() == nullptr);
    assert(owner.closed());
    auto after_close = ReimportHandle(device, current, 5);
    assert(!owner.Publish(nullptr, after_close));
    assert(!owner.Close());

    // The exact old allocation remains usable after replacement and Close.
    initial.reset();
    assert(IOSurfaceGetWidth(published->surface()) == 64);
    assert(published->texture().iosurface == published->surface());
    assert(IOSurfaceLock(published->surface(), 0, nullptr) == kIOReturnSuccess);
    assert(IOSurfaceGetBaseAddress(published->surface()) != nullptr);
    assert(IOSurfaceUnlock(published->surface(), 0, nullptr) == kIOReturnSuccess);

    SurfaceBackingOwner concurrent;
    assert(concurrent.Publish(nullptr, AllocateHandle(device, 64, 48, 1)));
    std::atomic<bool> stop{false};
    std::atomic<unsigned> reads{0};
    std::vector<std::thread> readers;
    for (unsigned i = 0; i < 4; ++i) readers.emplace_back([&] {
      while (!stop.load(std::memory_order_acquire)) {
        const auto snapshot = concurrent.Acquire();
        assert(snapshot != nullptr);
        assert(snapshot->physical_width() == IOSurfaceGetWidth(snapshot->surface()));
        assert(snapshot->physical_height() == snapshot->texture().height);
        assert(snapshot->logical_width() * 2 == snapshot->physical_width());
        assert(snapshot->texture().iosurface == snapshot->surface());
        reads.fetch_add(1, std::memory_order_relaxed);
      }
    });
    while (reads.load(std::memory_order_acquire) < 4) std::this_thread::yield();
    for (unsigned revision = 2; revision < 34; ++revision) {
      const auto prior = concurrent.Acquire();
      assert(concurrent.Publish(prior, AllocateHandle(device,
          revision % 2 ? 64 : 80, revision % 2 ? 48 : 60, revision)));
    }
    stop.store(true, std::memory_order_release);
    for (auto& reader : readers) reader.join();
    assert(reads.load() != 0);

    // Real retained resources with a test-only shared_ptr deleter reenter the
    // actual owner. A lock-held release deadlocks; destructor must seal first.
    for (bool destroy : {false, true}) {
      auto retiring = std::make_unique<SurfaceBackingOwner>();
      auto* raw = retiring.get();
      auto resource = AllocateHandle(device, 64, 48, 1);
      unsigned released = 0;
      SurfaceBackingOwner::Handle tracked(new SurfaceBackingHandle(
          resource->surface(), resource->texture(), 64, 48, 32, 24,
          resource->bytes_per_row(), 1), [&](const SurfaceBackingHandle* handle) {
        assert(raw->closed() && raw->Acquire() == nullptr);
        assert(!raw->Publish(nullptr, resource));
        ++released;
        delete handle;
      });
      assert(raw->Publish(nullptr, tracked));
      tracked.reset();
      if (destroy) retiring.reset(); else assert(raw->Close());
      assert(released == 1);
    }
  }
  std::puts("surface backing owner: real GPU/coherent snapshots/reimport/stale+invalid rejection/retained old allocation/unlocked close+destructor reentry PASS");
  return 0;
}
