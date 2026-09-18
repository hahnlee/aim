#pragma once

#include "channel_endpoint.h"
#include "routing_transport_scheduler.h"

#include <mutex>

namespace darwin_art::input {

// Couples channel transport progress to its Android owner Looper without JNI
// references or receiver ownership. A conflicting Looper is rejected explicitly.
class ChannelRoutingContinuation final {
 public:
  ~ChannelRoutingContinuation();
  bool Bind(void* looper, const std::shared_ptr<ChannelEndpoint>& endpoint,
            const InputRoutingHandle& routing,
            bool (*refresh_writable)(const InputRoutingHandle&));
  bool Request();
  void Retire();

 private:
  std::mutex mutex_;
  void* looper_ = nullptr;
  bool closed_ = false;
  RoutingTransportSchedulerHandle scheduler_;
};

}  // namespace darwin_art::input
