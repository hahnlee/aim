#pragma once

#include <memory>

#include "receiver_registry.h"

namespace aim::input {
class InputTransport;

// A recipient pins only its transport, never the containing channel. Transport
// identity is the terminal/FIFO lane; receiver identity is a separate epoch.
// Routing may compare or retain this resource, but must not perform I/O.
struct InputRoutingEndpoint {
  const std::shared_ptr<InputTransport> transport;
  const ReceiverId endpoint_id = 0;
};
}  // namespace aim::input
