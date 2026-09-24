#include "transaction_bridge.h"

#include "FrontEnd/LayerHierarchy.h"
#include "FrontEnd/LayerLifecycleManager.h"
#include "FrontEnd/RequestedLayerState.h"
#include "FrontEnd/TransactionHandler.h"
#include "layer_state_factory.h"

#include <algorithm>
#include <functional>
#include <mutex>
#include <string>
#include <unordered_map>
#include <unordered_set>
#include <utility>
#include <vector>

namespace {

std::mutex g_transaction_mutex;
android::surfaceflinger::frontend::TransactionHandler g_transaction_handler;
android::surfaceflinger::frontend::LayerLifecycleManager g_layer_lifecycle;
android::surfaceflinger::frontend::LayerHierarchyBuilder g_layer_hierarchy;
std::unordered_set<uint32_t> g_known_layers;
std::vector<uint32_t> g_layer_order;

android::ResolvedComposerState ResolveLayerState(
    const DarwinArtSurfaceFlingerLayerUpdate& source) {
  android::ResolvedComposerState resolved;
  resolved.layerId = source.layer_id;
  // Darwin's pointer-free wire ABI uses zero for "no parent" while the AOSP
  // frontend uses UNASSIGNED_LAYER_ID. This matters when eReparent is set:
  // passing zero through makes an otherwise valid root an offscreen child of
  // a nonexistent layer 0.
  resolved.parentId =
      source.parent_id == 0 ? UNASSIGNED_LAYER_ID : source.parent_id;
  resolved.relativeParentId = source.relative_parent_id == 0
      ? UNASSIGNED_LAYER_ID
      : source.relative_parent_id;
  resolved.state.layerId = static_cast<int32_t>(source.layer_id);
  // Transparent-region hints are consumed by Darwin's GPU composer. They are
  // not part of this vendored frontend's layer_state_t change-bit set, so do
  // not let the private wire bit alter AOSP transaction merge semantics.
  resolved.state.what =
      source.what & ~static_cast<uint64_t>(
                          DARWIN_ART_SF_TRANSPARENT_REGION_CHANGED |
                          DARWIN_ART_SF_BUFFER_DEFINES_BOUNDS);
  resolved.state.flags = source.flags;
  resolved.state.mask = source.mask;
  resolved.state.bufferTransform = source.transform;
  resolved.state.x = source.x;
  resolved.state.y = source.y;
  resolved.state.matrix.dsdx = source.scale_x;
  resolved.state.matrix.dtdx = 0.0f;
  resolved.state.matrix.dtdy = 0.0f;
  resolved.state.matrix.dsdy = source.scale_y;
  resolved.state.z = source.z;
  resolved.state.color.a = source.alpha;
  resolved.state.destinationFrame =
      android::Rect(source.destination_left, source.destination_top,
                    source.destination_right, source.destination_bottom);
  resolved.state.crop = android::FloatRect(
      static_cast<float>(source.crop_left),
      static_cast<float>(source.crop_top),
      static_cast<float>(source.crop_right),
      static_cast<float>(source.crop_bottom));
  return resolved;
}

void AddMissingLayers(const DarwinArtSurfaceFlingerLayerUpdate* updates,
                      size_t update_count) {
  std::unordered_map<uint32_t, uint32_t> requested_parents;
  requested_parents.reserve(update_count);
  for (size_t index = 0; index < update_count; ++index) {
    const auto& update = updates[index];
    if (update.layer_id != 0) {
      requested_parents[update.layer_id] = update.parent_id;
    }
  }

  std::vector<std::unique_ptr<
      android::surfaceflinger::frontend::RequestedLayerState>>
      layers;
  layers.reserve(update_count * 2);
  std::unordered_set<uint32_t> visiting;
  std::function<void(uint32_t)> add_layer = [&](uint32_t layer_id) {
    if (layer_id == 0 || g_known_layers.contains(layer_id)) return;
    if (!visiting.insert(layer_id).second) return;

    const auto requested = requested_parents.find(layer_id);
    const uint32_t parent_id = requested == requested_parents.end()
        ? 0
        : requested->second;
    // Buffer snapshots can name a structural SurfaceControl parent which has
    // no buffer of its own. SurfaceFlinger still owns a handle for that
    // parent, so mirror it before adding the child. If its creation metadata
    // was not part of this buffer snapshot, it is a display-root placeholder;
    // a later transaction can reparent it through the normal AOSP merge path.
    add_layer(parent_id);
    if (!g_known_layers.contains(layer_id)) {
      layers.push_back(
          android::surfaceflinger::frontend::makeDarwinRequestedLayerState(
              layer_id, parent_id));
      g_known_layers.insert(layer_id);
    }
    visiting.erase(layer_id);
  };
  for (size_t index = 0; index < update_count; ++index) {
    add_layer(updates[index].layer_id);
  }
  if (!layers.empty()) g_layer_lifecycle.addLayers(std::move(layers));
}

void RebuildLayerOrder() {
  g_layer_hierarchy.update(g_layer_lifecycle);
  g_layer_order.clear();
  g_layer_hierarchy.getHierarchy().traverseInZOrder(
      [](const android::surfaceflinger::frontend::LayerHierarchy& hierarchy,
         const android::surfaceflinger::frontend::LayerHierarchy::TraversalPath&) {
        const auto* layer = hierarchy.getLayer();
        if (layer != nullptr) g_layer_order.push_back(layer->id);
        return true;
      });
  g_layer_lifecycle.commitChanges();
}

}  // namespace

