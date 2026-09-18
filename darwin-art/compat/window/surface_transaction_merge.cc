#include "surface_transaction_merge.h"

#include <algorithm>
#include <limits>
#include <new>
#include <stdexcept>
#include <utility>

namespace darwin_art::window {
namespace {

bool AddWillOverflow(size_t left, size_t right) {
  return right > std::numeric_limits<size_t>::max() - left;
}

SurfaceTransaction::Update* FindUpdate(SurfaceTransaction* transaction,
                                       ASurfaceControl* control) {
  if (transaction == nullptr || control == nullptr) return nullptr;
  const auto found = std::find_if(
      transaction->updates.begin(), transaction->updates.end(),
      [control](const SurfaceTransaction::Update& update) {
        return update.opaque == control;
      });
  if (found != transaction->updates.end()) return &*found;
  transaction->updates.push_back({.opaque = control});
  return &transaction->updates.back();
}

void MoveMatchingBufferCallbacks(SurfaceTransaction* destination,
                                 SurfaceTransaction* disposal,
                                 ASurfaceControl* control) {
  for (auto it = destination->buffer_callbacks.begin();
       it != destination->buffer_callbacks.end();) {
    if (it->control == control) {
      disposal->buffer_callbacks.push_back(*it);
      it = destination->buffer_callbacks.erase(it);
    } else {
      ++it;
    }
  }
}

void MoveDisplacedBuffer(SurfaceTransaction* destination,
                         SurfaceTransaction::Update* merged,
                         SurfaceTransaction* disposal) {
  // A destination callback must be discarded when its buffer is replaced.
  // Move the old ownership into disposal so deletion after this structural
  // operation performs the existing fence forwarding and callback contract.
  const bool has_matching_callback = std::any_of(
      destination->buffer_callbacks.begin(), destination->buffer_callbacks.end(),
      [control = merged->opaque](const SurfaceTransaction::BufferCallback& callback) {
        return callback.control == control;
      });
  if (!merged->has_buffer && !has_matching_callback) return;
  SurfaceTransaction::Update* old = FindUpdate(disposal, merged->opaque);
  if (old == nullptr) return;
  old->opaque = merged->opaque;
  old->has_buffer = merged->has_buffer;
  old->buffer = merged->buffer;
  old->acquire_fence = merged->acquire_fence;
  old->submission_cookie = merged->submission_cookie;
  merged->buffer = nullptr;
  merged->acquire_fence = -1;
  merged->submission_cookie = 0;
  merged->has_buffer = false;
  MoveMatchingBufferCallbacks(destination, disposal, merged->opaque);
}

bool ReserveForMerge(SurfaceTransaction* destination,
                     SurfaceTransaction* source,
                     SurfaceTransaction* disposal) {
  if (!disposal->controls.empty() || !disposal->updates.empty() ||
      !disposal->commits.empty() || !disposal->completes.empty() ||
      !disposal->discards.empty() || !disposal->buffer_callbacks.empty()) {
    return false;
  }
  if (AddWillOverflow(destination->controls.size(), source->controls.size()) ||
      AddWillOverflow(destination->updates.size(), source->updates.size()) ||
      AddWillOverflow(destination->commits.size(), source->commits.size()) ||
      AddWillOverflow(destination->completes.size(), source->completes.size()) ||
      AddWillOverflow(destination->discards.size(), source->discards.size()) ||
      AddWillOverflow(destination->buffer_callbacks.size(),
                      source->buffer_callbacks.size())) {
    return false;
  }
  // Every source control may be a duplicate and every destination callback may
  // be displaced. These are conservative upper bounds and are reserved before
  // ownership is changed so allocation failure leaves all three transactions
  // untouched.
  disposal->controls.reserve(source->controls.size());
  disposal->updates.reserve(source->updates.size());
  disposal->buffer_callbacks.reserve(destination->buffer_callbacks.size());
  destination->controls.reserve(destination->controls.size() +
                                source->controls.size());
  destination->updates.reserve(destination->updates.size() +
                               source->updates.size());
  destination->commits.reserve(destination->commits.size() +
                               source->commits.size());
  destination->completes.reserve(destination->completes.size() +
                                 source->completes.size());
  destination->discards.reserve(destination->discards.size() +
                                source->discards.size());
  destination->buffer_callbacks.reserve(destination->buffer_callbacks.size() +
                                        source->buffer_callbacks.size());
  return true;
}

}  // namespace

bool MergeSurfaceTransactions(SurfaceTransaction* destination,
                              SurfaceTransaction* source,
                              SurfaceTransaction* disposal) noexcept {
  if (destination == nullptr || source == nullptr || destination == source)
    return true;
  if (disposal == nullptr) return false;
  try {
    if (!ReserveForMerge(destination, source, disposal)) return false;
    // Transfer every source control reference. A duplicate is still an owned
    // reference from source and therefore belongs in disposal, not release.
    for (ASurfaceControl* control : source->controls) {
      if (std::find(destination->controls.begin(), destination->controls.end(),
                    control) == destination->controls.end()) {
        destination->controls.push_back(control);
      } else {
        disposal->controls.push_back(control);
      }
    }

    for (auto& incoming : source->updates) {
      SurfaceTransaction::Update* merged =
          FindUpdate(destination, incoming.opaque);
      if (merged == nullptr) continue;
      if (incoming.has_buffer) {
        MoveDisplacedBuffer(destination, merged, disposal);
        merged->buffer = incoming.buffer;
        merged->acquire_fence = incoming.acquire_fence;
        merged->submission_cookie = incoming.submission_cookie;
        merged->has_buffer = true;
        incoming.buffer = nullptr;
        incoming.acquire_fence = -1;
        incoming.submission_cookie = 0;
      }
#define MERGE_SURFACE_FIELD(flag, field) \
    if (incoming.flag) {                  \
      merged->flag = true;                \
      merged->field = incoming.field;     \
    }
      MERGE_SURFACE_FIELD(has_geometry, source)
      if (incoming.has_geometry) merged->destination = incoming.destination;
      MERGE_SURFACE_FIELD(has_crop, crop)
      MERGE_SURFACE_FIELD(has_visibility, visible)
      MERGE_SURFACE_FIELD(has_position, position_x)
      if (incoming.has_position) merged->position_y = incoming.position_y;
      MERGE_SURFACE_FIELD(has_transform, transform)
      MERGE_SURFACE_FIELD(has_z_order, z_order)
      MERGE_SURFACE_FIELD(has_scale, scale_x)
      if (incoming.has_scale) merged->scale_y = incoming.scale_y;
      MERGE_SURFACE_FIELD(has_alpha, alpha)
      MERGE_SURFACE_FIELD(has_parent, parent)
#undef MERGE_SURFACE_FIELD
      if (incoming.has_z_order || incoming.has_relative_layer) {
        merged->has_z_order = incoming.has_z_order;
        merged->has_relative_layer = incoming.has_relative_layer;
        merged->relative_to = incoming.relative_to;
        merged->z_order = incoming.z_order;
      }
      if (incoming.has_damage) {
        merged->has_damage = true;
        merged->damage = std::move(incoming.damage);
      }
      if (incoming.has_transparent_region) {
        merged->has_transparent_region = true;
        merged->transparent_region = std::move(incoming.transparent_region);
      }
    }

    destination->commits.insert(destination->commits.end(),
                                source->commits.begin(), source->commits.end());
    destination->completes.insert(destination->completes.end(),
                                  source->completes.begin(),
                                  source->completes.end());
    destination->discards.insert(destination->discards.end(),
                                 source->discards.begin(), source->discards.end());
    destination->buffer_callbacks.insert(destination->buffer_callbacks.end(),
                                         source->buffer_callbacks.begin(),
                                         source->buffer_callbacks.end());
    source->buffer_callbacks.clear();
    source->controls.clear();
    source->updates.clear();
    source->commits.clear();
    source->completes.clear();
    source->discards.clear();
    return true;
  } catch (const std::bad_alloc&) {
    return false;
  } catch (const std::length_error&) {
    return false;
  }
}

}  // namespace darwin_art::window
