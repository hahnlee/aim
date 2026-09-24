#include "service_darwin.h"
#include "../process/service_endpoint.h"
#include "../graphics/metal_shared_event_provider.h"
#include "composition_protocol.h"
#include "composition_queue.h"
#include "transaction_reply.h"
#include "iosurface_backing.h"
#include "output_registry.h"
#include "retained_layer_state.h"
#include "service_ingress.h"
#include "socket_deadline.h"
#include "socket_transport.h"
#include "display_attachment.h"
#include "layer_geometry.h"

#include "transaction_bridge.h"

#import <Foundation/Foundation.h>
#import <IOSurface/IOSurface.h>
#import <Metal/Metal.h>

#include <algorithm>
#include <array>
#include <atomic>
#include <chrono>
#include <cerrno>
#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <deque>
#include <fcntl.h>
#include <future>
#include <libproc.h>
#include <map>
#include <memory>
#include <limits>
#include <mutex>
#include <notify.h>
#include <poll.h>
#include <signal.h>
#include <set>
#include <string>
#include <stdexcept>
#include <sys/socket.h>
#include <sys/stat.h>
#include <sys/proc.h>
#include <sys/un.h>
#include <thread>
#include <type_traits>
#include <unordered_map>
#include <unistd.h>
#include <vector>

extern "C" int darwin_art_bionic_socket_broker_close(int fd);
extern "C" int darwin_art_bionic_fd_export_for_scm(int guest_fd);
extern "C" int darwin_art_bionic_fd_import_from_scm(int host_fd);
extern "C" int darwin_art_android_metal_shared_event_fence_fd(
    void* shared_event, uint64_t signal_value);

