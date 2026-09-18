#include "surface_control_registry.h"

#include "surface_transaction_lifetime.h"
#include "surface_transaction_state.h"

#include <algorithm>
#include <atomic>
#include <cstdio>
#include <limits>
#include <mutex>
#include <memory>
#include <new>
#include <string>
#include <unordered_set>
#include <unordered_map>
#include <utility>
#include <unistd.h>

extern "C" void AHardwareBuffer_acquire(AHardwareBuffer* buffer);
extern "C" void AHardwareBuffer_release(AHardwareBuffer* buffer);

namespace darwin_art::window {
namespace {

struct SurfaceControl final {
  std::atomic<uint32_t> references{1};
  uint32_t owner_process_id = 0;
  uint32_t layer_id = 0;
  std::string name;
  SurfaceControl* parent = nullptr;
  uint32_t imported_parent_owner_process_id = 0;
  uint32_t imported_parent_layer_id = 0;
  SurfaceControl* relative_to = nullptr;
  bool composition_root = false;
  AHardwareBuffer* buffer = nullptr;
  bool buffer_reference_owned = false;
  ARect source{};
  ARect destination{};
  ARect crop{};
  bool has_geometry = false;
  bool has_crop = false;
  bool visible = true;
  int32_t position_x = 0;
  int32_t position_y = 0;
  int32_t transform = 0;
  int32_t z_order = 0;
  float scale_x = 1.0f;
  float scale_y = 1.0f;
  float alpha = 1.0f;
  std::vector<ARect> transparent_region;
};

std::mutex g_surface_controls_mutex;
std::vector<SurfaceControl*> g_surface_controls;
std::atomic<uint32_t> g_next_surface_layer_id{1};
// At most one prepared plan may reserve the committed registry.  The plan
// pins stable objects, but allowing another accepted mutator to run while it
// is pending would invalidate its simulated predecessor/relation state.
bool g_active_prepared_commit = false;

struct LayerKey {
  uint32_t owner_process = 0;
  uint32_t layer = 0;

