#include "metal_composer.h"

#import <IOSurface/IOSurface.h>
#import <Metal/Metal.h>

#include <algorithm>
#include <array>
#include <cstdio>
#include <mutex>

namespace {

struct MetalComposerState {
  id<MTLDevice> device = nil;
  id<MTLCommandQueue> queue = nil;
  id<MTLRenderPipelineState> pipeline = nil;

  ~MetalComposerState() {
    [pipeline release];
    [queue release];
    [device release];
  }
};

std::mutex& ComposerMutex() {
  static std::mutex mutex;
  return mutex;
}

MetalComposerState& ComposerState() {
  static MetalComposerState state;
  return state;
}

bool EnsurePipeline(id<MTLDevice> device, MetalComposerState& state) {
  if (state.device == device && state.queue != nil && state.pipeline != nil)
    return true;
  [state.pipeline release];
  [state.queue release];
  [state.device release];
  state = {};

  static NSString* const source = @R"metal(
#include <metal_stdlib>
using namespace metal;
struct VertexIn { float2 position; float2 texcoord; };
struct VertexOut { float4 position [[position]]; float2 texcoord; };
struct TransparentRegionUniform {
  uint count;
  uint reserved0;
  uint reserved1;
  uint reserved2;
  float4 rects[8];
};
vertex VertexOut darwin_art_composer_vertex(
    uint vertex_id [[vertex_id]], constant VertexIn* vertices [[buffer(0)]]) {
  VertexOut out;
  out.position = float4(vertices[vertex_id].position, 0.0, 1.0);
  out.texcoord = vertices[vertex_id].texcoord;
  return out;
}
fragment float4 darwin_art_composer_fragment(
    VertexOut in [[stage_in]], texture2d<float> source [[texture(0)]],
    constant float& alpha [[buffer(0)]],
    constant TransparentRegionUniform& transparent [[buffer(1)]]) {
  // SurfaceControl's transparent-region hint is a hole in this layer, not a
  // replacement color.  Discarding lets the already-composed lower layer
  // survive the source-over blend, including when this layer's buffer is
  // otherwise opaque.  Rectangles are bounded and supplied in target pixels.
  for (uint index = 0; index < transparent.count && index < 8; ++index) {
    const float4 rect = transparent.rects[index];
    if (in.position.x >= rect.x && in.position.x < rect.z &&
        in.position.y >= rect.y && in.position.y < rect.w) {
      discard_fragment();
    }
  }
  constexpr sampler linear_sampler(coord::normalized, address::clamp_to_edge,
                                   filter::linear);
  return source.sample(linear_sampler, in.texcoord) * alpha;
}
)metal";

  NSError* error = nil;
  id<MTLLibrary> library = [device newLibraryWithSource:source
                                                options:nil
                                                  error:&error];
  if (library == nil) {
    std::fprintf(stderr, "ART Metal Composer: shader compile failed: %s\n",
                 error == nil ? "unknown" : error.localizedDescription.UTF8String);
    return false;
  }
  id<MTLFunction> vertex =
      [library newFunctionWithName:@"darwin_art_composer_vertex"];
  id<MTLFunction> fragment =
      [library newFunctionWithName:@"darwin_art_composer_fragment"];
  MTLRenderPipelineDescriptor* descriptor =
      [[MTLRenderPipelineDescriptor alloc] init];
  descriptor.vertexFunction = vertex;
  descriptor.fragmentFunction = fragment;
  descriptor.colorAttachments[0].pixelFormat = MTLPixelFormatBGRA8Unorm;
  descriptor.colorAttachments[0].blendingEnabled = YES;
  descriptor.colorAttachments[0].rgbBlendOperation = MTLBlendOperationAdd;
  descriptor.colorAttachments[0].alphaBlendOperation = MTLBlendOperationAdd;
  descriptor.colorAttachments[0].sourceRGBBlendFactor = MTLBlendFactorOne;
  descriptor.colorAttachments[0].sourceAlphaBlendFactor = MTLBlendFactorOne;
  descriptor.colorAttachments[0].destinationRGBBlendFactor =
      MTLBlendFactorOneMinusSourceAlpha;
  descriptor.colorAttachments[0].destinationAlphaBlendFactor =
      MTLBlendFactorOneMinusSourceAlpha;
  id<MTLRenderPipelineState> pipeline =
      [device newRenderPipelineStateWithDescriptor:descriptor error:&error];
  [descriptor release];
  [fragment release];
  [vertex release];
  [library release];
  if (pipeline == nil) {
    std::fprintf(stderr, "ART Metal Composer: pipeline creation failed: %s\n",
                 error == nil ? "unknown" : error.localizedDescription.UTF8String);
    return false;
  }
  id<MTLCommandQueue> queue = [device newCommandQueue];
  if (queue == nil) {
    [pipeline release];
    return false;
  }
  state.device = [device retain];
  state.queue = queue;
  state.pipeline = pipeline;
  return true;
}