extern "C" bool darwin_art_surfaceflinger_commit_transaction(
    uint64_t transaction_id,
    const DarwinArtSurfaceFlingerLayerUpdate* updates,
    size_t update_count,
    DarwinArtSurfaceFlingerCommitResult* out_result) {
  if (transaction_id == 0 || (update_count != 0 && updates == nullptr) ||
      out_result == nullptr) {
    return false;
  }
  *out_result = {};

  android::QueuedTransactionState transaction{};
  transaction.flags = 0;
  transaction.desiredPresentTime = 0;
  transaction.isAutoTimestamp = true;
  transaction.postTime = systemTime();
  transaction.hasListenerCallbacks = false;
  transaction.originPid = 1;
  transaction.originUid = 1;
  transaction.id = transaction_id;
  transaction.states.reserve(update_count);
  for (size_t index = 0; index < update_count; ++index) {
    transaction.states.emplace_back(ResolveLayerState(updates[index]));
  }

  std::lock_guard<std::mutex> lock(g_transaction_mutex);
  AddMissingLayers(updates, update_count);
  g_transaction_handler.queueTransaction(std::move(transaction));
  g_transaction_handler.collectTransactions();
  std::vector<android::QueuedTransactionState> flushed =
      g_transaction_handler.flushTransactions();

  size_t layer_state_count = 0;
  bool found = false;
  for (const auto& candidate : flushed) {
    layer_state_count += candidate.states.size();
    found = found || candidate.id == transaction_id;
  }
  if (!found || g_transaction_handler.hasPendingTransactions()) return false;

  g_layer_lifecycle.applyTransactions(flushed);
  RebuildLayerOrder();

  *out_result = DarwinArtSurfaceFlingerCommitResult{
      .transaction_id = transaction_id,
      .transaction_count = flushed.size(),
      .layer_state_count = layer_state_count,
  };
  return true;
}

extern "C" bool darwin_art_surfaceflinger_copy_layer_order(
    uint32_t* out_layer_ids, size_t capacity, size_t* out_count) {
  if (out_count == nullptr || (capacity != 0 && out_layer_ids == nullptr)) {
    return false;
  }
  std::lock_guard<std::mutex> lock(g_transaction_mutex);
  *out_count = g_layer_order.size();
  if (capacity == 0) return true;
  if (capacity < g_layer_order.size()) return false;
  std::copy(g_layer_order.begin(), g_layer_order.end(), out_layer_ids);
  return true;
}

extern "C" bool darwin_art_surfaceflinger_destroy_layer_handles(
    const uint32_t* layer_ids, size_t layer_count) {
  if (layer_count != 0 && layer_ids == nullptr) return false;
  std::lock_guard<std::mutex> lock(g_transaction_mutex);
  std::vector<std::pair<uint32_t, std::string>> destroyed_handles;
  destroyed_handles.reserve(layer_count);
  for (size_t index = 0; index < layer_count; ++index) {
    if (layer_ids[index] == 0 || !g_known_layers.contains(layer_ids[index])) {
      return false;
    }
    destroyed_handles.emplace_back(layer_ids[index], "DarwinLayer");
  }
  g_layer_lifecycle.onHandlesDestroyed(destroyed_handles);
  for (size_t index = 0; index < layer_count; ++index) {
    g_known_layers.erase(layer_ids[index]);
  }
  RebuildLayerOrder();
  return true;
}