  bool operator==(const LayerKey& other) const {
    return owner_process == other.owner_process && layer == other.layer;
  }
};

struct LayerKeyHash {
  size_t operator()(const LayerKey& key) const noexcept {
    return (static_cast<size_t>(key.owner_process) << 32) ^
           static_cast<size_t>(key.layer);
  }
};

struct LayerOccurrence {
  AHardwareBuffer* buffer = nullptr;  // one registry-owned reference
  bool buffer_reference_owned = false;
  uint64_t cookie = 0;
  size_t wrappers = 0;
};

// Buffer submission occurrence is a property of the canonical remote layer,
// not of a process-local SurfaceControl wrapper. The canonical buffer keeps a
// reference independent of any process-local alias, so an alias replacement
// can report the real predecessor even when its wrapper has no local buffer.
std::unordered_map<LayerKey, LayerOccurrence, LayerKeyHash> g_layer_occurrences;

bool IsAttachedToCompositionRoot(
    const SurfaceControl* control,
    const std::vector<SurfaceControl*>& controls) {
  for (size_t depth = 0; control != nullptr && depth <= controls.size();
       ++depth) {
    if (std::find(controls.begin(), controls.end(), control) == controls.end())
      return false;
    if (control->composition_root) return true;
    control = control->parent;
  }
  return false;
}

uint32_t ParentOwner(const SurfaceControl* control) {
  if (control == nullptr) return 0;
  return control->parent != nullptr
      ? control->parent->owner_process_id
      : control->imported_parent_owner_process_id;
}

uint32_t ParentLayer(const SurfaceControl* control) {
  if (control == nullptr) return 0;
  return control->parent != nullptr ? control->parent->layer_id
                                    : control->imported_parent_layer_id;
}

bool CopyViewsLocked(
    const SurfaceTransaction* transaction,
    const std::vector<SurfaceControl*>& controls,
    std::vector<SurfaceControlStateView>* control_views,
    std::vector<SurfaceControlUpdateView>* update_views) {
  if (transaction == nullptr || control_views == nullptr ||
      update_views == nullptr) {
    return false;
  }
  try {
    control_views->clear();
    control_views->reserve(controls.size());
    for (SurfaceControl* control : controls) {
      if (control == nullptr) continue;
      control_views->push_back({
          .opaque = reinterpret_cast<ASurfaceControl*>(control),
          .owner_process_id = control->owner_process_id,
          .layer_id = control->layer_id,
          .parent_owner_process_id = ParentOwner(control),
          .parent_id = ParentLayer(control),
          .relative_parent_owner_process_id =
              control->relative_to == nullptr
                  ? 0 : control->relative_to->owner_process_id,
          .relative_parent_id = control->relative_to == nullptr
              ? 0 : control->relative_to->layer_id,
          .buffer = control->buffer,
          .name = control->name,
          .composition_root = control->composition_root,
          .attached_to_root = IsAttachedToCompositionRoot(control, controls),
          .has_geometry = control->has_geometry,
          .source = control->source,
          .destination = control->destination,
          .crop = control->crop,
          .has_crop = control->has_crop,
          .visible = control->visible,
          .position_x = control->position_x,
          .position_y = control->position_y,
          .transform = control->transform,
          .z_order = control->z_order,
          .scale_x = control->scale_x,
          .scale_y = control->scale_y,
          .alpha = control->alpha,
          .transparent_region = control->transparent_region,
      });
    }

    update_views->clear();
    update_views->reserve(transaction->updates.size());
    for (const auto& update : transaction->updates) {
      auto* control = reinterpret_cast<SurfaceControl*>(update.opaque);
      if (control == nullptr) continue;
      auto* parent = reinterpret_cast<SurfaceControl*>(update.parent);
      auto* relative_to =
          reinterpret_cast<SurfaceControl*>(update.relative_to);
      update_views->push_back({
          .opaque = update.opaque,
          .buffer = update.buffer,
          .has_buffer = update.has_buffer,
          .has_position = update.has_position,
          .position_x = update.position_x,
          .position_y = update.position_y,
          .has_z_order = update.has_z_order,
          .z_order = update.z_order,
          .has_alpha = update.has_alpha,
          .alpha = update.alpha,
          .has_scale = update.has_scale,
          .scale_x = update.scale_x,
          .scale_y = update.scale_y,
          .has_visibility = update.has_visibility,
          .visible = update.visible,
          .has_parent = update.has_parent,
          .parent_owner_process_id = parent == nullptr
              ? 0 : parent->owner_process_id,
          .parent_id = parent == nullptr ? 0 : parent->layer_id,
          .has_relative_layer = update.has_relative_layer,
          .relative_parent_owner_process_id = relative_to == nullptr
              ? 0 : relative_to->owner_process_id,
          .relative_parent_id = relative_to == nullptr
              ? 0 : relative_to->layer_id,
          .has_transform = update.has_transform,
          .transform = update.transform,
          .has_crop = update.has_crop,
          .crop = update.crop,
          .has_geometry = update.has_geometry,
          .source = update.source,
          .destination = update.destination,
          .has_damage = update.has_damage,
          .damage = update.damage,
          .has_transparent_region = update.has_transparent_region,
          .transparent_region = update.transparent_region,
      });
    }
    return true;
  } catch (...) {
    return false;
  }
}

}  // namespace

struct PreparedBufferMutation final {
  SurfaceControl* control = nullptr;
  LayerOccurrence* occurrence = nullptr;
  AHardwareBuffer* previous_control_buffer = nullptr;
  AHardwareBuffer* previous_occurrence_buffer = nullptr;
  bool previous_control_owned = false;
  bool previous_occurrence_owned = false;
  uint64_t previous_cookie = 0;
  AHardwareBuffer* next_buffer = nullptr;
  size_t next_reference_slot = std::numeric_limits<size_t>::max();
  size_t update_index = 0;
};

struct PreparedRelationMutation final {
  SurfaceControl* control = nullptr;
  SurfaceControl* old_parent = nullptr;
  SurfaceControl* new_parent = nullptr;
  SurfaceControl* old_relative = nullptr;
  SurfaceControl* new_relative = nullptr;
  size_t parent_reference_slot = std::numeric_limits<size_t>::max();
  size_t relative_reference_slot = std::numeric_limits<size_t>::max();
  uint32_t parent_owner_process_id = 0;
  uint32_t parent_id = 0;
  uint32_t relative_parent_owner_process_id = 0;
  uint32_t relative_parent_id = 0;
  bool composition_root = false;
};

// Prepare must never mutate the committed canonical occurrence table.  This
// value is the small shadow state used to model several updates to the same
// (owner, layer) occurrence in transaction order.  `actual` remains the
// stable map element that Finalize will update after acceptance.
struct SimulatedOccurrence final {
  LayerOccurrence* actual = nullptr;
  AHardwareBuffer* buffer = nullptr;
  bool buffer_reference_owned = false;
  uint64_t cookie = 0;
};

struct SimulatedControl final {
  AHardwareBuffer* buffer = nullptr;
  bool buffer_reference_owned = false;
};

struct PreparedSurfaceCommit::Impl final {
  SurfaceControlRegistry* registry = nullptr;
  SurfaceTransaction* transaction = nullptr;
  SurfaceTransactionStats* output_stats = nullptr;
  SurfaceControlSnapshot snapshot;
  SurfaceTransactionStats staged_stats;
  std::vector<SurfaceControlStateView> candidate_controls;
  std::unordered_map<SurfaceControl*, size_t> candidate_indices;
  std::unordered_map<SurfaceControl*, SurfaceControl*> candidate_parents;
  std::unordered_map<SurfaceControl*, SurfaceControl*> candidate_relatives;
  std::vector<PreparedBufferMutation> buffer_mutations;
  std::vector<PreparedRelationMutation> relation_mutations;
  std::vector<AHardwareBuffer*> prepared_buffer_references;
  std::vector<SurfaceControl*> prepared_relation_references;
  // Raw identities are kept separate until every allocation-sensitive
  // projection step has completed.  `touched_pins` contains only references
  // that were actually acquired.
  std::vector<SurfaceControl*> pending_touched_pins;
  std::vector<SurfaceControl*> touched_pins;
  std::vector<AHardwareBuffer*> release_buffers;
  std::vector<SurfaceControl*> release_relations;
  std::vector<AHardwareBuffer*> staged_stat_references;
  size_t release_buffer_count = 0;
  size_t release_relation_count = 0;
  bool active_reserved = false;
  bool finalized = false;

