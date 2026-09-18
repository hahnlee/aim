#include "service_darwin.h"
#include "client_receipt.h"
#include "composition_protocol.h"
#include "socket_transport.h"
#include "retained_layer_state.h"
#import <IOSurface/IOSurface.h>
#include <cerrno>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <unistd.h>
#include <utility>
#include <vector>
extern "C" int darwin_art_bionic_socket_broker_close(int);
extern "C" int darwin_art_bionic_fd_export_for_scm(int);
extern "C" int darwin_art_bionic_fd_import_from_scm(int);
extern "C" int darwin_art_android_metal_shared_event_fence_fd(void*, uint64_t);
namespace {
using namespace darwin_art::surfaceflinger;
class NativeDescriptorOwner final {
 public:
  explicit NativeDescriptorOwner(int fd) : fd_(fd) {}
  ~NativeDescriptorOwner() { if (fd_ >= 0) close(fd_); }
  NativeDescriptorOwner(const NativeDescriptorOwner&) = delete;
  NativeDescriptorOwner& operator=(const NativeDescriptorOwner&) = delete;
  int release() { return std::exchange(fd_, -1); }
 private:
  int fd_;
};
static DarwinArtSurfaceFlingerReceipt SubmitSurfaceFlingerRequest(
    RequestKind kind,
    uint32_t target_iosurface_id, uint32_t target_width,
    uint32_t target_height, uint64_t transaction_id,
    const DarwinArtMetalComposerLayer* layers, size_t layer_count,
    void* producer_event, uint64_t producer_value) {
  DarwinArtSurfaceFlingerReceipt failure{DARWIN_ART_SF_COMMIT_REJECTED, EINVAL, -1};
  try {
  const char* path = std::getenv("DARWIN_ART_SURFACEFLINGER_SOCKET");
  if (path == nullptr || path[0] == '\0' ||
      ((producer_event == nullptr) != (producer_value == 0)) ||
      layer_count > kMaximumLayers ||
      (layer_count != 0 && layers == nullptr)) {
    return failure;
  }
  int producer_host_descriptor = -1;
  if (producer_event != nullptr) {
    const int producer_guest_descriptor =
        darwin_art_android_metal_shared_event_fence_fd(producer_event,
                                                       producer_value);
    if (producer_guest_descriptor < 0) return failure;
    producer_host_descriptor =
        darwin_art_bionic_fd_export_for_scm(producer_guest_descriptor);
    (void)darwin_art_bionic_socket_broker_close(producer_guest_descriptor);
    if (producer_host_descriptor < 0) return failure;
  }
  NativeDescriptorOwner producer_descriptor_owner(producer_host_descriptor);
  std::vector<WireLayer> wire_layers;
  wire_layers.reserve(layer_count);
  for (size_t index = 0; index < layer_count; ++index) {
    const DarwinArtMetalComposerLayer& layer = layers[index];
    auto surface = reinterpret_cast<IOSurfaceRef>(layer.iosurface);
    const uint32_t surface_id =
        surface == nullptr ? 0 : IOSurfaceGetID(surface);
    if (layer.layer_id == 0) return failure;
    WireLayer wire{
        .owner_process_id = layer.owner_process_id,
        .layer_id = layer.layer_id,
        .parent_owner_process_id = layer.parent_owner_process_id,
        .parent_id = layer.parent_id,
        .relative_parent_owner_process_id =
            layer.relative_parent_owner_process_id,
        .relative_parent_id = layer.relative_parent_id,
        .iosurface_id = surface_id,
        .width = layer.width,
        .height = layer.height,
        .what = layer.what,
        .flags = layer.flags,
        .mask = layer.mask,
        .transform = layer.transform,
        .producer_bottom_left = layer.producer_bottom_left ? 1u : 0u,
        .source_left = layer.source_left,
        .source_top = layer.source_top,
        .source_right = layer.source_right,
        .source_bottom = layer.source_bottom,
        .destination_left = layer.destination_left,
        .destination_top = layer.destination_top,
        .destination_right = layer.destination_right,
        .destination_bottom = layer.destination_bottom,
        .position_x = layer.position_x,
        .position_y = layer.position_y,
        .scale_x = layer.scale_x,
        .scale_y = layer.scale_y,
        .has_crop = layer.has_crop ? 1u : 0u,
        .crop_left = layer.crop_left,
        .crop_top = layer.crop_top,
        .crop_right = layer.crop_right,
        .crop_bottom = layer.crop_bottom,
        .z = layer.z,
        .alpha = layer.alpha,
        .transparent_region_count = 0,
    };
    wire.transparent_region_count = CopyTransparentRegion(
        layer.transparent_region, layer.transparent_region_count,
        wire.transparent_region);
    wire_layers.push_back(wire);
  }
  const int fd = Connect(path);
  if (fd < 0) {
    std::fprintf(stderr, "ART SurfaceFlinger client: connect failed path=%s errno=%d\n",
                 path, errno);
    return failure;
  }
  NativeDescriptorOwner socket_owner(fd);
  RequestHeader request{};
  std::memcpy(request.magic, kRequestMagic.data(), kRequestMagic.size());
  request.version = kProtocolVersion;
  request.kind = static_cast<uint32_t>(kind);
  request.process_id = static_cast<uint32_t>(getpid());
  request.target_iosurface_id = target_iosurface_id;
  request.target_width = target_width;
  request.target_height = target_height;
  request.layer_count = static_cast<uint32_t>(wire_layers.size());
  request.has_producer_fence = producer_host_descriptor >= 0 ? 1 : 0;
  request.transaction_id = transaction_id;
  if (std::getenv("DARWIN_ART_DEBUG_GRAPHICS_DSO") != nullptr) {
    std::fprintf(stderr,
                 "ART SurfaceFlinger client: target=%u logical=%ux%u "
                 "transaction=%llu layers=%zu\n",
                 target_iosurface_id, target_width, target_height,
                 static_cast<unsigned long long>(transaction_id),
                 wire_layers.size());
  }
  failure = {DARWIN_ART_SF_COMMIT_UNKNOWN, EIO, -1};
  const bool written =
      WriteAll(fd, &request, sizeof(request)) &&
      (wire_layers.empty() ||
       WriteAll(fd, wire_layers.data(),
                wire_layers.size() * sizeof(WireLayer))) &&
      SendDescriptor(fd, producer_host_descriptor);
  if (producer_host_descriptor >= 0) close(producer_descriptor_owner.release());
  if (!written) return failure;
  return ReadClientReceipt(fd, &darwin_art_bionic_fd_import_from_scm);
  } catch (...) {
    failure.error = ENOMEM;
    return failure;
  }
}
}  // namespace