namespace {

using darwin_art::surfaceflinger::CompositionJob;
using darwin_art::surfaceflinger::CompositionQueue;
using darwin_art::surfaceflinger::ImportedCompositionBackings;
using darwin_art::surfaceflinger::IosurfaceBacking;
using darwin_art::surfaceflinger::OutputRegistry;
using darwin_art::surfaceflinger::OutputEpoch;
using darwin_art::surfaceflinger::OutputRequest;
using darwin_art::surfaceflinger::OutputResponse;
using darwin_art::surfaceflinger::OutputTransition;
using darwin_art::surfaceflinger::CopyTransparentRegion;
using darwin_art::surfaceflinger::MergeRetainedLayer;
using darwin_art::surfaceflinger::ServiceIngress;
using darwin_art::surfaceflinger::RequestHeader;
using darwin_art::surfaceflinger::RequestKind;
using darwin_art::surfaceflinger::ResponseHeader;
using darwin_art::surfaceflinger::WireLayer;
using darwin_art::surfaceflinger::kMaximumLayers;
using darwin_art::surfaceflinger::kProtocolVersion;
using darwin_art::surfaceflinger::kRequestMagic;
using darwin_art::surfaceflinger::kResponseMagic;
using darwin_art::surfaceflinger::ReadAll;
using darwin_art::surfaceflinger::WriteAll;
using darwin_art::surfaceflinger::SendDescriptor;
using darwin_art::surfaceflinger::ReceiveDescriptor;
using darwin_art::surfaceflinger::Connect;

void ProcessRequest(CompositionJob& job);
bool ConsumeProducerFence(int descriptor) noexcept;

std::string CompositionNotificationName(uint32_t surface_id) {
  return "dev.darwinart.surface." + std::to_string(surface_id) +
         ".composition";
}

bool TraceReparentToNull() {
  static const bool enabled =
      std::getenv("DARWIN_ART_TRACE_REPARENT_NULL") != nullptr;
  return enabled;
}


struct RetainedLayer {
  uint32_t submitting_process_id = 0;
  WireLayer layer{};
  std::shared_ptr<const IosurfaceBacking> backing;
};

struct TargetState {
  uint32_t width = 0;
  uint32_t height = 0;
  std::shared_ptr<const IosurfaceBacking> backing;
  std::shared_ptr<const OutputEpoch> epoch;
  // Explicit output anchors only. Descendant display membership is derived
  // from the current SurfaceControl parent graph on every composition.
  std::set<uint32_t> root_layers;
};

struct ServiceState {
  std::mutex mutex;
  std::map<uint32_t, TargetState> targets;
  // SurfaceControl hierarchy is display-independent. Buffer presentation
  // selects a target later, but WMS structural commits must survive even when
  // they arrive before the first application buffer.
  std::map<uint32_t, RetainedLayer> layers;
  std::map<std::pair<uint32_t, uint32_t>, uint32_t> global_layers;
  uint32_t next_global_layer = 1;
};

ServiceState& State() {
  static ServiceState state;
  return state;
}

OutputRegistry& Outputs() {
  static OutputRegistry registry;
  return registry;
}

// Caller holds ServiceState. The producer named a scanout its live owner
// replaced or resized (an AppKit resize) while the frame was in flight, or
// read the id and extent across that change. The output registry is
// authoritative: address the owner's current output and extent (Android
// anchors migrate on Replace). Outputs of dropped owners stay unresolved.
void RetargetToCurrentOutput(RequestHeader& header,
                             std::shared_ptr<const OutputEpoch>& epoch) {
  if (!epoch || !epoch->current()) epoch = Outputs().Admit(header.target_iosurface_id);
  if (!epoch || !epoch->current()) return;
  const auto& identity = epoch->identity();
  if (identity.iosurface_id == header.target_iosurface_id &&
      identity.logical_width == header.target_width &&
      identity.logical_height == header.target_height) return;
  if (std::getenv("DARWIN_ART_DEBUG_SURFACE_TRANSACTIONS") != nullptr) {
    std::fprintf(stderr,
                 "ART SurfaceFlinger: retarget replaced output pid=%u "
                 "transaction=%llu from=%u to=%u extent=%ux%u\n",
                 header.process_id,
                 static_cast<unsigned long long>(header.transaction_id),
                 header.target_iosurface_id, identity.iosurface_id,
                 identity.logical_width, identity.logical_height);
  }
  header.target_iosurface_id = identity.iosurface_id;
  header.target_width = identity.logical_width;
  header.target_height = identity.logical_height;
}

// Caller holds ServiceState first, then enters OutputRegistry. Replacing an
// output preserves its Android anchors; retirement removes only that output.
void ApplyOutputTransition(ServiceState& state, const OutputTransition& change) {
  if (change.response.status != 0) return;
  if (change.previous) {
    auto target = state.targets.extract(change.previous->identity().iosurface_id);
    if (change.current) {
      if (target.empty()) throw std::runtime_error("output target state missing");
      const auto& identity = change.current->identity();
      target.key() = identity.iosurface_id;
      target.mapped().width = identity.logical_width;
      target.mapped().height = identity.logical_height;
      target.mapped().backing = change.current->backing();
      target.mapped().epoch = change.current;
      if (!state.targets.insert(std::move(target)).inserted)
        throw std::runtime_error("output target state collision");
    }
  } else if (change.current) {
    const auto& identity = change.current->identity();
    auto [target, inserted] = state.targets.try_emplace(identity.iosurface_id);
    if (!inserted) throw std::runtime_error("output target state collision");
    target->second.width = identity.logical_width;
    target->second.height = identity.logical_height;
    target->second.backing = change.current->backing();
    target->second.epoch = change.current;
  }
}

OutputResponse ApplyOutputControl(int descriptor, const OutputRequest& request) {
  std::lock_guard<std::mutex> lock(State().mutex);
  auto change = Outputs().Apply(descriptor, request);
  ApplyOutputTransition(State(), change);
  return change.response;
}

void DropOutputControl(int descriptor) noexcept {
  std::lock_guard<std::mutex> lock(State().mutex);
  ApplyOutputTransition(State(), Outputs().Drop(descriptor));
}

uint32_t GlobalLayerId(ServiceState& state, uint32_t process_id,
                       uint32_t layer_id) {
  if (layer_id == 0) return 0;
  const auto key = std::make_pair(process_id, layer_id);
  auto [found, inserted] =
      state.global_layers.emplace(key, state.next_global_layer);
  if (inserted) ++state.next_global_layer;
  return found->second;
}

std::unordered_map<uint32_t, uint32_t> BuildParentMap(
    ServiceState& state) {
  std::unordered_map<uint32_t, uint32_t> parents;
  parents.reserve(state.layers.size());
  for (const auto& [global_id, retained] : state.layers) {
    const WireLayer& layer = retained.layer;
    const uint32_t owner = layer.owner_process_id == 0
        ? retained.submitting_process_id
        : layer.owner_process_id;
    const uint32_t parent_owner = layer.parent_owner_process_id == 0
        ? owner
        : layer.parent_owner_process_id;
    parents[global_id] =
        GlobalLayerId(state, parent_owner, layer.parent_id);
  }
  return parents;
}

bool ResolveLayerTransactionTarget(ServiceState& state,
                                   const RequestHeader& header,
                                   const std::vector<WireLayer>& incoming,
                                   uint32_t* target_id,
                                   uint32_t* target_width,
                                   uint32_t* target_height) {
  auto parents = BuildParentMap(state);
  std::vector<uint32_t> buffered_layers;
  for (const WireLayer& layer : incoming) {
    const uint32_t owner =
        layer.owner_process_id == 0 ? header.process_id : layer.owner_process_id;
    const uint32_t global = GlobalLayerId(state, owner, layer.layer_id);
    const bool is_new = state.layers.find(global) == state.layers.end();
    if (is_new || (layer.what & DARWIN_ART_SF_REPARENT) != 0) {
      const uint32_t parent_owner = layer.parent_owner_process_id == 0
          ? owner
          : layer.parent_owner_process_id;
      parents[global] =
          GlobalLayerId(state, parent_owner, layer.parent_id);
    }
    if ((layer.what & DARWIN_ART_SF_BUFFER_CHANGED) != 0 &&
        layer.iosurface_id != 0) {
      buffered_layers.push_back(global);
    }
  }
  if (buffered_layers.empty()) return false;

  std::set<uint32_t> matches;
  for (const auto& [id, target] : state.targets) {
    for (uint32_t layer : buffered_layers) {
      if (darwin_art::surfaceflinger::IsAttachedToDisplayAnchor(
              layer, parents, target.root_layers)) {
        matches.insert(id);
        break;
      }
    }
  }
  if (matches.size() != 1) return false;
  const auto target = state.targets.find(*matches.begin());
  if (target == state.targets.end() || target->second.width == 0 ||
      target->second.height == 0) {
    return false;
  }
  *target_id = target->first;
  *target_width = target->second.width;
  *target_height = target->second.height;
  return true;
}

bool ProcessAlive(uint32_t process_id) {
  if (process_id == 0) return false;
  if (kill(static_cast<pid_t>(process_id), 0) == 0) {
    // kill(pid, 0) also succeeds for a zombie. Android SurfaceFlinger drops a
    // client's layers when its Binder process dies; retaining a Darwin zombie
    // here leaves Chromium's final fullscreen renderer buffer above the new
    // tab-hub frame until the parent eventually reaps it.
    proc_bsdinfo info{};
    const int bytes = proc_pidinfo(static_cast<int>(process_id),
                                   PROC_PIDTBSDINFO, 0, &info, sizeof(info));
    return bytes != sizeof(info) || info.pbi_status != SZOMB;
  }
  return errno == EPERM;
}

bool DestroyProcessLayers(ServiceState& state, uint32_t process_id) {
  std::vector<uint32_t> layer_ids;
  for (const auto& [key, global_id] : state.global_layers) {
    if (key.first == process_id) layer_ids.push_back(global_id);
  }
  if (!layer_ids.empty() &&
      !darwin_art_surfaceflinger_destroy_layer_handles(layer_ids.data(),
                                                       layer_ids.size())) {
    return false;
  }
  for (auto iterator = state.global_layers.begin();
       iterator != state.global_layers.end();) {
    if (iterator->first.first == process_id)
      iterator = state.global_layers.erase(iterator);
    else
      ++iterator;
  }
  return true;
}

void SignalCompletion(int descriptor, bool successful) {
  if (descriptor < 0) return;
  if (successful) {
    constexpr uint64_t kSignaledFence =
        UINT64_C(0x44415257494e4653);  // "DARWINFS"
    ssize_t written = -1;
    do {
      written = write(descriptor, &kSignaledFence, sizeof(kSignaledFence));
    } while (written < 0 && errno == EINTR);
  }
  close(descriptor);
}

class CompletionDescriptorGuard {
 public:
  explicit CompletionDescriptorGuard(int descriptor) : descriptor_(descriptor) {}
  CompletionDescriptorGuard(const CompletionDescriptorGuard&) = delete;
  CompletionDescriptorGuard& operator=(const CompletionDescriptorGuard&) = delete;
  ~CompletionDescriptorGuard() {
    if (descriptor_ >= 0) close(descriptor_);
  }

  int release() {
    const int descriptor = descriptor_;
    descriptor_ = -1;
    return descriptor;
  }