  ~Impl() {
    if (active_reserved) {
      std::lock_guard<std::mutex> lock(g_surface_controls_mutex);
      g_active_prepared_commit = false;
      active_reserved = false;
    }
    for (AHardwareBuffer*& buffer : prepared_buffer_references) {
      if (buffer != nullptr) {
        AHardwareBuffer_release(buffer);
        buffer = nullptr;
      }
    }
    for (SurfaceControl*& control : prepared_relation_references) {
      if (control != nullptr) {
        registry->Release(reinterpret_cast<ASurfaceControl*>(control));
        control = nullptr;
      }
    }
    for (SurfaceControl*& control : touched_pins) {
      if (control != nullptr) {
        registry->Release(reinterpret_cast<ASurfaceControl*>(control));
        control = nullptr;
      }
    }
    for (AHardwareBuffer*& buffer : staged_stat_references) {
      if (buffer != nullptr) {
        AHardwareBuffer_release(buffer);
        buffer = nullptr;
      }
    }
    // The staged maps contain borrowed keys/values while a plan is pending;
    // their retained references are tracked separately above.  Once a plan
    // is cancelled, discard the borrowed map entries without releasing them
    // a second time.  On successful finalization the maps have been swapped
    // into output_stats and this is already empty.
    staged_stats.previous_buffers.clear();
    staged_stats.previous_submission_cookies.clear();
  }

  void QueueBufferRelease(AHardwareBuffer* buffer) noexcept {
    if (buffer == nullptr) return;
    if (release_buffer_count >= release_buffers.size()) std::terminate();
    release_buffers[release_buffer_count++] = buffer;
  }

