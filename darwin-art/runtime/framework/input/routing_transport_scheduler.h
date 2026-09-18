#pragma once

#include <cstddef>
#include <cstdint>
#include <memory>

#include "routing_transport_dispatch.h"
#include "../../../compat/looper/android_looper_owner.h"

namespace darwin_art::input {

struct RoutingTransportSchedulerCallbacks {
  RoutingTransportDrainResult (*on_progress)(void*) = nullptr;
  void (*on_failure)(void*) = nullptr;
  void* context = nullptr;
  std::shared_ptr<void> context_owner;
};

// Schedules bounded routing continuation work on the Android owner Looper.
// This owner has no local-wake or channel reference: a task pins only its
// independent state, and Retire closes that state before suppressing tasks.
class RoutingTransportScheduler {
 public:
  static std::shared_ptr<RoutingTransportScheduler> Create(
      void* looper, InputRoutingHandle routing,
      const RoutingTransportSchedulerCallbacks& callbacks);
  ~RoutingTransportScheduler();
  RoutingTransportScheduler(const RoutingTransportScheduler&) = delete;
  RoutingTransportScheduler& operator=(const RoutingTransportScheduler&) = delete;

  bool Request();
  bool Retire();

 public:
  struct State;

 private:
  explicit RoutingTransportScheduler(std::shared_ptr<State> state);
  std::shared_ptr<State> state_;
};

using RoutingTransportSchedulerHandle =
    std::shared_ptr<RoutingTransportScheduler>;

}  // namespace darwin_art::input