struct Vertex {
  float position[2];
  float texcoord[2];
};

// Keep the CPU payload layout identical to TransparentRegionUniform in the
// Metal shader. The explicit padding gives the uint/float4 pair the same
// 16-byte alignment on both sides of the API boundary.
struct TransparentRegionUniform {
  uint32_t count = 0;
  uint32_t reserved[3] = {};
  float rects[kDarwinArtMaxTransparentRegionRects][4] = {};
};

static_assert(sizeof(TransparentRegionUniform) == 16 +
                  16 * kDarwinArtMaxTransparentRegionRects);

std::array<float, 2> TransformTexcoord(uint32_t transform, float u0,
                                        float v0, float u1, float v1,
                                        size_t corner) {
  // Corners are destination TL, BL, BR, TR. Android HAL transforms operate
  // on the buffer before it is placed in the destination frame.
  const std::array<std::array<float, 2>, 4> identity{{
      {{u0, v0}}, {{u0, v1}}, {{u1, v1}}, {{u1, v0}}}};
  const auto pick = [&](std::array<size_t, 4> mapping) {
    return identity[mapping[corner]];
  };
  switch (transform & 7u) {
    case 1:  // HAL_TRANSFORM_FLIP_H
      return pick({3, 2, 1, 0});
    case 2:  // HAL_TRANSFORM_FLIP_V
      return pick({1, 0, 3, 2});
    case 3:  // HAL_TRANSFORM_ROT_180
      return pick({2, 3, 0, 1});
    case 4:  // HAL_TRANSFORM_ROT_90
      return pick({1, 2, 3, 0});
    case 5:  // HAL_TRANSFORM_ROT_90 | FLIP_H
      return pick({0, 3, 2, 1});
    case 6:  // HAL_TRANSFORM_ROT_90 | FLIP_V
      return pick({2, 1, 0, 3});
    case 7:  // HAL_TRANSFORM_ROT_270
      return pick({3, 0, 1, 2});
    default:
      return identity[corner];
  }
}

}  // namespace

