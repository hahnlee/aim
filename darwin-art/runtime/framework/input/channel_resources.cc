#include "channel_resources.h"

#include <utility>
#include <algorithm>
#include <mutex>

namespace darwin_art::input {
namespace {
std::mutex g_resources_mutex;
std::vector<std::weak_ptr<InputChannelResources>> g_resources;
}

void RegisterChannelResources(const std::shared_ptr<InputChannelResources>& resource) {
  if (resource == nullptr) return;
  std::lock_guard<std::mutex> lock(g_resources_mutex);
  std::erase_if(g_resources, [](const auto& entry) { return entry.expired(); });
  const std::weak_ptr<InputChannelResources> identity = resource;
  // Compare control-block identity without pinning/destroying resources under
  // this mutex. Parcel imports can register an already known channel.
  if (std::any_of(g_resources.begin(), g_resources.end(), [&](const auto& entry) {
        return !entry.owner_before(identity) && !identity.owner_before(entry);
      })) return;
  g_resources.emplace_back(resource);
}

std::vector<std::shared_ptr<InputChannelResources>> SnapshotChannelResources() {
  std::vector<std::shared_ptr<InputChannelResources>> result;
  {
    std::lock_guard<std::mutex> lock(g_resources_mutex);
    // Allocate before pinning any resource. Subsequent pushes cannot allocate,
    // so OOM cannot release the final notifying resource under this mutex.
    result.reserve(g_resources.size());
    std::erase_if(g_resources, [](const auto& entry) { return entry.expired(); });
    for (const auto& entry : g_resources) {
      if (auto resource = entry.lock()) result.push_back(std::move(resource));
    }
  }
  return result;
}

std::shared_ptr<InputChannelResources> FindChannelResourcesForRouting(
    const InputRoutingHandle& routing) {
  if (routing == nullptr) return {};
  const auto resources = SnapshotChannelResources();
  for (const auto& resource : resources)
    if (resource->Routing() == routing) return resource;
  return {};
}

InputChannelResources::InputChannelResources(
    std::string name, std::shared_ptr<ChannelEndpoint> endpoint)
    : name_(std::move(name)), endpoint_(std::move(endpoint)),
      routing_(CreateInputRoutingState()) {}

InputChannelResources::~InputChannelResources() { continuation_.Retire(); }

bool InputChannelResources::BindContinuation(
    void* looper, bool (*refresh_writable)(const InputRoutingHandle&)) {
  return continuation_.Bind(looper, endpoint_, routing_, refresh_writable);
}

bool InputChannelResources::RequestContinuation() {
  return continuation_.Request();
}

}  // namespace darwin_art::input