  void QueueRelationRelease(SurfaceControl* control) noexcept {
    if (control == nullptr) return;
    if (release_relation_count >= release_relations.size()) std::terminate();
    release_relations[release_relation_count++] = control;
  }
};

PreparedSurfaceCommit::~PreparedSurfaceCommit() { Cancel(); }

PreparedSurfaceCommit::PreparedSurfaceCommit() noexcept = default;

PreparedSurfaceCommit::PreparedSurfaceCommit(
    PreparedSurfaceCommit&& other) noexcept = default;

PreparedSurfaceCommit& PreparedSurfaceCommit::operator=(
    PreparedSurfaceCommit&& other) noexcept {
  if (this == &other) return *this;
  Cancel();
  impl_ = std::move(other.impl_);
  return *this;
}

const SurfaceControlSnapshot* PreparedSurfaceCommit::Snapshot() const noexcept {
  return impl_ == nullptr ? nullptr : &impl_->snapshot;
}

void PreparedSurfaceCommit::Cancel() noexcept {
  if (impl_ == nullptr) return;
  impl_.reset();
}

SurfaceControlRegistry& SurfaceControlRegistry::Instance() {
  static auto* registry = new SurfaceControlRegistry();
  return *registry;
}

ASurfaceControl* SurfaceControlRegistry::Create(
    ASurfaceControl* parent, const char* name, bool composition_root,
    uint32_t imported_owner_process_id, uint32_t imported_layer_id,
    uint32_t imported_parent_owner_process_id,
    uint32_t imported_parent_layer_id) {
  if ((imported_parent_owner_process_id == 0) !=
      (imported_parent_layer_id == 0)) return nullptr;
  std::unique_ptr<SurfaceControl> control(new (std::nothrow) SurfaceControl());
  if (control == nullptr) return nullptr;
  control->owner_process_id = imported_owner_process_id == 0
      ? static_cast<uint32_t>(getpid()) : imported_owner_process_id;
  control->layer_id = imported_layer_id == 0
      ? g_next_surface_layer_id.fetch_add(1, std::memory_order_relaxed)
      : imported_layer_id;
  try {
    if (name != nullptr) control->name = name;
    control->composition_root = composition_root;
    control->imported_parent_owner_process_id = imported_parent_owner_process_id;
    control->imported_parent_layer_id = imported_parent_layer_id;
    std::lock_guard<std::mutex> lock(g_surface_controls_mutex);
    // Allocate both registry entries before publishing ownership. If vector
    // growth fails, remove only the newly prepared, still-empty occurrence.
    const auto [occurrence, inserted] = g_layer_occurrences.try_emplace(
        LayerKey{control->owner_process_id, control->layer_id});
    try {
      g_surface_controls.push_back(control.get());
    } catch (...) {
      if (inserted) g_layer_occurrences.erase(occurrence);
      throw;
    }
    ++occurrence->second.wrappers;
    if (parent != nullptr) Acquire(parent);
    control->parent = reinterpret_cast<SurfaceControl*>(parent);
  } catch (const std::bad_alloc&) {
    return nullptr;
  } catch (const std::length_error&) {
    return nullptr;
  }
  return reinterpret_cast<ASurfaceControl*>(control.release());
}

void SurfaceControlRegistry::Acquire(ASurfaceControl* opaque) {
  auto* control = reinterpret_cast<SurfaceControl*>(opaque);
  if (control != nullptr)
    control->references.fetch_add(1, std::memory_order_relaxed);
}

void SurfaceControlRegistry::Release(ASurfaceControl* opaque) {
  auto* control = reinterpret_cast<SurfaceControl*>(opaque);
  if (control == nullptr ||
      control->references.fetch_sub(1, std::memory_order_acq_rel) != 1) {
    return;
  }
  SurfaceControl* parent = nullptr;
  SurfaceControl* relative_to = nullptr;
  AHardwareBuffer* canonical_buffer = nullptr;
  bool canonical_buffer_owned = false;
  {
    std::lock_guard<std::mutex> lock(g_surface_controls_mutex);
    std::erase(g_surface_controls, control);
    parent = std::exchange(control->parent, nullptr);
    relative_to = std::exchange(control->relative_to, nullptr);
    for (SurfaceControl* child : g_surface_controls) {
      if (child->parent == control) child->parent = nullptr;
      if (child->relative_to == control) child->relative_to = nullptr;
    }
    const LayerKey key{control->owner_process_id, control->layer_id};
    const auto occurrence = g_layer_occurrences.find(key);
    if (occurrence != g_layer_occurrences.end() && occurrence->second.wrappers > 0) {
      --occurrence->second.wrappers;
      if (occurrence->second.wrappers == 0) {
        canonical_buffer = occurrence->second.buffer;
        canonical_buffer_owned = occurrence->second.buffer_reference_owned;
        g_layer_occurrences.erase(occurrence);
      }
    }
  }
  if (canonical_buffer != nullptr && canonical_buffer_owned)
    AHardwareBuffer_release(canonical_buffer);
  if (control->buffer != nullptr && control->buffer_reference_owned)
    AHardwareBuffer_release(control->buffer);
  delete control;
  if (relative_to != nullptr) Release(reinterpret_cast<ASurfaceControl*>(relative_to));
  if (parent != nullptr) Release(reinterpret_cast<ASurfaceControl*>(parent));
}

bool SurfaceControlRegistry::GetIdentity(
    const ASurfaceControl* opaque, uint32_t* owner_process_id,
    uint32_t* layer_id) const {
  const auto* control = reinterpret_cast<const SurfaceControl*>(opaque);
  if (control == nullptr || owner_process_id == nullptr || layer_id == nullptr)
    return false;
  std::lock_guard<std::mutex> lock(g_surface_controls_mutex);
  if (std::find(g_surface_controls.begin(), g_surface_controls.end(), control) ==
      g_surface_controls.end()) {
    return false;
  }
  *owner_process_id = control->owner_process_id;
  *layer_id = control->layer_id;
  return true;
}

size_t SurfaceControlRegistry::CopyTransparentRegion(
    const ASurfaceControl* opaque, int32_t* rects, size_t capacity) const {
  const auto* control = reinterpret_cast<const SurfaceControl*>(opaque);
  if (control == nullptr) return 0;
  std::lock_guard<std::mutex> lock(g_surface_controls_mutex);
  const size_t count = control->transparent_region.size();
  if (rects == nullptr || capacity == 0) return count;
  const size_t copied = std::min(count, capacity);
  for (size_t index = 0; index < copied; ++index) {
    const ARect& rect = control->transparent_region[index];
    rects[index * 4 + 0] = rect.left;
    rects[index * 4 + 1] = rect.top;
    rects[index * 4 + 2] = rect.right;
    rects[index * 4 + 3] = rect.bottom;
  }
  return count;
}

bool SurfaceControlRegistry::CopyViews(
    const SurfaceTransaction* transaction,
    std::vector<SurfaceControlStateView>* control_views,
    std::vector<SurfaceControlUpdateView>* update_views) const {
  std::lock_guard<std::mutex> lock(g_surface_controls_mutex);
  return CopyViewsLocked(transaction, g_surface_controls, control_views,
                         update_views);
}

bool SurfaceControlRegistry::CaptureSnapshot(
    const SurfaceTransaction* transaction, uint32_t local_process_id,
    bool central_surfaceflinger, SurfaceControlSnapshot* snapshot) const {
  if (snapshot == nullptr) return false;
  std::lock_guard<std::mutex> lock(g_surface_controls_mutex);
  std::vector<SurfaceControlStateView> control_views;
  std::vector<SurfaceControlUpdateView> update_views;
  if (!CopyViewsLocked(transaction, g_surface_controls, &control_views,
                       &update_views)) {
    return false;
  }
  return BuildSurfaceControlSnapshot(
      control_views, update_views, local_process_id, central_surfaceflinger,
      snapshot);
}

bool SurfaceControlRegistry::PrepareSurfaceCommit(
    SurfaceTransaction* transaction, uint32_t local_process_id,
    bool central_surfaceflinger, SurfaceTransactionStats* stats,
    PreparedSurfaceCommit* prepared) const {
  if (transaction == nullptr || stats == nullptr || prepared == nullptr ||
      !stats->controls.empty() ||
      !stats->previous_buffers.empty() ||
      !stats->previous_submission_cookies.empty() || stats->present_fence >= 0)
    return false;
  prepared->Cancel();

  auto plan = std::unique_ptr<PreparedSurfaceCommit::Impl>(
      new (std::nothrow) PreparedSurfaceCommit::Impl());
  if (plan == nullptr) return false;
  plan->registry = const_cast<SurfaceControlRegistry*>(this);
  plan->transaction = transaction;
  plan->output_stats = stats;

  std::lock_guard<std::mutex> lock(g_surface_controls_mutex);
  if (g_active_prepared_commit) return false;
  try {
    g_active_prepared_commit = true;
    plan->active_reserved = true;
    const auto known = [](SurfaceControl* control) {
      return control != nullptr &&
             std::find(g_surface_controls.begin(), g_surface_controls.end(),
                       control) != g_surface_controls.end();
    };
    for (const auto& update : transaction->updates) {
      auto* control = reinterpret_cast<SurfaceControl*>(update.opaque);
      auto* parent = reinterpret_cast<SurfaceControl*>(update.parent);
      auto* relative = reinterpret_cast<SurfaceControl*>(update.relative_to);
      if (!known(control) || (parent != nullptr && !known(parent)) ||
          (relative != nullptr && !known(relative))) {
        return false;
      }
    }
    for (ASurfaceControl* control : transaction->controls) {
      if (control != nullptr &&
          !known(reinterpret_cast<SurfaceControl*>(control))) {
        return false;
      }
    }

    std::vector<SurfaceControlUpdateView> update_views;
    if (!CopyViewsLocked(transaction, g_surface_controls,
                         &plan->candidate_controls, &update_views)) {
      return false;
    }
    plan->staged_stats.controls = transaction->controls;
    plan->candidate_indices.reserve(plan->candidate_controls.size());
    plan->candidate_parents.reserve(plan->candidate_controls.size());
    plan->candidate_relatives.reserve(plan->candidate_controls.size());
    for (auto& view : plan->candidate_controls) {
      auto* control = reinterpret_cast<SurfaceControl*>(view.opaque);
      plan->candidate_indices.emplace(control,
                                     &view - plan->candidate_controls.data());
      plan->candidate_parents.emplace(control, control->parent);
      plan->candidate_relatives.emplace(control, control->relative_to);
    }

    std::unordered_map<SurfaceControl*, SimulatedControl> simulated_buffers;
    std::unordered_map<LayerKey, SimulatedOccurrence, LayerKeyHash>
        simulated_occurrences;
    simulated_buffers.reserve(transaction->updates.size());
    simulated_occurrences.reserve(transaction->updates.size());
    for (size_t update_index = 0; update_index < transaction->updates.size();
         ++update_index) {
      const auto& update = transaction->updates[update_index];
      auto* control = reinterpret_cast<SurfaceControl*>(update.opaque);
      const auto control_index = plan->candidate_indices.find(control);
      if (control_index == plan->candidate_indices.end()) return false;
      auto candidate = plan->candidate_controls.begin() +
          static_cast<std::ptrdiff_t>(control_index->second);
      if (update_index >= update_views.size() ||
          !ApplySurfaceControlUpdateOverlay(&*candidate,
                                            update_views[update_index])) {
        return false;
      }
      if (update.has_parent) {
        auto* parent = reinterpret_cast<SurfaceControl*>(update.parent);
        plan->candidate_parents[control] = parent;
        candidate->composition_root = false;
      }
      // The absolute z-order form clears a prior relative layer.  A later
      // relative-layer field in the same update wins, matching the overlay.
      if (update.has_z_order && !update.has_relative_layer)
        plan->candidate_relatives[control] = nullptr;
      if (update.has_relative_layer) {
        plan->candidate_relatives[control] =
            reinterpret_cast<SurfaceControl*>(update.relative_to);
      }

      if (update.has_buffer) {
        const LayerKey key{control->owner_process_id, control->layer_id};
        const auto occurrence = g_layer_occurrences.find(key);
        if (occurrence == g_layer_occurrences.end()) return false;
        auto simulated = simulated_occurrences.find(key);
        if (simulated == simulated_occurrences.end()) {
          simulated = simulated_occurrences
                          .emplace(key, SimulatedOccurrence{
                                           .actual = &occurrence->second,
                                           .buffer = occurrence->second.buffer,
                                           .buffer_reference_owned =
                                               occurrence->second
                                                   .buffer_reference_owned,
                                           .cookie = occurrence->second.cookie})
                          .first;
        }
        auto buffer = simulated_buffers.find(control);
        if (buffer == simulated_buffers.end()) {
          buffer = simulated_buffers
                       .emplace(control, SimulatedControl{
                                           .buffer = control->buffer,
                                           .buffer_reference_owned =
                                               control->buffer_reference_owned})
                       .first;
        }
        plan->buffer_mutations.push_back({
            .control = control,
            .occurrence = simulated->second.actual,
            .previous_control_buffer = buffer->second.buffer,
            .previous_occurrence_buffer = simulated->second.buffer,
            .previous_control_owned = buffer->second.buffer_reference_owned,
            .previous_occurrence_owned = simulated->second.buffer_reference_owned,
            .previous_cookie = simulated->second.cookie,
            .next_buffer = update.buffer,
            .update_index = update_index,
        });
        buffer->second.buffer = update.buffer;
        buffer->second.buffer_reference_owned = update.buffer != nullptr;
        simulated->second.buffer = update.buffer;
        // The canonical shadow represents the reference that would be owned
        // after this update.  The committed occurrence remains untouched.
        simulated->second.buffer_reference_owned = update.buffer != nullptr;
        simulated->second.cookie = update.submission_cookie;
      }
    }

    // Every touched control keeps its stable object and map occurrence alive
    // through transport, frontend acceptance and finalization.
    std::unordered_set<SurfaceControl*> touched;
    touched.reserve(transaction->updates.size());
    for (const auto& update : transaction->updates) {
      auto* control = reinterpret_cast<SurfaceControl*>(update.opaque);
      if (touched.insert(control).second)
        plan->pending_touched_pins.push_back(control);
    }

    // Project attachment reachability only after the complete update overlay;
    // a child may be reparented before its new parent is updated in the same
    // transaction.
    for (auto& view : plan->candidate_controls) {
      auto* control = reinterpret_cast<SurfaceControl*>(view.opaque);
      view.attached_to_root = false;
      std::unordered_set<SurfaceControl*> seen;
      seen.reserve(plan->candidate_controls.size());
      for (SurfaceControl* cursor = control; cursor != nullptr;) {
        if (!seen.insert(cursor).second) break;
        const auto index = plan->candidate_indices.find(cursor);
        if (index == plan->candidate_indices.end()) break;
        const auto& candidate = plan->candidate_controls[index->second];
        if (candidate.composition_root) {
          view.attached_to_root = true;
          break;
        }
        cursor = plan->candidate_parents[cursor];
      }
    }

    plan->relation_mutations.reserve(touched.size());
    for (SurfaceControl* control : plan->pending_touched_pins) {
      const auto index = plan->candidate_indices.find(control);
      if (index == plan->candidate_indices.end()) return false;
      const auto& candidate = plan->candidate_controls[index->second];
      const SurfaceControl* new_parent = plan->candidate_parents.at(control);
      const SurfaceControl* new_relative =
          plan->candidate_relatives.at(control);
      const bool parent_changed =
          new_parent != control->parent ||
          candidate.parent_owner_process_id != ParentOwner(control) ||
          candidate.parent_id != ParentLayer(control) ||
          candidate.composition_root != control->composition_root;
      const bool relative_changed =
          new_relative != control->relative_to ||
          candidate.relative_parent_owner_process_id !=
              (control->relative_to == nullptr
                   ? 0 : control->relative_to->owner_process_id) ||
          candidate.relative_parent_id !=
              (control->relative_to == nullptr ? 0
                                                : control->relative_to->layer_id);
      if (!parent_changed && !relative_changed) continue;
      plan->relation_mutations.push_back({
          .control = control,
          .old_parent = control->parent,
          .new_parent = const_cast<SurfaceControl*>(new_parent),
          .old_relative = control->relative_to,
          .new_relative = const_cast<SurfaceControl*>(new_relative),
          .parent_owner_process_id = candidate.parent_owner_process_id,
          .parent_id = candidate.parent_id,
          .relative_parent_owner_process_id =
              candidate.relative_parent_owner_process_id,
          .relative_parent_id = candidate.relative_parent_id,
          .composition_root = candidate.composition_root,
      });
    }

    plan->snapshot = {};
    if (!BuildSurfaceControlSnapshot(
            plan->candidate_controls, update_views, local_process_id,
            central_surfaceflinger, &plan->snapshot)) {
      return false;
    }

    // Preallocate every finalize-time container before any retained resource
    // is acquired. No map/vector growth is permitted after this point.
    plan->prepared_buffer_references.resize(
        plan->buffer_mutations.size(), nullptr);
    plan->prepared_relation_references.resize(
        plan->relation_mutations.size() * 2, nullptr);
    plan->release_buffers.resize(plan->buffer_mutations.size() * 2);
    plan->release_relations.resize(plan->relation_mutations.size() * 2 +
                                   plan->pending_touched_pins.size());
    for (const auto& mutation : plan->buffer_mutations) {
      const auto& update = transaction->updates[mutation.update_index];
      plan->staged_stats.previous_buffers.erase(update.opaque);
      plan->staged_stats.previous_submission_cookies.erase(update.opaque);
      if (mutation.previous_occurrence_buffer != nullptr) {
        plan->staged_stats.previous_buffers.emplace(
            update.opaque, mutation.previous_occurrence_buffer);
        plan->staged_stats.previous_submission_cookies.emplace(
            update.opaque, mutation.previous_cookie);
      }
    }

    // Allocate the staged-reference ledger before acquiring anything.  The
    // map values themselves stay borrowed until every allocation-sensitive
    // preparation step has completed.
    plan->staged_stat_references.resize(
        plan->staged_stats.previous_buffers.size(), nullptr);

    size_t relation_slot = 0;
    for (auto& mutation : plan->relation_mutations) {
      if (mutation.new_parent != nullptr &&
          mutation.new_parent != mutation.old_parent) {
        mutation.parent_reference_slot = relation_slot++;
        const_cast<SurfaceControlRegistry*>(this)->Acquire(
            reinterpret_cast<ASurfaceControl*>(mutation.new_parent));
        plan->prepared_relation_references[mutation.parent_reference_slot] =
            mutation.new_parent;
      }
      if (mutation.new_relative != nullptr &&
          mutation.new_relative != mutation.old_relative) {
        mutation.relative_reference_slot = relation_slot++;
        const_cast<SurfaceControlRegistry*>(this)->Acquire(
            reinterpret_cast<ASurfaceControl*>(mutation.new_relative));
        plan->prepared_relation_references[mutation.relative_reference_slot] =
            mutation.new_relative;
      }
    }
    plan->prepared_relation_references.resize(relation_slot);

    // The raw candidate list is complete and all its vector capacity is
    // fixed.  Populate the owned ledger only after each pin is acquired so a
    // failure before this point cannot release an unowned control.
    plan->touched_pins.resize(plan->pending_touched_pins.size(), nullptr);
    for (size_t index = 0; index < plan->pending_touched_pins.size(); ++index) {
      SurfaceControl* control = plan->pending_touched_pins[index];
      const_cast<SurfaceControlRegistry*>(this)->Acquire(
          reinterpret_cast<ASurfaceControl*>(control));
      plan->touched_pins[index] = control;
    }

    size_t buffer_slot = 0;
    for (auto& mutation : plan->buffer_mutations) {
      if (mutation.next_buffer != nullptr) {
        mutation.next_reference_slot = buffer_slot;
        // SetBuffer already owns the transaction-side reference.  Prepare
        // retains only the independent canonical occurrence reference; on
        // Finalize the transaction reference transfers to the control.
        AHardwareBuffer_acquire(mutation.next_buffer);
        plan->prepared_buffer_references[buffer_slot++] = mutation.next_buffer;
      }
    }
    plan->prepared_buffer_references.resize(buffer_slot);
    size_t stats_slot = 0;
    for (const auto& [control, buffer] : plan->staged_stats.previous_buffers) {
      (void)control;
      AHardwareBuffer_acquire(buffer);
      plan->staged_stat_references[stats_slot++] = buffer;
    }
    plan->output_stats = stats;
    prepared->impl_ = std::move(plan);
    return true;
  } catch (...) {
    return false;
  }
}

bool SurfaceControlRegistry::FinalizeSurfaceCommit(
    PreparedSurfaceCommit* prepared) noexcept {
  if (prepared == nullptr || prepared->impl_ == nullptr) return false;
  auto* plan = prepared->impl_.get();
  if (plan->finalized || plan->registry == nullptr ||
      plan->transaction == nullptr || plan->output_stats == nullptr) {
    return false;
  }

  {
    std::lock_guard<std::mutex> lock(g_surface_controls_mutex);
    for (SurfaceControl* control : plan->touched_pins) {
      const auto index = plan->candidate_indices.find(control);
      if (index == plan->candidate_indices.end()) return false;
      auto& candidate = plan->candidate_controls[index->second];
      control->source = candidate.source;
      control->destination = candidate.destination;
      control->crop = candidate.crop;
      control->has_geometry = candidate.has_geometry;
      control->has_crop = candidate.has_crop;
      control->visible = candidate.visible;
      control->position_x = candidate.position_x;
      control->position_y = candidate.position_y;
      control->transform = candidate.transform;
      control->z_order = candidate.z_order;
      control->scale_x = candidate.scale_x;
      control->scale_y = candidate.scale_y;
      control->alpha = candidate.alpha;
      control->transparent_region = std::move(candidate.transparent_region);
    }
    for (auto& mutation : plan->relation_mutations) {
      mutation.control->parent = mutation.new_parent;
      mutation.control->relative_to = mutation.new_relative;
      mutation.control->imported_parent_owner_process_id =
          mutation.parent_owner_process_id;
      mutation.control->imported_parent_layer_id = mutation.parent_id;
      mutation.control->composition_root = mutation.composition_root;
      if (mutation.parent_reference_slot !=
          std::numeric_limits<size_t>::max()) {
        plan->prepared_relation_references[mutation.parent_reference_slot] =
            nullptr;
      }
      if (mutation.relative_reference_slot !=
          std::numeric_limits<size_t>::max()) {
        plan->prepared_relation_references[mutation.relative_reference_slot] =
            nullptr;
      }
      if (mutation.old_parent != nullptr &&
          mutation.old_parent != mutation.new_parent) {
        plan->QueueRelationRelease(mutation.old_parent);
      }
      if (mutation.old_relative != nullptr &&
          mutation.old_relative != mutation.new_relative) {
        plan->QueueRelationRelease(mutation.old_relative);
      }
    }

    for (const auto& mutation : plan->buffer_mutations) {
      auto& update = plan->transaction->updates[mutation.update_index];
      if (mutation.previous_control_buffer != nullptr &&
          mutation.previous_control_owned) {
        plan->QueueBufferRelease(mutation.previous_control_buffer);
      }
      if (mutation.previous_occurrence_buffer != nullptr &&
          mutation.previous_occurrence_owned) {
        plan->QueueBufferRelease(mutation.previous_occurrence_buffer);
      }
      mutation.control->buffer = mutation.next_buffer;
      mutation.control->buffer_reference_owned = mutation.next_buffer != nullptr;
      mutation.occurrence->buffer = mutation.next_buffer;
      mutation.occurrence->buffer_reference_owned =
          mutation.next_buffer != nullptr;
      mutation.occurrence->cookie = update.submission_cookie;
      if (mutation.next_buffer != nullptr) {
        plan->prepared_buffer_references[mutation.next_reference_slot] = nullptr;
      }
      update.buffer = nullptr;
    }
    plan->output_stats->previous_buffers.swap(
        plan->staged_stats.previous_buffers);
    plan->output_stats->previous_submission_cookies.swap(
        plan->staged_stats.previous_submission_cookies);
    plan->output_stats->controls.swap(plan->staged_stats.controls);
    for (AHardwareBuffer*& buffer : plan->staged_stat_references)
      buffer = nullptr;
    plan->finalized = true;
    g_active_prepared_commit = false;
    plan->active_reserved = false;
  }

  for (size_t index = 0; index < plan->release_buffer_count; ++index)
    AHardwareBuffer_release(plan->release_buffers[index]);
  for (size_t index = 0; index < plan->release_relation_count; ++index)
    plan->registry->Release(
        reinterpret_cast<ASurfaceControl*>(plan->release_relations[index]));
  for (SurfaceControl*& control : plan->touched_pins) {
    if (control != nullptr) {
      plan->registry->Release(reinterpret_cast<ASurfaceControl*>(control));
      control = nullptr;
    }
  }
  return true;
}

void SurfaceControlRegistry::CancelSurfaceCommit(
    PreparedSurfaceCommit* prepared) const noexcept {
  if (prepared == nullptr) return;
  prepared->Cancel();
}

bool SurfaceControlRegistry::ApplyAcceptedTransaction(
    SurfaceTransaction* transaction, SurfaceTransactionStats* stats) {
  if (transaction == nullptr || stats == nullptr) return false;
  std::vector<SurfaceControl*> replaced_relationships;
  {
    std::lock_guard<std::mutex> lock(g_surface_controls_mutex);
    if (g_active_prepared_commit) return false;
    for (auto& update : transaction->updates) {
      auto* control = reinterpret_cast<SurfaceControl*>(update.opaque);
      if (control == nullptr) continue;
      if (update.has_buffer) {
        const LayerKey key{control->owner_process_id, control->layer_id};
        auto occurrence = g_layer_occurrences.find(key);
        if (occurrence == g_layer_occurrences.end()) {
          occurrence = g_layer_occurrences
                          .emplace(key, LayerOccurrence{})
                          .first;
        }
        const uint64_t previous_occurrence = occurrence->second.cookie;
        AHardwareBuffer* previous_buffer = occurrence->second.buffer;
        // Pointer equality is deliberately not a replacement filter: the
        // same AHardwareBuffer can be submitted again at a new occurrence.
        if (previous_buffer != nullptr) {
          AHardwareBuffer_acquire(previous_buffer);
          stats->previous_buffers[update.opaque] = previous_buffer;
          stats->previous_submission_cookies[update.opaque] =
              previous_occurrence;
        }
        if (update.buffer != nullptr) {
          // SetBuffer owns the transaction-side reference.  Retain only the
          // independent canonical occurrence reference; the transaction
          // reference transfers to this wrapper when the update is consumed.
          AHardwareBuffer_acquire(update.buffer);
        }
        if (previous_buffer != nullptr && occurrence->second.buffer_reference_owned)
          AHardwareBuffer_release(previous_buffer);
        if (control->buffer != nullptr && control->buffer_reference_owned)
          AHardwareBuffer_release(control->buffer);
        control->buffer = update.buffer;
        control->buffer_reference_owned = update.buffer != nullptr;
        occurrence->second.buffer = update.buffer;
        occurrence->second.buffer_reference_owned = update.buffer != nullptr;
        occurrence->second.cookie = update.submission_cookie;
        update.buffer = nullptr;
      }
      if (update.has_geometry) {
        control->source = update.source;
        control->destination = update.destination;
        control->has_geometry = true;
      }
      if (update.has_crop) {
        control->crop = update.crop;
        control->has_crop = true;
      }
      if (update.has_visibility) control->visible = update.visible;
      if (update.has_position) {
        control->position_x = update.position_x;
        control->position_y = update.position_y;
      }
      if (update.has_transform) control->transform = update.transform;
      if (update.has_z_order) {
        control->z_order = update.z_order;
        if (control->relative_to != nullptr) {
          replaced_relationships.push_back(control->relative_to);
          control->relative_to = nullptr;
        }
      }
      if (update.has_relative_layer) {
        control->z_order = update.z_order;
        auto* relative_to = reinterpret_cast<SurfaceControl*>(update.relative_to);
        if (control->relative_to != relative_to) {
          if (relative_to != nullptr) {
            Acquire(reinterpret_cast<ASurfaceControl*>(relative_to));
          }
          if (control->relative_to != nullptr)
            replaced_relationships.push_back(control->relative_to);
          control->relative_to = relative_to;
        }
      }
      if (update.has_scale) {
        control->scale_x = update.scale_x;
        control->scale_y = update.scale_y;
      }
      if (update.has_alpha) control->alpha = update.alpha;
      if (update.has_transparent_region)
        control->transparent_region = update.transparent_region;
      if (update.has_parent) {
        auto* parent = reinterpret_cast<SurfaceControl*>(update.parent);
        if (control->parent != parent) {
          if (parent != nullptr)
            Acquire(reinterpret_cast<ASurfaceControl*>(parent));
          if (control->parent != nullptr) replaced_relationships.push_back(control->parent);
          control->parent = parent;
        }
        control->imported_parent_owner_process_id = 0;
        control->imported_parent_layer_id = 0;
        control->composition_root = false;
      }
    }
  }
  for (SurfaceControl* relationship : replaced_relationships)
    Release(reinterpret_cast<ASurfaceControl*>(relationship));
  return true;
}

}  // namespace darwin_art::window