extern "C" DarwinArtSurfaceFlingerReceipt darwin_art_surfaceflinger_service_present_receipt(
    uint32_t target_iosurface_id, uint32_t target_width,
    uint32_t target_height, uint64_t transaction_id,
    const DarwinArtMetalComposerLayer* layers, size_t layer_count,
    void* producer_event, uint64_t producer_value) {
  return SubmitSurfaceFlingerRequest(RequestKind::kDisplayPresent,
      target_iosurface_id, target_width, target_height, transaction_id,
      layers, layer_count, producer_event, producer_value);
}
extern "C" DarwinArtSurfaceFlingerReceipt darwin_art_surfaceflinger_service_submit_receipt(
    uint64_t transaction_id, const DarwinArtMetalComposerLayer* layers, size_t layer_count) {
  return SubmitSurfaceFlingerRequest(RequestKind::kLayerTransaction, 0, 0, 0,
      transaction_id, layers, layer_count, nullptr, 0);
}
extern "C" DarwinArtSurfaceFlingerReceipt darwin_art_surfaceflinger_service_commit_receipt(
    uint64_t transaction_id, const DarwinArtMetalComposerLayer* layers, size_t layer_count) {
  return SubmitSurfaceFlingerRequest(RequestKind::kStructuralCommit, 0, 0, 0,
      transaction_id, layers, layer_count, nullptr, 0);
}
extern "C" int darwin_art_surfaceflinger_service_present(
    uint32_t target_iosurface_id, uint32_t target_width,
    uint32_t target_height, uint64_t transaction_id,
    const DarwinArtMetalComposerLayer* layers, size_t layer_count,
    void* producer_event, uint64_t producer_value) {
  return darwin_art_surfaceflinger_service_present_receipt(target_iosurface_id,
      target_width, target_height, transaction_id, layers, layer_count,
      producer_event, producer_value).completion_fd;
}
extern "C" int darwin_art_surfaceflinger_service_submit(
    uint64_t transaction_id, const DarwinArtMetalComposerLayer* layers, size_t layer_count) {
  return darwin_art_surfaceflinger_service_submit_receipt(
      transaction_id, layers, layer_count).completion_fd;
}
extern "C" int darwin_art_surfaceflinger_service_commit(
    uint64_t transaction_id, const DarwinArtMetalComposerLayer* layers, size_t layer_count) {
  return darwin_art_surfaceflinger_service_commit_receipt(
      transaction_id, layers, layer_count).completion_fd;
}
