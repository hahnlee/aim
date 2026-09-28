#include "runtime/framework/input/input_routing.h"
#include "runtime/framework/input/input_transport.h"

#include <cassert>
#include <cstdio>
#include <utility>

using namespace aim;
using namespace aim::input;

namespace aim::input {
int InputTransport::RemoteEndpointFd() const { return -1; }
InputTransportStatus SendInputTransportPacket(
    InputTransport*, const aim::AimInputPacket&) {
  return InputTransportStatus::kTerminal;
}
}  // namespace aim::input

static AimInputPacket Key(uint64_t sequence) {
  AimInputPacket packet{};
  packet.kind = AimInputPacketKind::kKey;
  packet.key.version = 1;
  packet.key.size = sizeof(AimKeyEventV1);
  packet.key.sequence = sequence;
  return packet;
}

int main() {
  const auto routing = CreateInputRoutingState();
  const auto original = PrepareInputRoutingRecipient(routing, 41);
  assert(original && PublishInputRoutingRecipient(original).Published());
  assert(EnqueueInputRoutingPacket(routing, Key(1), 41));
  assert(EnqueueInputRoutingPacket(routing, Key(2), 41));

  InputRoutingPacketLease lease;
  assert(AcquireInputRoutingPacketLease(routing, original, &lease));
  assert(lease && lease.Recipient() == original && lease.Endpoint() == nullptr);
  assert(lease.Packet()->key.sequence == 1);
  InputRoutingPacketEnvelope envelope;
  assert(!DequeueInputRoutingPacketEnvelope(routing, &envelope, 41));
  assert(HasInputRoutingPackets(routing));

  // A deferred invocation rolls back the claim without consuming the head.
  lease = {};
  assert(AcquireInputRoutingPacketLease(routing, original, &lease));
  assert(lease.Packet()->key.sequence == 1);
  assert(lease.Complete(false));
  assert(AcquireInputRoutingPacketLease(routing, original, &lease));
  assert(lease.Packet()->key.sequence == 2);
  assert(lease.Complete());
  assert(!HasInputRoutingPackets(routing));

  // A replacement cannot make a stale numeric-ID token authoritative.
  assert(EnqueueInputRoutingPacket(routing, Key(3), 41));
  const auto replacement = PrepareInputRoutingRecipient(routing, 41);
  {
    InputRoutingPacketLease stale;
    assert(AcquireInputRoutingPacketLease(routing, original, &stale));
    // Legacy requeue cannot insert a duplicate ahead of the claimed head.
    assert(!RequeueInputRoutingPacketFront(routing, Key(99), 41));
    assert(replacement && PublishInputRoutingRecipient(replacement).Published());
    InputRoutingPacketLease duplicate;
    assert(!AcquireInputRoutingPacketLease(routing, replacement, &duplicate));
    // Scope exit settles the retired exact record as an implicit terminal
    // rejection, allowing successor traffic to proceed.
  }
  assert(EnqueueInputRoutingPacket(routing, Key(4), 41));
  assert(AcquireInputRoutingPacketLease(routing, replacement, &lease));
  assert(lease.Packet()->key.sequence == 4);
  assert(lease.Complete());
  std::puts("input routing packet lease: exact head/deferred rollback/replacement PASS");
}