 private:
  int descriptor_;
};

CompositionQueue& CompositionJobs() {
  // The worker must outlive all client/service threads and is therefore
  // intentionally process-lifetime storage rather than a destructed static.
  static auto* queue = new CompositionQueue();
  return *queue;
}

bool ConsumeProducerFence(int descriptor) noexcept {
  if (descriptor < 0) return true;
  std::array<uint8_t, sizeof(uint64_t)> marker{};
  ssize_t received = -1;
  do {
    received = read(descriptor, marker.data(), marker.size());
  } while (received < 0 && errno == EINTR);
  close(descriptor);
  return received > 0;
}

int32_t ProjectDisplayCoordinate(int32_t coordinate, uint32_t logical_extent,
                                 uint32_t physical_extent) {
  if (logical_extent == 0 || logical_extent == physical_extent)
    return coordinate;
  const int64_t product = static_cast<int64_t>(coordinate) * physical_extent;
  const int64_t half = static_cast<int64_t>(logical_extent) / 2;
  const int64_t projected =
      product >= 0 ? (product + half) / logical_extent
                   : (product - half) / logical_extent;
  return static_cast<int32_t>(std::clamp(
      projected, static_cast<int64_t>(std::numeric_limits<int32_t>::min()),
      static_cast<int64_t>(std::numeric_limits<int32_t>::max())));
}

void ApplyDisplayProjection(DarwinArtMetalComposerLayer* layer,
                            uint32_t logical_width, uint32_t logical_height,
                            uint32_t physical_width, uint32_t physical_height) {
  if (layer == nullptr) return;
  layer->destination_left = ProjectDisplayCoordinate(
      layer->destination_left, logical_width, physical_width);
  layer->destination_top = ProjectDisplayCoordinate(
      layer->destination_top, logical_height, physical_height);
  layer->destination_right = ProjectDisplayCoordinate(
      layer->destination_right, logical_width, physical_width);
  layer->destination_bottom = ProjectDisplayCoordinate(
      layer->destination_bottom, logical_height, physical_height);
  if (layer->has_clip) {
    layer->clip_left = ProjectDisplayCoordinate(
        layer->clip_left, logical_width, physical_width);
    layer->clip_top = ProjectDisplayCoordinate(
        layer->clip_top, logical_height, physical_height);
    layer->clip_right = ProjectDisplayCoordinate(
        layer->clip_right, logical_width, physical_width);
    layer->clip_bottom = ProjectDisplayCoordinate(
        layer->clip_bottom, logical_height, physical_height);
  }
  const uint32_t region_count = std::min(
      layer->transparent_region_count,
      static_cast<uint32_t>(kDarwinArtMaxTransparentRegionRects));
  for (uint32_t index = 0; index < region_count; ++index) {
    auto& rect = layer->transparent_region[index];
    rect.left = ProjectDisplayCoordinate(rect.left, logical_width,
                                         physical_width);
    rect.top = ProjectDisplayCoordinate(rect.top, logical_height,
                                        physical_height);
    rect.right = ProjectDisplayCoordinate(rect.right, logical_width,
                                          physical_width);
    rect.bottom = ProjectDisplayCoordinate(rect.bottom, logical_height,
                                           physical_height);
  }
}

void DebugSurfacePixels(const char* phase, uint64_t transaction_id,
                        uint32_t layer_id, IOSurfaceRef surface) {
  if (surface == nullptr ||
      std::getenv("DARWIN_ART_DEBUG_SURFACECONTROL_PIXELS") == nullptr) {
    return;
  }
  if (IOSurfaceLock(surface, kIOSurfaceLockReadOnly, nullptr) != kIOReturnSuccess) {
    std::fprintf(stderr,
                 "ART SurfaceFlinger: %s pixels transaction=%llu layer=%u "
                 "lock=failed\n",
                 phase, static_cast<unsigned long long>(transaction_id),
                 layer_id);
    return;
  }
  const auto* bytes = static_cast<const uint8_t*>(IOSurfaceGetBaseAddress(surface));
  const size_t width = IOSurfaceGetWidth(surface);
  const size_t height = IOSurfaceGetHeight(surface);
  const size_t row_bytes = IOSurfaceGetBytesPerRow(surface);
  const size_t element_bytes = IOSurfaceGetBytesPerElement(surface);
  uint64_t hash = UINT64_C(14695981039346656037);
  if (bytes != nullptr && element_bytes >= 4) {
    const size_t visible_row_bytes = width * element_bytes;
    for (size_t row = 0; row < height; ++row) {
      const uint8_t* pixel = bytes + row * row_bytes;
      for (size_t index = 0; index < visible_row_bytes; ++index) {
        hash ^= pixel[index];
        hash *= UINT64_C(1099511628211);
      }
    }
  }
  std::fprintf(stderr,
               "ART SurfaceFlinger: %s pixels transaction=%llu layer=%u "
               "surface=%u size=%zux%zu element=%zu hash=%016llx",
               phase, static_cast<unsigned long long>(transaction_id), layer_id,
               IOSurfaceGetID(surface), width, height, element_bytes,
               static_cast<unsigned long long>(hash));
  for (double y : std::array<double, 3>{0.25, 0.5, 0.75}) {
    for (double x : std::array<double, 3>{0.25, 0.5, 0.75}) {
      const size_t px = std::min(static_cast<size_t>(width * x), width - 1);
      const size_t py = std::min(static_cast<size_t>(height * y), height - 1);
      const uint8_t* pixel = bytes == nullptr || element_bytes < 4
                                 ? nullptr
                                 : bytes + py * row_bytes + px * element_bytes;
      if (pixel == nullptr)
        std::fprintf(stderr, " [%zu,%zu]=unmapped", px, py);
      else
        std::fprintf(stderr, " [%zu,%zu]=BGRA(%u,%u,%u,%u)", px, py,
                     pixel[0], pixel[1], pixel[2], pixel[3]);
    }
  }
  std::fprintf(stderr, "\n");
  IOSurfaceUnlock(surface, kIOSurfaceLockReadOnly, nullptr);
}

void CaptureTargetPpm(IOSurfaceRef surface, uint64_t transaction_id) {
  const char* path =
      std::getenv("DARWIN_ART_DEBUG_SURFACECONTROL_CAPTURE_PATH");
  if (surface == nullptr || path == nullptr || path[0] == '\0') return;
  std::string resolved_path(path);
  constexpr char kTransactionToken[] = "{transaction}";
  const size_t token_offset = resolved_path.find(kTransactionToken);
  if (token_offset != std::string::npos) {
    resolved_path.replace(token_offset, std::strlen(kTransactionToken),
                          std::to_string(transaction_id));
  }
  if (IOSurfaceLock(surface, kIOSurfaceLockReadOnly, nullptr) !=
      kIOReturnSuccess) {
    return;
  }
  const auto* bytes = static_cast<const uint8_t*>(IOSurfaceGetBaseAddress(surface));
  const size_t width = IOSurfaceGetWidth(surface);
  const size_t height = IOSurfaceGetHeight(surface);
  const size_t row_bytes = IOSurfaceGetBytesPerRow(surface);
  const size_t element_bytes = IOSurfaceGetBytesPerElement(surface);
  FILE* output = bytes == nullptr || element_bytes < 4
      ? nullptr
      : std::fopen(resolved_path.c_str(), "wb");
  if (output != nullptr) {
    std::fprintf(output, "P6\n%zu %zu\n255\n", width, height);
    std::vector<uint8_t> rgb(width * 3);
    for (size_t row = 0; row < height; ++row) {
      const uint8_t* source = bytes + row * row_bytes;
      for (size_t column = 0; column < width; ++column) {
        rgb[column * 3] = source[column * element_bytes + 2];
        rgb[column * 3 + 1] = source[column * element_bytes + 1];
        rgb[column * 3 + 2] = source[column * element_bytes];
      }
      std::fwrite(rgb.data(), 1, rgb.size(), output);
    }
    std::fclose(output);
  }
  IOSurfaceUnlock(surface, kIOSurfaceLockReadOnly, nullptr);
}

void ProcessRequest(CompositionJob& job) {
  RequestHeader header = job.header;
  std::vector<WireLayer> incoming = std::move(job.incoming);
  const auto request_kind = static_cast<RequestKind>(header.kind);
  const bool targetless_buffer =
      request_kind == RequestKind::kLayerTransaction;
  if (targetless_buffer) {
    std::lock_guard<std::mutex> lock(State().mutex);
    if (!ResolveLayerTransactionTarget(
            State(), header, incoming, &header.target_iosurface_id,
            &header.target_width, &header.target_height)) {
      std::fprintf(stderr,
                   "ART SurfaceFlinger: targetless buffer transaction has "
                   "no unique attached output pid=%u transaction=%llu\n",
                   header.process_id,
                   static_cast<unsigned long long>(header.transaction_id));
      SignalCompletion(job.completion_descriptor, false);
      job.completion_descriptor = -1;
      return;
    }
    // Layer transactions resolve after preceding queued structural changes.
    // Retain the resolved registration under the same State→Registry lock.
    job.output_epoch = Outputs().Admit(header.target_iosurface_id);
  }
  const bool structural_only =
      request_kind == RequestKind::kStructuralCommit;
  const bool explicit_display_present =
      request_kind == RequestKind::kDisplayPresent;
  auto completion_guard = std::make_shared<CompletionDescriptorGuard>(
      job.completion_descriptor);
  job.completion_descriptor = -1;
  if (!structural_only && request_kind == RequestKind::kDisplayPresent &&
      (!job.output_epoch || !job.output_epoch->current())) {
    std::lock_guard<std::mutex> lock(State().mutex);
    RetargetToCurrentOutput(header, job.output_epoch);
  }
  if (!structural_only &&
      (!job.output_epoch || !job.output_epoch->current())) {
    std::fprintf(stderr,
                 "ART SurfaceFlinger: output not current before commit pid=%u "
                 "transaction=%llu target=%u\n",
                 header.process_id,
                 static_cast<unsigned long long>(header.transaction_id),
                 header.target_iosurface_id);
    SignalCompletion(completion_guard->release(), false);
    return;
  }
  @autoreleasepool {
    struct TargetSnapshot {
      uint32_t id;
      uint32_t width;
      uint32_t height;
      std::shared_ptr<const OutputEpoch> epoch;
    };
    std::vector<TargetSnapshot> structural_targets;
    auto target_backing = job.output_epoch ? job.output_epoch->backing() : nullptr;
    IOSurfaceRef target_surface = structural_only || !target_backing
        ? nullptr
        : static_cast<IOSurfaceRef>(target_backing->native_surface());
    const size_t target_surface_width =
        target_surface == nullptr ? 0 : IOSurfaceGetWidth(target_surface);
    const size_t target_surface_height =
        target_surface == nullptr ? 0 : IOSurfaceGetHeight(target_surface);
    const bool physical_extent_valid =
        target_surface_width > 0 && target_surface_width <= UINT32_MAX &&
        target_surface_height > 0 && target_surface_height <= UINT32_MAX;
    const uint32_t physical_width =
        physical_extent_valid ? static_cast<uint32_t>(target_surface_width) : 0;
    const uint32_t physical_height =
        physical_extent_valid ? static_cast<uint32_t>(target_surface_height) : 0;
    bool valid = structural_only ||
                 (target_surface != nullptr &&
                  physical_extent_valid);
    std::vector<std::shared_ptr<const IosurfaceBacking>> retained_backings;
    if (target_backing) retained_backings.push_back(target_backing);
    std::vector<DarwinArtMetalComposerLayer> composition;
    struct ReparentNullTrace {
      uint32_t owner_process_id;
      uint32_t local_layer_id;
      uint32_t global_layer_id;
      uint32_t parent_id;
      uint32_t parent_global_id;
      uint64_t what;
    };
    std::vector<ReparentNullTrace> reparent_null_traces;
    bool android_committed = false;
    bool output_superseded = false;
    {
      std::lock_guard<std::mutex> lock(State().mutex);
      ServiceState& state = State();
      TargetState* target = nullptr;
      if (!structural_only) {
        const auto found = state.targets.find(header.target_iosurface_id);
        if (!job.output_epoch->current() || found == state.targets.end() ||
            found->second.epoch != job.output_epoch ||
            found->second.width != header.target_width ||
            found->second.height != header.target_height) {
          std::fprintf(stderr,
                       "ART SurfaceFlinger: output anchor changed before commit "
                       "pid=%u transaction=%llu target=%u\n",
                       header.process_id,
                       static_cast<unsigned long long>(header.transaction_id),
                       header.target_iosurface_id);
          SignalCompletion(completion_guard->release(), false);
          return;
        }
        target = &found->second;
      }
      std::vector<uint32_t> dead_owners;
      for (const auto& [global_id, retained] : state.layers) {
        (void)global_id;
        const uint32_t owner = retained.layer.owner_process_id == 0
            ? retained.submitting_process_id
            : retained.layer.owner_process_id;
        if (!ProcessAlive(owner) &&
            std::find(dead_owners.begin(), dead_owners.end(), owner) ==
                dead_owners.end()) {
          dead_owners.push_back(owner);
        }
      }
      for (uint32_t owner : dead_owners) {
        if (!DestroyProcessLayers(state, owner)) valid = false;
        for (auto iterator = state.layers.begin();
             iterator != state.layers.end();) {
          const uint32_t retained_owner =
              iterator->second.layer.owner_process_id == 0
                  ? iterator->second.submitting_process_id
                  : iterator->second.layer.owner_process_id;
          if (retained_owner == owner) {
            for (auto& [target_id, target_state] : state.targets) {
              (void)target_id;
              target_state.root_layers.erase(iterator->first);
            }
            iterator = state.layers.erase(iterator);
          } else
            ++iterator;
        }
      }

      std::vector<DarwinArtSurfaceFlingerLayerUpdate> updates;
      updates.reserve(incoming.size());
      for (const WireLayer& layer : incoming) {
        const uint32_t owner_process_id = layer.owner_process_id == 0
            ? header.process_id
            : layer.owner_process_id;
        const uint32_t parent_owner_process_id =
            layer.parent_owner_process_id == 0
                ? owner_process_id
                : layer.parent_owner_process_id;
        const uint32_t relative_parent_owner_process_id =
            layer.relative_parent_owner_process_id == 0
                ? owner_process_id
                : layer.relative_parent_owner_process_id;
        updates.push_back({
            .layer_id = GlobalLayerId(state, owner_process_id, layer.layer_id),
            .parent_id =
                GlobalLayerId(state, parent_owner_process_id, layer.parent_id),
            .relative_parent_id = GlobalLayerId(
                state, relative_parent_owner_process_id,
                layer.relative_parent_id),
            .what = layer.what,
            .flags = layer.flags,
            .mask = layer.mask,
            .transform = layer.transform,
            .x = static_cast<float>(layer.position_x),
            .y = static_cast<float>(layer.position_y),
            .scale_x = layer.scale_x,
            .scale_y = layer.scale_y,
            .z = layer.z,
            .alpha = layer.alpha,
            .destination_left = layer.destination_left,
            .destination_top = layer.destination_top,
            .destination_right = layer.destination_right,
            .destination_bottom = layer.destination_bottom,
            .crop_left = layer.crop_left,
            .crop_top = layer.crop_top,
            .crop_right = layer.crop_right,
            .crop_bottom = layer.crop_bottom,
        });
        const uint32_t global_id =
            GlobalLayerId(state, owner_process_id, layer.layer_id);
        if ((layer.what & DARWIN_ART_SF_REPARENT) != 0 &&
            layer.parent_id != 0) {
          for (auto& [target_id, target_state] : state.targets) {
            (void)target_id;
            target_state.root_layers.erase(global_id);
          }
        }
        if (target != nullptr && explicit_display_present &&
            layer.iosurface_id != 0 && layer.parent_id == 0) {
          target->root_layers.insert(global_id);
        }
        if (TraceReparentToNull() &&
            (layer.what & DARWIN_ART_SF_REPARENT) != 0 &&
            layer.parent_id == 0) {
          reparent_null_traces.push_back({
              .owner_process_id = owner_process_id,
              .local_layer_id = layer.layer_id,
              .global_layer_id = global_id,
              .parent_id = layer.parent_id,
              .parent_global_id = GlobalLayerId(
                  state, parent_owner_process_id, layer.parent_id),
              .what = layer.what,
          });
        }
        auto [retained, inserted] = state.layers.try_emplace(
            global_id, RetainedLayer{.submitting_process_id = header.process_id,
                                     .layer = layer});
        if (!inserted) {
          if (!structural_only || retained->second.submitting_process_id == 0) {
            retained->second.submitting_process_id = header.process_id;
          }
          MergeRetainedLayer(retained->second.layer, layer);
        }
        if (inserted || (layer.what & DARWIN_ART_SF_BUFFER_CHANGED) != 0) {
          retained->second.backing = job.backings
              ? job.backings->Find(layer.iosurface_id) : nullptr;
        }
      }
      DarwinArtSurfaceFlingerCommitResult commit{};
      const uint64_t central_transaction_id =
          (static_cast<uint64_t>(header.process_id) << 32) ^
          (header.transaction_id & UINT64_C(0xffffffff));
      if (!darwin_art_surfaceflinger_commit_transaction(
              central_transaction_id, updates.data(), updates.size(),
              &commit)) {
        std::fprintf(stderr,
                     "ART SurfaceFlinger: AOSP transaction flush failed "
                     "pid=%u transaction=%llu layers=%zu\n",
                     header.process_id,
                     static_cast<unsigned long long>(central_transaction_id),
                     updates.size());
        valid = false;
      } else {
        android_committed = true;
      }

      struct VisibleLayer {
        uint32_t process_id;
        WireLayer layer;
        std::shared_ptr<const IosurfaceBacking> backing;
      };
      std::map<uint32_t, VisibleLayer> visible_layers;
      const auto parent_by_layer = BuildParentMap(state);
      std::unordered_map<uint32_t,
                         darwin_art::surfaceflinger::LayerGeometryState>
          geometry_by_layer;
      geometry_by_layer.reserve(state.layers.size());
      for (const auto& [global_id, retained] : state.layers) {
        const WireLayer& layer = retained.layer;
        constexpr uint32_t kLayerHidden = 1u;
        geometry_by_layer.emplace(
            global_id,
            darwin_art::surfaceflinger::LayerGeometryState{
                .parent_id = parent_by_layer.at(global_id),
                .position_x = layer.position_x,
                .position_y = layer.position_y,
                .scale_x = layer.scale_x,
                .scale_y = layer.scale_y,
                .alpha = layer.alpha,
                .hidden = (layer.flags & kLayerHidden) != 0,
                .has_crop = layer.has_crop != 0,
                .crop = {.left = layer.crop_left,
                         .top = layer.crop_top,
                         .right = layer.crop_right,
                         .bottom = layer.crop_bottom},
            });
        if (structural_only || target == nullptr ||
            !darwin_art::surfaceflinger::IsAttachedToDisplayAnchor(
                global_id, parent_by_layer, target->root_layers) ||
            layer.iosurface_id == 0 ||
            (layer.flags & kLayerHidden) != 0) {
          continue;
        }
        visible_layers.emplace(
            global_id,
            VisibleLayer{.process_id = retained.submitting_process_id,
                         .layer = layer,
                         .backing = retained.backing});
      }
      size_t layer_order_count = 0;
      std::vector<uint32_t> layer_order;
      if (!darwin_art_surfaceflinger_copy_layer_order(
              nullptr, 0, &layer_order_count)) {
        valid = false;
      } else {
        layer_order.resize(layer_order_count);
        if (!darwin_art_surfaceflinger_copy_layer_order(
                layer_order.data(), layer_order.size(), &layer_order_count)) {
          valid = false;
        }
      }
      if (TraceReparentToNull()) {
        for (const auto& trace : reparent_null_traces) {
          const bool in_order = std::find(layer_order.begin(), layer_order.end(),
                                          trace.global_layer_id) !=
              layer_order.end();
          std::fprintf(stderr,
                       "ART SurfaceFlinger trace: after-commit-reparent-null "
                       "owner=%u local=%u global=%u what=0x%llx parent=%u "
                       "parent_global=%u order_membership=%d target=%u\n",
                       trace.owner_process_id, trace.local_layer_id,
                       trace.global_layer_id,
                       static_cast<unsigned long long>(trace.what),
                       trace.parent_id, trace.parent_global_id,
                       in_order ? 1 : 0, header.target_iosurface_id);
        }
      }
      // Retained buffers can belong to detached/offscreen layers. Only the
      // AOSP hierarchy determines which candidates participate in this frame;
      // absence from its order is not a transaction failure.
      for (uint32_t global_id : layer_order) {
        if (structural_only) break;
        const auto found = visible_layers.find(global_id);
        if (found == visible_layers.end()) continue;
        const VisibleLayer& visible = found->second;
        const WireLayer& layer = visible.layer;
        const uint32_t owner_process_id = layer.owner_process_id == 0
            ? visible.process_id
            : layer.owner_process_id;
        const uint32_t parent_owner_process_id =
            layer.parent_owner_process_id == 0
                ? owner_process_id
                : layer.parent_owner_process_id;
        const uint32_t relative_parent_owner_process_id =
            layer.relative_parent_owner_process_id == 0
                ? owner_process_id
                : layer.relative_parent_owner_process_id;
        const auto resolved =
            darwin_art::surfaceflinger::ResolveLayerGeometry(
                global_id, geometry_by_layer,
                {.left = layer.destination_left,
                 .top = layer.destination_top,
                 .right = layer.destination_right,
                 .bottom = layer.destination_bottom});
        if (!resolved.visible) continue;
        IOSurfaceRef surface = !found->second.backing ? nullptr
            : static_cast<IOSurfaceRef>(found->second.backing->native_surface());
        if (surface == nullptr) {
          std::fprintf(stderr,
                       "ART SurfaceFlinger: retained backing missing layer=%u "
                       "storage=%u transaction=%llu\n",
                       global_id, layer.iosurface_id,
                       static_cast<unsigned long long>(header.transaction_id));
          valid = false;
          continue;
        }
        DebugSurfacePixels("source", header.transaction_id, global_id, surface);
        retained_backings.push_back(found->second.backing);
        composition.push_back({
            .owner_process_id = owner_process_id,
            .layer_id = global_id,
            .parent_owner_process_id = parent_owner_process_id,
            .parent_id = GlobalLayerId(state, parent_owner_process_id,
                                       layer.parent_id),
            .relative_parent_owner_process_id =
                relative_parent_owner_process_id,
            .relative_parent_id = GlobalLayerId(
                state, relative_parent_owner_process_id,
                layer.relative_parent_id),
            .what = layer.what,
            .flags = layer.flags,
            .mask = layer.mask,
            .transform = layer.transform,
            .producer_bottom_left = layer.producer_bottom_left != 0,
            .iosurface = surface,
            .width = layer.width,
            .height = layer.height,
            .source_left = layer.source_left,
            .source_top = layer.source_top,
            .source_right = layer.source_right,
            .source_bottom = layer.source_bottom,
            .destination_left = resolved.destination.left,
            .destination_top = resolved.destination.top,
            .destination_right = resolved.destination.right,
            .destination_bottom = resolved.destination.bottom,
            .position_x = layer.position_x,
            .position_y = layer.position_y,
            .scale_x = layer.scale_x,
            .scale_y = layer.scale_y,
            .has_crop = layer.has_crop != 0,
            .crop_left = layer.crop_left,
            .crop_top = layer.crop_top,
            .crop_right = layer.crop_right,
            .crop_bottom = layer.crop_bottom,
            .has_clip = resolved.has_clip,
            .clip_left = resolved.clip.left,
            .clip_top = resolved.clip.top,
            .clip_right = resolved.clip.right,
            .clip_bottom = resolved.clip.bottom,
            .z = layer.z,
            .alpha = layer.alpha * resolved.inherited_alpha,
            .transparent_region_count = 0,
        });
        composition.back().transparent_region_count = CopyTransparentRegion(
            layer.transparent_region, layer.transparent_region_count,
            composition.back().transparent_region);
        ApplyDisplayProjection(&composition.back(), header.target_width,
                               header.target_height, physical_width,
                               physical_height);
        if (std::getenv("DARWIN_ART_DEBUG_GRAPHICS_DSO") != nullptr) {
          const auto& selected = composition.back();
          std::fprintf(
              stderr,
              "ART SurfaceFlinger: selected layer transaction=%llu order=%zu "
              "global=%u owner=%u parent=%u relative=%u z=%d "
              "destination=[%d,%d,%d,%d] source=[%d,%d,%d,%d] alpha=%.3f\n",
              static_cast<unsigned long long>(header.transaction_id),
              composition.size() - 1, selected.layer_id,
              selected.owner_process_id, selected.parent_id,
              selected.relative_parent_id, selected.z,
              selected.destination_left, selected.destination_top,
              selected.destination_right, selected.destination_bottom,
              selected.source_left, selected.source_top, selected.source_right,
              selected.source_bottom, selected.alpha);
        }
      }
      if (structural_only && valid) {
        structural_targets.reserve(state.targets.size());
        for (const auto& [id, existing_target] : state.targets) {
          if (id != 0 && existing_target.width != 0 &&
              existing_target.height != 0) {
            structural_targets.push_back(
                {id, existing_target.width, existing_target.height,
                 existing_target.epoch});
          }
        }
      }
    }

    // Receipt proves real Android acceptance, not ingress/queue admission.
    // Send outside State's lock; later GPU failure changes only completion.
    if (android_committed && job.reply) (void)job.reply->Committed();

    if (structural_only) {
      // A structural WMS transaction can move, hide or detach layers after an
      // application has stopped producing buffers. Recompose every known
      // display target from the updated retained tree so stale popup/child
      // pixels cannot remain until an unrelated app frame arrives.
      for (const TargetSnapshot& target : structural_targets) {
        CompositionJob redraw{};
        redraw.header = header;
        // This is an internal display recomposition of already-retained
        // layers, not another structural transaction. Keeping the original
        // kind recursively scheduled the same redraw until the compositor
        // thread exhausted its stack whenever WMS added a popup ViewRoot.
        redraw.header.kind =
            static_cast<uint32_t>(RequestKind::kDisplayPresent);
        redraw.header.target_iosurface_id = target.id;
        redraw.header.target_width = target.width;
        redraw.header.target_height = target.height;
        redraw.header.layer_count = 0;
        redraw.completion_descriptor = -1;
        redraw.output_epoch = target.epoch;
        ProcessRequest(redraw);
      }
      SignalCompletion(completion_guard->release(), valid);
      if (!valid) {
        std::fprintf(stderr,
                     "ART SurfaceFlinger: central structural commit failed "
                     "pid=%u transaction=%llu layers=%zu\n",
                     header.process_id,
                     static_cast<unsigned long long>(header.transaction_id),
                     incoming.size());
      }
      return;
    }

    // State validation above is the admission linearization point. Already
    // admitted GPU work may finish on its retained former backing after Drop;
    // completion publication still checks the live epoch under State lock.
    // Acquire the +1 device only after early stale/structural returns.
    id<MTLDevice> device = MTLCreateSystemDefaultDevice();
    if (device == nil) valid = false;
    void* completion = nullptr;
    bool completion_owned_by_callback = false;
    uint64_t completion_value = 0;
    if (std::getenv("DARWIN_ART_DEBUG_GRAPHICS_DSO") != nullptr &&
        !composition.empty()) {
      const auto& layer = composition.front();
      std::fprintf(stderr,
                   "ART SurfaceFlinger: display projection logical=%ux%u "
                   "physical=%ux%u destination=[%d,%d,%d,%d]\n",
                   header.target_width, header.target_height, physical_width,
                   physical_height, layer.destination_left,
                   layer.destination_top, layer.destination_right,
                   layer.destination_bottom);
    }
    if (valid) {
      // Short submission admission, never hold State across shader/pipeline
      // creation or Metal encoding. Drop cancels later admission/publication,
      // not a previously admitted job's retained backing.
      std::lock_guard<std::mutex> lock(State().mutex);
      const auto anchor = State().targets.find(header.target_iosurface_id);
      valid = job.output_epoch->current() &&
          anchor != State().targets.end() &&
          anchor->second.epoch == job.output_epoch &&
          anchor->second.width == header.target_width &&
          anchor->second.height == header.target_height;
      // ADR 0003: an output replaced or retired after admission leaves the
      // committed Android transaction without a new frame; it is not a
      // submission failure. No GPU work has been issued for it yet.
      output_superseded = !valid && android_committed;
    }
    if (valid &&
        !darwin_art_metal_composer_compose(
            device, target_surface, physical_width, physical_height,
            composition.data(), composition.size(), nullptr, 0, &completion,
            &completion_value)) {
      valid = false;
    }
    if (valid && completion != nullptr && completion_value != 0) {
      id<MTLSharedEvent> event = reinterpret_cast<id<MTLSharedEvent>>(completion);
      MTLSharedEventListener* listener =
          [[MTLSharedEventListener alloc] init];
      if (listener == nil) {
        valid = false;
      } else {
        const std::string publication_notification =
            CompositionNotificationName(header.target_iosurface_id);
        const char* capture_path =
            std::getenv("DARWIN_ART_DEBUG_SURFACECONTROL_CAPTURE_PATH");
        IOSurfaceRef debug_target =
            std::getenv("DARWIN_ART_DEBUG_SURFACECONTROL_PIXELS") != nullptr ||
                    (capture_path != nullptr && capture_path[0] != '\0')
                ? target_surface
                : nullptr;
        if (debug_target != nullptr) CFRetain(debug_target);
        auto completed_backings = std::make_shared<decltype(retained_backings)>(
            std::move(retained_backings));
        auto completed_epoch = job.output_epoch;
        // The provider registry owns the event until this completion tail.
        // CFRelease would leave its registry entry dangling; retiring before
        // notification would also abandon the borrowed asynchronous handle.
        completion_owned_by_callback = true;
        [event notifyListener:listener
                      atValue:completion_value
                        block:^(id<MTLSharedEvent>, uint64_t) {
                          // Producer fences and display-consumer readiness
                          // are separate contracts. Every completed target
                          // must wake its independent HWC/AppKit subscriber.
                          bool current_output = false;
                          uint32_t notification_status = NOTIFY_STATUS_OK;
                          {
                            // Linearize publication with Replace/Drop. A
                            // separate atomic check followed by notify_post
                            // could wake a replacement sharing the same ID.
                            std::lock_guard<std::mutex> lock(State().mutex);
                            const auto anchor = State().targets.find(
                                header.target_iosurface_id);
                            current_output = completed_epoch->current() &&
                                anchor != State().targets.end() &&
                                anchor->second.epoch == completed_epoch;
                            if (current_output)
                              notification_status = notify_post(
                                  publication_notification.c_str());
                          }
                          if (std::getenv(
                                  "DARWIN_ART_DEBUG_SURFACE_TRANSACTIONS") !=
                              nullptr) {
                            std::fprintf(
                                stderr,
                                "ART SurfaceFlinger: display notification "
                                "posted target=%u status=%u\n",
                                header.target_iosurface_id,
                                notification_status);
                          }
                          if (debug_target != nullptr) {
                            DebugSurfacePixels("target", header.transaction_id,
                                               0, debug_target);
                            CaptureTargetPpm(debug_target,
                                             header.transaction_id);
                            CFRelease(debug_target);
                          }
                          // The shared-event callback proves this submitted
                          // GPU use finished even if its output was retired or
                          // replaced meanwhile. Output eligibility gates only
                          // scanout publication above, not buffer-use completion.
                          SignalCompletion(completion_guard->release(), true);
                          completed_backings->clear();
                          darwin_art_android_metal_shared_event_release(completion);
                        }];
        [listener release];
      }
    } else {
      valid = false;
    }
    if (!valid && output_superseded) {
      if (std::getenv("DARWIN_ART_DEBUG_SURFACE_TRANSACTIONS") != nullptr) {
        std::fprintf(stderr,
                     "ART SurfaceFlinger: committed without present; output "
                     "superseded pid=%u transaction=%llu target=%u\n",
                     header.process_id,
                     static_cast<unsigned long long>(header.transaction_id),
                     header.target_iosurface_id);
      }
      SignalCompletion(completion_guard->release(), true);
    } else if (!valid) {
      std::fprintf(stderr,
                   "ART SurfaceFlinger: central compose failed pid=%u "
                   "transaction=%llu layers=%zu target=%u\n",
                   header.process_id,
                   static_cast<unsigned long long>(header.transaction_id),
                   composition.size(), header.target_iosurface_id);
      SignalCompletion(completion_guard->release(), false);
    } else if (std::getenv("DARWIN_ART_DEBUG_SURFACE_TRANSACTIONS") !=
               nullptr) {
      std::fprintf(stderr,
                   "ART SurfaceFlinger: central compose queued owner=%d "
                   "client=%u transaction=%llu layers=%zu target=%u\n",
                   getpid(), header.process_id,
                   static_cast<unsigned long long>(header.transaction_id),
                   composition.size(), header.target_iosurface_id);
    }
    if (completion != nullptr && !completion_owned_by_callback)
      darwin_art_android_metal_shared_event_release(completion);
    if (device != nil) [device release];
  }
}

bool HandleRequest(int client, const std::array<char, 8>& prefix) {
  const auto deadline = std::chrono::steady_clock::now() + std::chrono::seconds(2);
  RequestHeader header{};
  std::memcpy(header.magic, prefix.data(), prefix.size());
  const auto valid_kind = [](uint32_t value) {
    return value <= static_cast<uint32_t>(RequestKind::kLayerTransaction);
  };
  if (!darwin_art::surfaceflinger::ReadAllUntil(
          client, reinterpret_cast<unsigned char*>(&header) + prefix.size(),
          sizeof(header) - prefix.size(), deadline) ||
      std::memcmp(header.magic, kRequestMagic.data(), kRequestMagic.size()) !=
          0 ||
      header.version != kProtocolVersion || !valid_kind(header.kind) ||
      header.process_id == 0 ||
      !((header.kind == static_cast<uint32_t>(RequestKind::kDisplayPresent) &&
         header.target_iosurface_id != 0 && header.target_width != 0 &&
         header.target_height != 0) ||
        (header.kind != static_cast<uint32_t>(RequestKind::kDisplayPresent) &&
         header.target_iosurface_id == 0 && header.target_width == 0 &&
         header.target_height == 0 && header.has_producer_fence == 0)) ||
      header.layer_count > kMaximumLayers ||
      header.has_producer_fence > 1) {
    return false;
  }
  std::vector<WireLayer> incoming(header.layer_count);
  if (!incoming.empty() &&
      !darwin_art::surfaceflinger::ReadAllUntil(
          client, incoming.data(), incoming.size() * sizeof(WireLayer), deadline)) {
    return false;
  }
  int producer_descriptor = -1;
  if (!darwin_art::surfaceflinger::WaitReadableUntil(client, deadline) ||
      !ReceiveDescriptor(client, header.has_producer_fence != 0,
                         &producer_descriptor)) {
    return false;
  }
  // Ingress retains originals until successful queue transfer, including
  // allocation failure while importing storage or growing the bounded FIFO.
  CompletionDescriptorGuard producer_guard(producer_descriptor);
  std::vector<uint32_t> storage_ids;
  storage_ids.reserve(incoming.size() + 1);
  storage_ids.push_back(header.target_iosurface_id);
  for (const auto& layer : incoming) storage_ids.push_back(layer.iosurface_id);
  auto backings = ImportedCompositionBackings::Import(storage_ids);
  int completion_pipe[2]{-1, -1};
  int32_t status = backings ? 0 : EINVAL;
  std::shared_ptr<const OutputEpoch> output_epoch;
  if (status == 0 && header.kind == static_cast<uint32_t>(RequestKind::kDisplayPresent)) {
    std::lock_guard<std::mutex> lock(State().mutex);
    output_epoch = Outputs().Admit(header.target_iosurface_id);
    RetargetToCurrentOutput(header, output_epoch);
    if (!output_epoch || !output_epoch->current() ||
        output_epoch->identity().logical_width != header.target_width ||
        output_epoch->identity().logical_height != header.target_height)
      status = ESTALE;
  }
  if (status != 0) {
    std::fprintf(stderr,
                 "ART SurfaceFlinger: request rejected pid=%u transaction=%llu "
                 "kind=%u target=%u status=%d\n",
                 header.process_id,
                 static_cast<unsigned long long>(header.transaction_id),
                 header.kind, header.target_iosurface_id, status);
  }
  if (status == 0 && pipe(completion_pipe) != 0) status = errno;
  CompletionDescriptorGuard completion_read_guard(completion_pipe[0]);
  CompletionDescriptorGuard completion_write_guard(completion_pipe[1]);
  if (status == 0) {
    for (int descriptor : completion_pipe) {
      const int flags = fcntl(descriptor, F_GETFD);
      if (flags < 0 ||
          fcntl(descriptor, F_SETFD, flags | FD_CLOEXEC) != 0) {
        status = errno;
        break;
      }
    }
#ifdef F_SETNOSIGPIPE
    if (status == 0 &&
        fcntl(completion_pipe[1], F_SETNOSIGPIPE, 1) != 0) {
      status = errno;
    }
#endif
  }
  ResponseHeader response{};
  std::shared_ptr<darwin_art::surfaceflinger::TransactionReply> reply;
  if (status == 0) {
    reply = darwin_art::surfaceflinger::TransactionReply::Create(
        client, completion_pipe[0]);
    if (!reply) status = ENOMEM;
  }
  if (status == 0) {
    if (CompositionJobs().TryEnqueue(CompositionJob{
            .header = header,
            .incoming = std::move(incoming),
            .producer_descriptor = producer_descriptor,
            .completion_descriptor = completion_pipe[1],
            .backings = std::move(backings),
            .output_epoch = std::move(output_epoch),
            .reply = reply})) {
      (void)producer_guard.release();
      (void)completion_write_guard.release();
      producer_descriptor = -1;
      completion_pipe[1] = -1;
      // Ingress closes only its own descriptor; the worker replies after
      // real commit without blocking output EOF/control processing.
      close(completion_read_guard.release());
      return true;
    } else {
      status = EAGAIN;
    }
  }
  std::memcpy(response.magic, kResponseMagic.data(), kResponseMagic.size());
  response.version = kProtocolVersion;
  response.status = status;
  response.commit = darwin_art::surfaceflinger::CommitDisposition::RejectedBeforeCommit;
  response.has_completion_fence = status == 0 ? 1 : 0;
  const bool sent = WriteAll(client, &response, sizeof(response)) &&
                    SendDescriptor(client,
                                   status == 0 ? completion_pipe[0] : -1);
  if (!sent || status != 0) {
    return false;
  }
  return true;
}

void ComposeIngress(int descriptor, const std::array<char, 8>& prefix) {
  (void)HandleRequest(descriptor, prefix);
}

void Serve(std::shared_ptr<darwin_art::process::ServiceEndpoint> endpoint,
           std::shared_ptr<std::promise<bool>> ready,
           std::shared_ptr<std::atomic<bool>> running) {
  const auto publication = endpoint->publish_ready(
      darwin_art::process::ServiceReadiness::Compositor);
  const bool available = publication != darwin_art::process::ReadinessPublication::Failed;
  running->store(available, std::memory_order_release);
  ready->set_value(available);
  if (!available) {
    CompositionJobs().Abort();
    return;
  }
  const int listener = endpoint->descriptor();
  const int failure = CompositionJobs().failure_descriptor();
  ServiceIngress ingress(&ApplyOutputControl, &DropOutputControl, &ComposeIngress);
  for (;;) {
    if (!running->load(std::memory_order_acquire)) break;
    std::vector<pollfd> waiters{
        {.fd = listener, .events = POLLIN, .revents = 0},
        {.fd = failure, .events = POLLIN, .revents = 0},
    };
    ingress.AppendPoll(&waiters);
    int polled = -1;
    do {
      polled = poll(waiters.data(), waiters.size(), 1000);
    } while (polled < 0 && errno == EINTR);
    if (polled < 0) {
      CompositionJobs().Fail();
      break;
    }
    if ((waiters[1].revents & POLLIN) != 0 ||
        (waiters[1].revents & (POLLERR | POLLHUP | POLLNVAL)) != 0) {
      CompositionJobs().Fail();
      break;
    }
    if ((waiters[0].revents & (POLLERR | POLLHUP | POLLNVAL)) != 0) {
      CompositionJobs().Fail();
      break;
    }
    for (size_t index = 2; index < waiters.size(); ++index) {
      if (waiters[index].revents != 0)
        ingress.Ready(waiters[index].fd, waiters[index].revents);
    }
    ingress.ExpirePending();
    if ((waiters[0].revents & POLLIN) == 0) continue;
    const int client = accept(listener, nullptr, nullptr);
    if (client < 0) {
      if (errno == EINTR) continue;
      CompositionJobs().Fail();
      break;
    }
    (void)ingress.Add(client);
  }
  running->store(false, std::memory_order_release);
}

}  // namespace

extern "C" bool darwin_art_surfaceflinger_service_start() {
  static std::once_flag once;
  static auto started = std::make_shared<std::atomic<bool>>(false);
  std::call_once(once, [] {
    const char* path = std::getenv("DARWIN_ART_SURFACEFLINGER_SOCKET");
    if (path == nullptr || path[0] == '\0') return;
    auto listener = std::make_shared<darwin_art::process::ServiceEndpoint>(path);
    if (!*listener) return;
    if (!CompositionJobs().Start(listener, &ProcessRequest,
                                 &ConsumeProducerFence, started)) {
      return;
    }
    auto ready = std::make_shared<std::promise<bool>>();
    auto result = ready->get_future();
    try {
      std::thread(Serve, listener, ready, started).detach();
    } catch (...) {
      CompositionJobs().Abort();
      ready->set_value(false);
      return;
    }
    if (!result.get()) return;
    std::fprintf(stderr, "ART SurfaceFlinger: central service transport started socket=%s\n",
                 path);
  });
  return started->load(std::memory_order_acquire);
}