extern "C" bool darwin_art_metal_composer_compose(
    void* metal_device, void* target_iosurface, uint32_t target_width,
    uint32_t target_height, const DarwinArtMetalComposerLayer* layers,
    size_t layer_count, void* producer_event, uint64_t producer_value,
    void** completion_event, uint64_t* completion_value) {
  if (completion_event == nullptr || completion_value == nullptr) return false;
  *completion_event = nullptr;
  *completion_value = 0;
  if (metal_device == nullptr || target_iosurface == nullptr ||
      target_width == 0 || target_height == 0 ||
      (layer_count != 0 && layers == nullptr)) {
    return false;
  }
  id<MTLDevice> device = reinterpret_cast<id<MTLDevice>>(metal_device);
  auto* target_surface = reinterpret_cast<IOSurfaceRef>(target_iosurface);
  const uint32_t surface_width = IOSurfaceGetWidth(target_surface);
  const uint32_t surface_height = IOSurfaceGetHeight(target_surface);
  if (surface_width == 0 || surface_height == 0) return false;
  // The transaction carries logical output dimensions, while a resized
  // IOSurface owns the authoritative physical texture extent. Metal rejects
  // a descriptor wider than the surface stride (e.g. 720 over a 600-wide
  // surface), so normalize composition to the target surface generation.
  const uint32_t compose_width = surface_width;
  const uint32_t compose_height = surface_height;
  if (std::getenv("DARWIN_ART_DEBUG_GRAPHICS_DSO") != nullptr &&
      (target_width != compose_width || target_height != compose_height)) {
    std::fprintf(stderr,
                 "ART Metal composer: target logical=%ux%u surface=%ux%u stride=%zu\n",
                 target_width, target_height, compose_width, compose_height,
                 IOSurfaceGetBytesPerRow(target_surface));
  }
  std::lock_guard<std::mutex> lock(ComposerMutex());
  MetalComposerState& state = ComposerState();
  if (!EnsurePipeline(device, state)) return false;

  MTLTextureDescriptor* target_descriptor =
      [MTLTextureDescriptor texture2DDescriptorWithPixelFormat:MTLPixelFormatBGRA8Unorm
                                                         width:compose_width
                                                        height:compose_height
                                                     mipmapped:NO];
  target_descriptor.storageMode = MTLStorageModeShared;
  target_descriptor.usage = MTLTextureUsageRenderTarget |
                            MTLTextureUsageShaderRead |
                            MTLTextureUsageShaderWrite;
  id<MTLTexture> target =
      [device newTextureWithDescriptor:target_descriptor
                             iosurface:target_surface
                                 plane:0];
  if (target == nil) return false;
  id<MTLCommandBuffer> command_buffer = [state.queue commandBuffer];
  if (command_buffer == nil) {
    [target release];
    return false;
  }
  if (producer_event != nullptr && producer_value != 0) {
    [command_buffer
        encodeWaitForEvent:reinterpret_cast<id<MTLSharedEvent>>(producer_event)
                     value:producer_value];
  }

  MTLRenderPassDescriptor* pass = [MTLRenderPassDescriptor renderPassDescriptor];
  pass.colorAttachments[0].texture = target;
  pass.colorAttachments[0].loadAction = MTLLoadActionClear;
  pass.colorAttachments[0].storeAction = MTLStoreActionStore;
  pass.colorAttachments[0].clearColor = MTLClearColorMake(0.0, 0.0, 0.0, 0.0);
  id<MTLRenderCommandEncoder> encoder =
      [command_buffer renderCommandEncoderWithDescriptor:pass];
  if (encoder == nil) {
    [target release];
    return false;
  }
  [encoder setRenderPipelineState:state.pipeline];

  for (size_t index = 0; index < layer_count; ++index) {
    const DarwinArtMetalComposerLayer& layer = layers[index];
    if (layer.iosurface == nullptr || layer.width == 0 || layer.height == 0 ||
        layer.source_right <= layer.source_left ||
        layer.source_bottom <= layer.source_top ||
        layer.destination_right <= layer.destination_left ||
        layer.destination_bottom <= layer.destination_top) {
      continue;
    }
    CFTypeRef storage_rgba = IOSurfaceCopyValue(
        reinterpret_cast<IOSurfaceRef>(layer.iosurface), CFSTR("DarwinArtStorageRGBA"));
    const bool rgba = storage_rgba != nullptr && CFEqual(storage_rgba, kCFBooleanTrue);
    if (storage_rgba != nullptr) CFRelease(storage_rgba);
    IOSurfaceRef source_surface = reinterpret_cast<IOSurfaceRef>(layer.iosurface);
    const uint32_t source_surface_width = IOSurfaceGetWidth(source_surface);
    const uint32_t source_surface_height = IOSurfaceGetHeight(source_surface);
    if (source_surface_width == 0 || source_surface_height == 0) continue;
    if (std::getenv("DARWIN_ART_DEBUG_GRAPHICS_DSO") != nullptr &&
        (layer.width != source_surface_width || layer.height != source_surface_height)) {
      std::fprintf(stderr,
                   "ART Metal composer: source logical=%ux%u surface=%ux%u stride=%zu\n",
                   layer.width, layer.height, source_surface_width,
                   source_surface_height, IOSurfaceGetBytesPerRow(source_surface));
    }
    MTLTextureDescriptor* source_descriptor =
        [MTLTextureDescriptor texture2DDescriptorWithPixelFormat:
                                 (rgba ? MTLPixelFormatRGBA8Unorm : MTLPixelFormatBGRA8Unorm)
                                                           width:source_surface_width
                                                          height:source_surface_height
                                                       mipmapped:NO];
    source_descriptor.storageMode = MTLStorageModeShared;
    source_descriptor.usage = MTLTextureUsageShaderRead |
                              MTLTextureUsageShaderWrite |
                              MTLTextureUsageRenderTarget;
    id<MTLTexture> source = [device
        newTextureWithDescriptor:source_descriptor
                       iosurface:source_surface
                           plane:0];
    if (source == nil) continue;

    const float left = 2.0f * static_cast<float>(layer.destination_left) /
                           static_cast<float>(compose_width) -
                       1.0f;
    const float right =
        2.0f * static_cast<float>(layer.destination_right) /
            static_cast<float>(compose_width) -
        1.0f;
    const float top = 1.0f -
                      2.0f * static_cast<float>(layer.destination_top) /
                          static_cast<float>(compose_height);
    const float bottom =
        1.0f - 2.0f * static_cast<float>(layer.destination_bottom) /
                   static_cast<float>(compose_height);
    const float u0 = static_cast<float>(layer.source_left) / layer.width;
    const float u1 = static_cast<float>(layer.source_right) / layer.width;
    // Surface transactions use top-left coordinates. Normalize the native
    // producer storage origin once, before applying the explicit Android HAL
    // transform. ANGLE composer-overlay buffers are already display-space;
    // HWUI's Metal buffers retain a bottom-left storage origin.
    CFTypeRef top_left = IOSurfaceCopyValue(
        reinterpret_cast<IOSurfaceRef>(layer.iosurface), CFSTR("DarwinArtProducerTopLeft"));
    const bool explicit_top_left = top_left != nullptr && CFEqual(top_left, kCFBooleanTrue);
    if (top_left != nullptr) CFRelease(top_left);
    const bool producer_bottom_left = layer.producer_bottom_left && !explicit_top_left;
    const float v0 = producer_bottom_left
                         ? 1.0f - static_cast<float>(layer.source_top) /
                               layer.height
                         : static_cast<float>(layer.source_top) / layer.height;
    const float v1 = producer_bottom_left
                         ? 1.0f - static_cast<float>(layer.source_bottom) /
                               layer.height
                         : static_cast<float>(layer.source_bottom) / layer.height;
    const auto texcoord = [&](size_t corner) {
      return TransformTexcoord(layer.transform, u0, v0, u1, v1, corner);
    };
    const auto tl = texcoord(0);
    const auto bl = texcoord(1);
    const auto br = texcoord(2);
    const auto tr = texcoord(3);
    const std::array<Vertex, 6> vertices{{
        {{left, top}, {tl[0], tl[1]}},
        {{left, bottom}, {bl[0], bl[1]}},
        {{right, bottom}, {br[0], br[1]}},
        {{left, top}, {tl[0], tl[1]}},
        {{right, bottom}, {br[0], br[1]}},
        {{right, top}, {tr[0], tr[1]}},
    }};
    const float alpha = std::clamp(layer.alpha, 0.0f, 1.0f);
    TransparentRegionUniform transparent_region;
    transparent_region.count = std::min(
        layer.transparent_region_count,
        static_cast<uint32_t>(kDarwinArtMaxTransparentRegionRects));
    for (uint32_t region_index = 0;
         region_index < transparent_region.count; ++region_index) {
      const DarwinArtTransparentRegionRect& rect =
          layer.transparent_region[region_index];
      transparent_region.rects[region_index][0] =
          static_cast<float>(rect.left);
      transparent_region.rects[region_index][1] =
          static_cast<float>(rect.top);
      transparent_region.rects[region_index][2] =
          static_cast<float>(rect.right);
      transparent_region.rects[region_index][3] =
          static_cast<float>(rect.bottom);
    }
    [encoder setVertexBytes:vertices.data()
                     length:sizeof(vertices)
                    atIndex:0];
    [encoder setFragmentTexture:source atIndex:0];
    [encoder setFragmentBytes:&alpha length:sizeof(alpha) atIndex:0];
    [encoder setFragmentBytes:&transparent_region
                        length:sizeof(transparent_region)
                       atIndex:1];
    if (layer.has_clip) {
      const int32_t clip_left = std::clamp(
          layer.clip_left, 0, static_cast<int32_t>(compose_width));
      const int32_t clip_top = std::clamp(
          layer.clip_top, 0, static_cast<int32_t>(compose_height));
      const int32_t clip_right = std::clamp(
          layer.clip_right, clip_left, static_cast<int32_t>(compose_width));
      const int32_t clip_bottom = std::clamp(
          layer.clip_bottom, clip_top, static_cast<int32_t>(compose_height));
      if (clip_right <= clip_left || clip_bottom <= clip_top) {
        [source release];
        continue;
      }
      [encoder setScissorRect:MTLScissorRect{
                                  static_cast<NSUInteger>(clip_left),
                                  static_cast<NSUInteger>(clip_top),
                                  static_cast<NSUInteger>(clip_right - clip_left),
                                  static_cast<NSUInteger>(clip_bottom - clip_top)}];
    } else {
      [encoder setScissorRect:MTLScissorRect{0, 0, compose_width,
                                             compose_height}];
    }
    [encoder drawPrimitives:MTLPrimitiveTypeTriangle
                vertexStart:0
                vertexCount:vertices.size()];
    [source release];
  }
  [encoder endEncoding];

  id<MTLSharedEvent> event = [device newSharedEvent];
  if (event == nil) {
    [target release];
    return false;
  }
  const uint64_t value = event.signaledValue + 1;
  [command_buffer encodeSignalEvent:event value:value];
  [command_buffer commit];
  [target release];
  *completion_event = reinterpret_cast<void*>(event);
  *completion_value = value;
  return true;
}
