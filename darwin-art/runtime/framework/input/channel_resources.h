#pragma once

#include "channel_endpoint.h"
#include "channel_routing_continuation.h"

#include <memory>
#include <string>
#include <vector>

namespace darwin_art::input {

// Native InputChannel resource core. Java handles/Binder globals belong to
// the JNI identity owner; a receiver retains this core, not that identity.
class InputChannelResources final {
 public:
  InputChannelResources(std::string name, std::shared_ptr<ChannelEndpoint> endpoint);
  ~InputChannelResources();
  InputChannelResources(const InputChannelResources&) = delete;
  InputChannelResources& operator=(const InputChannelResources&) = delete;
  const std::string& Name() const { return name_; }
  std::shared_ptr<ChannelEndpoint> Endpoint() const { return endpoint_; }
  InputRoutingHandle Routing() const { return routing_; }
  bool BindContinuation(void* looper,
                        bool (*refresh_writable)(const InputRoutingHandle&));
  bool RequestContinuation();

 private:
  const std::string name_;
  const std::shared_ptr<ChannelEndpoint> endpoint_;
  const InputRoutingHandle routing_;
  ChannelRoutingContinuation continuation_;
};

// Weak resource registry, independent of Java channel/Binder identity. Snapshot
// leases are acquired under the registry lock; work/destruction happens outside.
void RegisterChannelResources(const std::shared_ptr<InputChannelResources>&);
std::vector<std::shared_ptr<InputChannelResources>> SnapshotChannelResources();
std::shared_ptr<InputChannelResources> FindChannelResourcesForRouting(
    const InputRoutingHandle&);

}  // namespace darwin_art::input
