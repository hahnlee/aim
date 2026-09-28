#include "runtime/framework/input/input_routing.h"
#include "runtime/framework/input/input_transport.h"
#include "runtime/framework/input/routing_transport_dispatch.h"

#include <cassert>
#include <cstdio>
#include <cstdint>
#include <vector>
#include <functional>

using aim::AimInputEnqueueResult;
using ::AimPointerEventV2;
using aim::input::InputRoutingEndpoint;
using aim::input::InputRoutingHandle;
using aim::input::InputTransport;
using aim::input::InputTransportIo;
using aim::input::SubmitInputRoutingAdmission;

namespace {
std::vector<std::uint8_t> wire;
bool terminal_send = false;
bool block_send = false;
std::function<void()> during_send;

intptr_t Send(int, const void* bytes, size_t count, int) {
  if (during_send) {
    auto callback = std::move(during_send);
    during_send = {};
    callback();
  }
  if (block_send) return -1;
  if (terminal_send) return -1;
  const auto* begin = static_cast<const std::uint8_t*>(bytes);
  wire.insert(wire.end(), begin, begin + count);
  return static_cast<intptr_t>(count);
}
intptr_t Receive(int, void*, size_t, int) { return -1; }
int Close(int) { return 0; }
int Error() { return terminal_send ? 32 : 11; }

AimPointerEventV2 Pointer(std::uint64_t sequence) {
  AimPointerEventV2 pointer{};
  pointer.version = 2;
  pointer.size = sizeof(pointer);
  pointer.action = AIM_POINTER_DOWN;
  pointer.pointer_count = 1;
  pointer.x = pointer.y = pointer.raw_x = pointer.raw_y = 20;
  pointer.sequence = sequence;
  return pointer;
}

}  // namespace

// Unused default-I/O symbols remain link dependencies of input_transport.cc.
extern "C" int aim_bionic_socket_broker_socketpair(int, int, int,
                                                             int*) {
  return -1;
}
extern "C" intptr_t aim_bionic_socket_broker_send(int, const void*,
                                                           size_t, int) {
  return -1;
}
extern "C" intptr_t aim_bionic_socket_broker_recv(int, void*, size_t,
                                                           int) {
  return -1;
}
extern "C" int aim_bionic_socket_broker_close(int) { return 0; }
extern "C" int aim_bionic_errno_load() { return 11; }

int main() {
  const InputTransportIo io{Send, Receive, Close, Error};
  auto transport = std::make_shared<InputTransport>(io, false);
  aim::input::AdoptRemoteInputTransport(transport.get(), 77);
  const InputRoutingHandle routing =
      aim::input::CreateInputRoutingState();
  const auto endpoint = std::make_shared<const InputRoutingEndpoint>(
      InputRoutingEndpoint{transport, 501});
  assert(aim::input::SetInputRoutingConsumer(routing, 501, endpoint) ==
         0);
  assert(!aim::input::PublishInputRoutingWmsFrame(
      routing, 0, 0, 100, 100, true));
  aim::input::SetInputRoutingTransportReady(routing, 501, false);
  assert(aim::input::SetInputRoutingFocus(routing, 501));

  aim::input::InputRoutingAdmission admission;
  assert(aim::input::RouteFrameworkPointerPacket(Pointer(1),
                                                         &admission) ==
         AimInputEnqueueResult::kQueued);
  const auto submitted = SubmitInputRoutingAdmission(std::move(admission));
  assert(submitted.result == AimInputEnqueueResult::kQueued);
  assert(submitted.refresh_writable && !submitted.wake_local);
  assert(!wire.empty());
  const size_t frame_bytes = wire.size();
  // B reserves while A is admitted; A's settlement must execute B without
  // relying on a local receiver wake or a future writable/readable event.
  during_send = [&] {
    aim::input::InputRoutingAdmission successor;
    assert(aim::input::RouteFrameworkPointerPacket(Pointer(4), &successor) ==
           AimInputEnqueueResult::kQueued);
    const auto queued = SubmitInputRoutingAdmission(std::move(successor));
    assert(queued.result == AimInputEnqueueResult::kQueued);
  };
  assert(aim::input::RouteFrameworkPointerPacket(Pointer(3), &admission) ==
         AimInputEnqueueResult::kQueued);
  assert(SubmitInputRoutingAdmission(std::move(admission)).refresh_writable);
  assert(wire.size() == frame_bytes * 3);

  // Buffered acceptance across epoch retirement still requests writable
  // progress; continuation also admits the old stream's CANCEL to that wire.
  wire.clear();
  during_send = [&] {
    assert(aim::input::ClearInputRoutingFocus(routing, 501));
    block_send = true;
  };
  assert(aim::input::RouteFrameworkPointerPacket(Pointer(5), &admission) ==
         AimInputEnqueueResult::kQueued);
  const auto retired = SubmitInputRoutingAdmission(std::move(admission));
  assert(retired.refresh_writable && transport->HasPendingTx() && wire.empty());
  block_send = false;
  assert(aim::input::FlushInputTransport(transport.get()) ==
         aim::input::InputTransportStatus::kAccepted);
  assert(!transport->HasPendingTx() && wire.size() == frame_bytes * 2);
  assert(aim::input::SetInputRoutingFocus(routing, 501));

  // A terminal endpoint result is observable as no-focused, never as a
  // fabricated queued success, and the same submit owner handles it.
  terminal_send = true;
  assert(aim::input::RouteFrameworkPointerPacket(Pointer(2),
                                                         &admission) ==
         AimInputEnqueueResult::kQueued);
  const auto terminal = SubmitInputRoutingAdmission(std::move(admission));
  assert(terminal.result == AimInputEnqueueResult::kNoFocusedChannel);
  assert(!terminal.refresh_writable && !terminal.wake_local);
  assert(!aim::input::InputRoutingHasPending(routing));
  std::puts("routing-transport-dispatch: PASS fresh/retry owner/terminal");
  return 0;
}
