#include "channel_routing_continuation.h"

#include "routing_transport_dispatch.h"

#include <new>
#include <cstdio>
#include <utility>

namespace darwin_art::input {
namespace {
struct ContinuationPolicy {
  std::shared_ptr<ChannelEndpoint> endpoint;
  InputRoutingHandle routing;
  bool (*refresh_writable)(const InputRoutingHandle&);
};

RoutingTransportDrainResult Progress(void* context) {
  const auto& policy = *static_cast<ContinuationPolicy*>(context);
  const auto drained = DrainInputRoutingTransport(policy.routing);
  if (drained.remote_admitted) (void)policy.refresh_writable(policy.routing);
  if (drained.local_queued) (void)policy.endpoint->WakeLocal();
  return drained;
}
void Failure(void*) {
  std::fputs("ART InputChannel: deferred routing progress failed; retry required\n",
             stderr);
}
}  // namespace

ChannelRoutingContinuation::~ChannelRoutingContinuation() { Retire(); }

bool ChannelRoutingContinuation::Bind(
    void* looper, const std::shared_ptr<ChannelEndpoint>& endpoint,
    const InputRoutingHandle& routing,
    bool (*refresh_writable)(const InputRoutingHandle&)) {
  if (looper == nullptr || endpoint == nullptr || routing == nullptr ||
      refresh_writable == nullptr) return false;
  {
    std::lock_guard<std::mutex> lock(mutex_);
    if (closed_) return false;
    if (scheduler_ != nullptr) return looper_ == looper;
  }
  try {
    auto policy = std::make_shared<ContinuationPolicy>();
    policy->endpoint = endpoint;
    policy->routing = routing;
    policy->refresh_writable = refresh_writable;
    auto candidate = RoutingTransportScheduler::Create(
        looper, routing, {.on_progress = Progress, .on_failure = Failure,
                         .context = policy.get(), .context_owner = policy});
    if (candidate == nullptr) return false;
    {
      std::lock_guard<std::mutex> lock(mutex_);
      if (closed_) return false;
      if (scheduler_ != nullptr) return looper_ == looper;
      scheduler_ = std::move(candidate);
      looper_ = looper;
    }
    // A losing candidate retires outside the publication mutex.
    return true;
  } catch (const std::bad_alloc&) {
    return false;
  }
}

bool ChannelRoutingContinuation::Request() {
  RoutingTransportSchedulerHandle scheduler;
  {
    std::lock_guard<std::mutex> lock(mutex_);
    scheduler = scheduler_;
  }
  return scheduler != nullptr && scheduler->Request();
}

void ChannelRoutingContinuation::Retire() {
  RoutingTransportSchedulerHandle scheduler;
  {
    std::lock_guard<std::mutex> lock(mutex_);
    closed_ = true;
    scheduler = std::move(scheduler_);
  }
  if (scheduler != nullptr) (void)scheduler->Retire();
}
}  // namespace darwin_art::input
