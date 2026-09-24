#include "output_registry.h"

#include <IOSurface/IOSurface.h>

#include <cerrno>
#include <cstdlib>
#include <iterator>
#include <limits>

namespace darwin_art::surfaceflinger {

OutputRegistry::OutputRegistry() {
  do {
    arc4random_buf(&server_instance_, sizeof(server_instance_));
  } while (server_instance_ == 0);
}

OutputRegistry::~OutputRegistry() {
  for (const auto& [connection, epoch] : owners_) {
    (void)connection;
    epoch->current_.store(false, std::memory_order_release);
  }
}

OutputTransition OutputRegistry::Apply(int connection, const OutputRequest& request) {
  std::lock_guard<std::mutex> lock(mutex_);
  OutputTransition result;
  auto reject = [&](int error) {
    result.response.status = error;
    return result;
  };
  if (connection < 0 || request.magic != kOutputMagic ||
      request.version != kOutputProtocolVersion || request.reserved != 0)
    return reject(EINVAL);
  auto existing = owners_.find(connection);
  const bool registering = request.operation == OutputOperation::Register;
  if (registering) {
    if (existing != owners_.end()) return reject(EALREADY);
    if (request.token != OutputToken{} || request.generation != 0)
      return reject(EINVAL);
    if (owners_.size() >= kCapacity) return reject(ENOSPC);
  } else {
    if (existing == owners_.end()) return reject(ENOENT);
    const auto& identity = existing->second->identity();
    if (request.token != identity.token || request.generation != identity.generation)
      return reject(ESTALE);
    if (request.operation == OutputOperation::Retire) {
      result.previous = existing->second;
      result.response.token = identity.token;
      result.response.generation = identity.generation;
      existing->second->current_.store(false, std::memory_order_release);
      by_surface_.erase(identity.iosurface_id);
      replaced_.erase(connection);
      owners_.erase(existing);
      return result;
    }
    if (request.operation != OutputOperation::Replace) return reject(EINVAL);
  }
  if (request.iosurface_id == 0 || request.physical_width == 0 ||
      request.physical_height == 0 || request.logical_width == 0 ||
      request.logical_height == 0) return reject(EINVAL);
  auto occupied = by_surface_.find(request.iosurface_id);
  if (occupied != by_surface_.end() &&
      (registering || occupied->second != existing->second)) return reject(EEXIST);
  auto backing = IosurfaceBacking::Import(request.iosurface_id);
  if (!backing) return reject(ENOENT);
  auto surface = static_cast<IOSurfaceRef>(backing->native_surface());
  if (IOSurfaceGetWidth(surface) != request.physical_width ||
      IOSurfaceGetHeight(surface) != request.physical_height) return reject(EINVAL);
  OutputRequest identity = request;
  if (registering) {
    if (next_serial_ == 0) return reject(EOVERFLOW);
    identity.token = {server_instance_, next_serial_++};
    identity.generation = 1;
  } else {
    if (request.generation == std::numeric_limits<uint64_t>::max())
      return reject(EOVERFLOW);
    identity.generation = request.generation + 1;
    result.previous = existing->second;
  }
  auto epoch = std::shared_ptr<OutputEpoch>(new OutputEpoch(identity, std::move(backing)));
  // Finish potentially allocating the new index before invalidating the old
  // epoch. Rejected imports never disturb existing output ownership.
  by_surface_.insert_or_assign(identity.iosurface_id, epoch);
  ForgetReplacedId(identity.iosurface_id);
  if (registering) {
    try {
      owners_.emplace(connection, epoch);
    } catch (...) {
      by_surface_.erase(identity.iosurface_id);
      throw;
    }
  } else {
    existing->second->current_.store(false, std::memory_order_release);
    const uint32_t previous_id = existing->second->identity().iosurface_id;
    if (previous_id != identity.iosurface_id) {
      by_surface_.erase(previous_id);
      auto& history = replaced_[connection];
      history.push_back(previous_id);
      if (history.size() > kReplacedHistory) history.pop_front();
    }
    existing->second = epoch;
  }
  result.current = epoch;
  result.response.token = identity.token;
  result.response.generation = identity.generation;
  return result;
}

OutputTransition OutputRegistry::Drop(int connection) {
  std::lock_guard<std::mutex> lock(mutex_);
  OutputTransition result;
  const auto existing = owners_.find(connection);
  if (existing == owners_.end()) return result;
  result.previous = existing->second;
  existing->second->current_.store(false, std::memory_order_release);
  by_surface_.erase(existing->second->identity().iosurface_id);
  replaced_.erase(connection);
  owners_.erase(existing);
  return result;
}

void OutputRegistry::ForgetReplacedId(uint32_t iosurface_id) {
  for (auto& [connection, history] : replaced_) {
    (void)connection;
    for (auto id = history.begin(); id != history.end();) {
      id = *id == iosurface_id ? history.erase(id) : std::next(id);
    }
  }
}

std::shared_ptr<const OutputEpoch> OutputRegistry::Admit(uint32_t iosurface_id) const {
  std::lock_guard<std::mutex> lock(mutex_);
  const auto existing = by_surface_.find(iosurface_id);
  if (existing != by_surface_.end()) return existing->second;
  for (const auto& [connection, history] : replaced_) {
    for (const uint32_t id : history) {
      if (id != iosurface_id) continue;
      const auto owner = owners_.find(connection);
      return owner == owners_.end() ? nullptr : owner->second;
    }
  }
  return nullptr;
}

}  // namespace darwin_art::surfaceflinger
