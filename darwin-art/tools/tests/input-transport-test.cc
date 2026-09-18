#include "runtime/framework/input/input_transport.h"
#include "runtime/framework/input/input_transport_pump.h"
#include "runtime/framework/input/input_transport_readiness.h"
#include "runtime/framework/input/receiver_finish_owner.h"
#include "runtime/framework/input/input_routing.h"
#include "compat/looper/android_looper_owner.h"

#include <algorithm>
#include <cassert>
#include <cerrno>
#include <cstring>
#include <cstdint>
#include <utility>
#include <vector>
#include <limits>
#include <atomic>
#include <cstdlib>
#include <new>
#include <stdexcept>

// Test-only allocation injection, never linked into a product object.
static std::atomic<bool> fail_next_allocation{false};
void* operator new(std::size_t size) {
  if (fail_next_allocation.exchange(false)) throw std::bad_alloc();
  if (void* allocation = std::malloc(size == 0 ? 1 : size)) return allocation;
  throw std::bad_alloc();
}
void operator delete(void* allocation) noexcept { std::free(allocation); }

namespace {
std::vector<uint8_t> wire;
std::vector<std::vector<uint8_t>> receive_chunks;
size_t receive_index = 0;
size_t send_limit = 0;
bool block_send = false;
size_t send_budget = std::numeric_limits<size_t>::max();
int error_value = 11;
int close_count = 0;
bool throw_after_send = false;
darwin_art::input::ReceiverFinishOwner* finish_during_send = nullptr;
std::shared_ptr<const darwin_art::input::InputRoutingRecipient> finish_sink;
std::vector<int> send_fds;
darwin_art::looper::FdCallback registered_callback = nullptr;
void* registered_data = nullptr;
darwin_art::looper::OwnerRelease registered_release = nullptr;
int registered_events = 0;
darwin_art::input::InputTransport* terminate_tx_during_add = nullptr;

intptr_t Send(int fd, const void* bytes, size_t count, int) {
  if (auto* owner = std::exchange(finish_during_send, nullptr)) {
    using namespace darwin_art::input;
    assert(owner->Reserve(2, {ReceiverPacketOrigin::kImportedChannel,
                             (1ULL << 48) + 802}, finish_sink));
    bool handled = false;
    assert(!owner->CloseObservation(2, &handled));
    const auto progress = owner->Finish(2, false);
    assert(progress.coalesced && progress.retry_needed && progress.accepted == 0);
  }
  send_fds.push_back(fd);
  if (block_send || send_budget == 0) { errno = EAGAIN; return -1; }
  const size_t amount = std::min(send_budget,
      send_limit == 0 ? count : std::min(send_limit, count));
  send_budget -= amount;
  const auto* begin = static_cast<const uint8_t*>(bytes);
  wire.insert(wire.end(), begin, begin + amount);
  if (std::exchange(throw_after_send, false)) throw 71;
  return static_cast<intptr_t>(amount);
}
intptr_t Receive(int, void* bytes, size_t count, int) {
  if (receive_index == receive_chunks.size()) { errno = EAGAIN; return -1; }
  const auto& chunk = receive_chunks[receive_index++];
  if (chunk.empty()) return 0;
  const size_t amount = std::min(count, chunk.size());
  std::copy_n(chunk.data(), amount, static_cast<uint8_t*>(bytes));
  return static_cast<intptr_t>(amount);
}
int Close(int) { ++close_count; return 0; }
int Error() { return error_value; }

struct ProgressObserver {
  darwin_art::input::InputTransport* transport;
  std::vector<darwin_art::input::InputResourceProgressKind> events;
  bool check_rx_admission_released = false;
};
void OnProgress(void* context,
                darwin_art::input::InputResourceProgress event) noexcept {
  auto& observer = *static_cast<ProgressObserver*>(context);
  // These all reenter tx_mutex. Publication under that mutex would deadlock.
  (void)observer.transport->HasPendingTx();
  const auto prefix = observer.transport->CaptureAcceptedTxFence();
  (void)observer.transport->QueryTxFence(prefix);
  observer.events.push_back(event.kind);
  if (observer.check_rx_admission_released) {
    // This must run after the reader admission guard, not inside it.
    assert(darwin_art::input::PumpInputTransport(observer.transport, 42, {}) ==
           darwin_art::input::InputTransportStatus::kAccepted);
  }
}

void ThrowAck(void*, uint32_t, bool) { throw 71; }

bool OnPacket(void* context, const darwin_art::DarwinArtInputPacket&) {
  ++*static_cast<int*>(context);
  return true;
}

struct FocusObservation {
  std::vector<std::pair<uint64_t, bool>> values;
  darwin_art::input::FocusControlCallbackResult result =
      darwin_art::input::FocusControlCallbackResult::kConsumed;
  darwin_art::input::InputTransport* assert_live_during_callback = nullptr;
  darwin_art::input::InputTransportPumpLease* refresh_lease = nullptr;
  bool fail_tx_after_refresh = false;
};

struct ReentrantFocusObservation {
  darwin_art::input::InputTransport* transport;
  bool reenter = true;
  std::vector<uint64_t> epochs;
};
darwin_art::input::FocusControlCallbackResult OnReentrantFocus(
    void* context, uint64_t epoch, bool) noexcept {
  using namespace darwin_art::input;
  auto& observation = *static_cast<ReentrantFocusObservation*>(context);
  observation.epochs.push_back(epoch);
  if (std::exchange(observation.reenter, false)) {
    const InputTransportPumpCallbacks callbacks{
        .on_focus = OnReentrantFocus, .context = &observation};
    // Java focus delivery may reenter the original owner Looper. The nested
    // pump cannot redispatch the still-admitted head or erase later frames.
    assert(PumpInputTransport(observation.transport, 64, callbacks) ==
           InputTransportStatus::kBackpressured);
  }
  return FocusControlCallbackResult::kConsumed;
}

darwin_art::input::FocusControlCallbackResult OnFocus(
    void* context, uint64_t epoch, bool focused) noexcept {
  auto& observation = *static_cast<FocusObservation*>(context);
  if (observation.assert_live_during_callback != nullptr)
    assert(!observation.assert_live_during_callback->IsTerminal());
  if (observation.refresh_lease != nullptr) {
    using namespace darwin_art::input;
    assert(observation.refresh_lease->SetWritableResult(true) ==
           InputTransportWritableResult::kDeferred);
    if (observation.fail_tx_after_refresh) {
      error_value = 32;
      assert(FlushInputTransport(observation.assert_live_during_callback) ==
             InputTransportStatus::kTerminal);
    }
  }
  observation.values.emplace_back(epoch, focused);
  return observation.result;
}

darwin_art::DarwinArtInputPacket Packet(uint64_t sequence) {
  darwin_art::DarwinArtInputPacket packet;
  packet.kind = darwin_art::DarwinArtInputPacketKind::kPointer;
  packet.pointer.version = 2;
  packet.pointer.size = sizeof(DarwinArtPointerEventV2);
  packet.pointer.action = DARWIN_ART_POINTER_MOVE;
  packet.pointer.sequence = sequence;
  packet.pointer.pointer_count = 1;
  packet.pointer.x = 12.5f;
  packet.pointer.y = 7.25f;
  return packet;
}
}  // namespace

extern "C" int darwin_art_bionic_socket_broker_socketpair(int, int, int, int*) { return -1; }
extern "C" intptr_t darwin_art_bionic_socket_broker_send(int, const void*, size_t, int) { return -1; }
extern "C" intptr_t darwin_art_bionic_socket_broker_recv(int, void*, size_t, int) { return -1; }
extern "C" int darwin_art_bionic_socket_broker_close(int) { return 0; }
extern "C" int darwin_art_bionic_errno_load() { return error_value; }
namespace darwin_art::looper {
int AddFdOwned(void*, int, int, int events, FdCallback callback, void* data, void*,
               OwnerRelease release) {
  registered_events = events;
  registered_callback = callback;
  registered_data = data;
  registered_release = release;
  if (auto* transport = std::exchange(terminate_tx_during_add, nullptr))
    darwin_art::input::TerminateInputTransportTx(transport);
  return 1;
}
int RemoveFdIfOwned(void*, int, FdCallback callback, void* data) {
  if (callback != registered_callback || data != registered_data) return 0;
  registered_release(registered_data);
  registered_callback = nullptr;
  registered_data = nullptr;
  registered_release = nullptr;
  return 1;
}
}  // namespace darwin_art::looper

static void TestRetainedAckRetry() {
  using namespace darwin_art::input;
  const InputTransportIo io{Send, Receive, Close, Error};
  auto transport = std::make_shared<InputTransport>(io, false);
  assert(AdoptRemoteInputTransport(transport.get(), 51));
  auto recipient = std::make_shared<const InputRoutingRecipient>(
      InputRoutingHandle{}, 201,
      std::make_shared<const InputRoutingEndpoint>(InputRoutingEndpoint{transport, 201}));
  ReceiverFinishOwner owner;
  block_send = true;
  while (SendInputTransportPacket(transport.get(), Packet(1)) == InputTransportStatus::kAccepted) {}
  while (SendInputTransportAck64(transport.get(), 0, false) == InputTransportStatus::kAccepted) {}
  bool handled = false;
  for (uint32_t seq : {1u, 2u}) {
    assert(owner.Reserve(seq, {ReceiverPacketOrigin::kImportedChannel,
                              (1ULL << 48) + 800 + seq}, recipient));
    assert(!owner.CloseObservation(seq, &handled));
    const auto progress = owner.Finish(seq, seq == 1);
    assert(progress.accepted == 0 && progress.backpressured &&
           !progress.recovery_needed && progress.retry_needed);
  }
  assert(!owner.Finish(1, false));  // Duplicate finish cannot overwrite true.
  block_send = false;
  assert(FlushInputTransport(transport.get()) == InputTransportStatus::kAccepted);
  wire.clear(); send_fds.clear();
  const auto first = owner.RetryPending(1);
  assert(first.accepted == 1 && first.retry_needed && !first.backpressured);
  const auto second = owner.RetryPending(1);
  assert(second.accepted == 1 && !second.retry_needed);
  assert(owner.RetryPending().accepted == 0);
  assert(wire.size() == 2 * sizeof(transport_wire::AckFrameV2));
  transport_wire::AckFrameV2 ack;
  std::memcpy(&ack, wire.data(), sizeof(ack));
  assert(ack.sequence == (1ULL << 48) + 801 && ack.handled == 1);
  std::memcpy(&ack, wire.data() + sizeof(ack), sizeof(ack));
  assert(ack.sequence == (1ULL << 48) + 802 && ack.handled == 0);

  // Work recorded during submission must survive the progress admission.
  ReceiverFinishOwner reentrant;
  assert(reentrant.Reserve(1, {ReceiverPacketOrigin::kImportedChannel,
                              (1ULL << 48) + 801}, recipient));
  assert(!reentrant.CloseObservation(1, &handled));
  wire.clear(); finish_sink = recipient; finish_during_send = &reentrant;
  const auto nested = reentrant.Finish(1, true);
  assert(nested.accepted == 2 && !nested.retry_needed);
  assert(wire.size() == 2 * sizeof(ack));
  finish_sink.reset();

  // A post-enqueue exception can mean bytes already reached the peer. The
  // exact TX lane becomes terminal; a released claim must never resend them.
  ReceiverFinishOwner uncertain;
  assert(uncertain.Reserve(1, {ReceiverPacketOrigin::kImportedChannel, 901}, recipient));
  assert(!uncertain.CloseObservation(1, &handled));
  wire.clear(); throw_after_send = true;
  bool threw = false;
  try { (void)uncertain.Finish(1, true); }
  catch (int error) { assert(error == 71); threw = true; }
  assert(threw && transport->IsTxTerminal() && uncertain.HasPendingAck());
  const auto failed = uncertain.RetryPending();
  assert(failed.terminal == 1 && failed.accepted == 0 && !failed.retry_needed);
  assert(FlushInputTransport(transport.get()) == InputTransportStatus::kTerminal);
  assert(wire.size() == sizeof(ack));
  assert(uncertain.Reserve(1, {ReceiverPacketOrigin::kLocalQueue, 902}, recipient));
  assert(!uncertain.CloseObservation(1, &handled));
  assert(uncertain.Finish(1, true));  // Dead imported sink cannot starve local finish.

  // Empty-TX allocation rejection has no OUTPUT wake. Report recovery rather
  // than pretending writable readiness will arrive or spinning a local FD.
  auto empty = std::make_shared<InputTransport>(io, false);
  assert(AdoptRemoteInputTransport(empty.get(), 52));
  auto empty_sink = std::make_shared<const InputRoutingRecipient>(
      InputRoutingHandle{}, 202,
      std::make_shared<const InputRoutingEndpoint>(InputRoutingEndpoint{empty, 202}));
  ReceiverFinishOwner recovery;
  assert(recovery.Reserve(1, {ReceiverPacketOrigin::kImportedChannel, 903}, empty_sink));
  assert(!recovery.CloseObservation(1, &handled));
  fail_next_allocation.store(true);
  const auto rejected = recovery.Finish(1, false);
  assert(rejected.backpressured && rejected.recovery_needed && rejected.retry_needed);
  assert(!empty->HasPendingTx() && !empty->IsTxTerminal());
  wire.clear();
  assert(recovery.RetryPending().accepted == 1 && !recovery.HasPendingAck());
  assert(wire.size() == sizeof(ack));
  wire.clear(); send_fds.clear();
  std::puts("receiver ACK retry: capacity/budget/reentry/uncertain TX/empty recovery PASS");
}

static void TestReadinessProgressOrdering() {
  using namespace darwin_art::input;
  InputTransportIo io{Send, Receive, Close, Error};
  auto transport = std::make_shared<InputTransport>(io, false);
  assert(AdoptRemoteInputTransport(transport.get(), 62));
  InputTransportPumpLease lease;
  struct State {
    InputTransport* transport;
    InputTransportPumpLease* lease;
    int progress = 0;
    int terminal = 0;
  } state{transport.get(), &lease};
  InputTransportPumpCallbacks callbacks{
      .on_progress = [](void* p, InputTransportStatus status) {
        auto& state = *static_cast<State*>(p);
        if (state.progress++ != 0) return;
        assert(status == InputTransportStatus::kTerminal && state.transport->IsRxTerminal());
        block_send = true;
        assert(SendInputTransportAck64(state.transport, 9001, true) ==
               InputTransportStatus::kAccepted);
        // This request must observe committed ReaderComplete: queued bytes
        // still require OUTPUT even though policy asks to disable it.
        assert(state.lease->SetWritableResult(false) == InputTransportWritableResult::kDeferred);
      },
      .context = &state,
      .on_terminal = [](void* p) noexcept { ++static_cast<State*>(p)->terminal; }};
  receive_chunks = {{}}; receive_index = 0;
  assert(lease.Register(reinterpret_cast<void*>(1), transport, 62, 1, callbacks));
  assert(registered_callback(62, 1, registered_data) == 1);
  assert(state.progress == 1 && state.terminal == 0 && !lease.IsQuiescent() &&
         transport->HasPendingTx() && (registered_events & 1) == 0 &&
         (registered_events & 2) != 0);
  block_send = false;
  assert(registered_callback(62, 2, registered_data) == 0);
  assert(state.progress == 2 && state.terminal == 1 && lease.IsQuiescent() &&
         !transport->HasPendingTx() && receive_index == 1 &&
         wire.size() == sizeof(transport_wire::AckFrameV2));
  receive_chunks.clear(); receive_index = 0; wire.clear(); send_fds.clear();
  std::puts("readiness lifecycle ordering: commit -> progress enqueue -> fresh TX drain PASS");
}

static void TestReadinessService() {
  using namespace darwin_art::input;
  InputTransportIo io{Send, Receive, Close, Error};
  auto transport = std::make_shared<InputTransport>(io, false);
  assert(AdoptRemoteInputTransport(transport.get(), 61));
  struct State { int reader = 0, legacy = 0, progress = 0; } state;
  InputTransportReadiness snapshot;
  snapshot.transport = transport;
  snapshot.callbacks.context = &state;
  snapshot.callbacks.on_progress = [](void* p, InputTransportStatus) {
    ++static_cast<State*>(p)->progress;
  };
  receive_chunks = {{}}; receive_index = 0;
  // Read authority is admitted by the lifecycle owner, not the readiness mask.
  auto result = ServiceInputTransportReadiness(snapshot, 61, 1, false, false);
  assert(result.default_progress.has_value() && !result.reader_complete_requested &&
         !result.retire && receive_index == 0 && state.progress == 0);
  result = ServiceInputTransportReadiness(snapshot, 61, 1, true, false);
  assert(result.reader_complete_requested && result.default_progress.has_value() &&
         *result.default_progress == InputTransportStatus::kTerminal &&
         receive_index == 1 && state.progress == 0);
  // Completion request and progress invocation remain separate from commit.
  snapshot.reader_context = &state;
  snapshot.reader_callback = [](int, int, void* p) {
    ++static_cast<State*>(p)->reader;
    return InputTransportReaderResult::kReaderComplete;
  };
  result = ServiceInputTransportReadiness(snapshot, 61, 1, false, false);
  assert(result.retire && state.reader == 0 && !result.default_progress);
  auto owner = std::make_shared<int>(9);
  std::weak_ptr<int> weak_owner = owner;
  snapshot.reader_owner = owner;
  owner.reset();
  result = ServiceInputTransportReadiness(snapshot, 61, 1, true, false);
  assert(result.reader_complete_requested && !result.default_progress && state.reader == 1);
  result = ServiceInputTransportReadiness(snapshot, 61, 8, false, true);
  assert(!result.reader_complete_requested && !result.default_progress &&
         !result.retire && state.reader == 1 && !transport->IsTxTerminal() &&
         !weak_owner.expired());
  snapshot.reader_callback = [](int, int, void*) -> InputTransportReaderResult {
    throw 71;
  };
  bool threw = false;
  try { (void)ServiceInputTransportReadiness(snapshot, 61, 1, true, false); }
  catch (int value) { threw = value == 71; }
  assert(threw);
  snapshot.reader_callback = [](int, int, void*) {
    return static_cast<InputTransportReaderResult>(255);
  };
  result = ServiceInputTransportReadiness(snapshot, 61, 1, true, false);
  assert(result.retire && !result.reader_complete_requested && !result.default_progress);
  snapshot.reader_callback = nullptr;
  snapshot.callback_context = &state;
  snapshot.callback = [](int, int, void* p) {
    ++static_cast<State*>(p)->legacy;
    return 0;
  };
  result = ServiceInputTransportReadiness(snapshot, 61, 1, true, false);
  assert(result.retire && !result.default_progress && state.legacy == 1);
  TerminateInputTransportTx(transport.get());
  result = ServiceInputTransportReadiness(snapshot, 61, 1, true, false);
  assert(result.retire && state.legacy == 2);  // Failed TX preserves reader authority.
  result = ServiceInputTransportReadiness(snapshot, 61, 2, false, false);
  assert(result.retire && state.legacy == 2);  // No OUTPUT-only legacy invocation.
  snapshot = {};
  assert(weak_owner.expired());
  receive_chunks.clear(); receive_index = 0; wire.clear(); send_fds.clear();
  std::puts("admitted readiness service: modes/authority/completion/pins/throw/legacy PASS");
}

static void TestOriginalFinishIdentity() {
  using namespace darwin_art::input;
  const InputTransportIo io{Send, Receive, Close, Error};
  auto original_transport = std::make_shared<InputTransport>(io, false);
  auto successor_transport = std::make_shared<InputTransport>(io, false);
  assert(AdoptRemoteInputTransport(original_transport.get(), 41));
  assert(AdoptRemoteInputTransport(successor_transport.get(), 42));
  auto original = std::make_shared<const InputRoutingRecipient>(
      InputRoutingHandle{}, 100,
      std::make_shared<const InputRoutingEndpoint>(
          InputRoutingEndpoint{original_transport, 100}));
  auto successor = std::make_shared<const InputRoutingRecipient>(
      InputRoutingHandle{}, 101,
      std::make_shared<const InputRoutingEndpoint>(
          InputRoutingEndpoint{successor_transport, 101}));
  ReceiverFinishOwner old_receiver, new_receiver;
  constexpr uint64_t packet_sequence = (1ULL << 48) + 701;
  assert(old_receiver.Reserve(1, {ReceiverPacketOrigin::kImportedChannel,
                                 packet_sequence}, original));
  assert(new_receiver.Reserve(1, {ReceiverPacketOrigin::kImportedChannel, 702},
                              successor));
  assert(old_receiver.HasOutstandingRemote() && !old_receiver.HasPendingAck());
  const auto sealed = old_receiver.SealRemoteAdmission();
  assert(sealed.sealed && sealed.outstanding_remote == 1);
  uint32_t sealed_next = 15, sealed_output = 88;
  assert(!old_receiver.ReserveNext(&sealed_next, &sealed_output,
      {ReceiverPacketOrigin::kImportedChannel, 703}, original));
  assert(sealed_next == 15 && sealed_output == 88);
  std::weak_ptr<const InputRoutingRecipient> retained = original;
  original.reset();
  assert(!retained.expired());
  bool handled = false;
  assert(!old_receiver.CloseObservation(1, &handled));
  wire.clear(); send_fds.clear();
  assert(old_receiver.Finish(1, true));
  assert(old_receiver.RemoteState().sealed && !old_receiver.HasOutstandingRemote());
  assert(retained.expired());
  assert(send_fds == std::vector<int>{41});
  assert(!old_receiver.Finish(1, false));
  assert(new_receiver.Finish(1, false));
  assert(!new_receiver.HasOutstandingRemote());  // Observation still open.
  assert(!new_receiver.Reserve(1, {ReceiverPacketOrigin::kLocalQueue, 0}, successor));
  assert(new_receiver.CloseObservation(1, &handled) && !handled);
  assert(wire.size() == 2 * sizeof(transport_wire::AckFrameV2));
  transport_wire::AckFrameV2 frame;
  std::memcpy(&frame, wire.data(), sizeof(frame));
  assert(frame.version == 2 && frame.sequence == packet_sequence && frame.handled == 1);

  // Actual V1 and V2 decoder ports stay distinct. A partial header must wait.
  assert(SendInputTransportAck(original_transport.get(), 9, true) ==
         InputTransportStatus::kAccepted);
  InputTransport rx(io, false);
  assert(AdoptRemoteInputTransport(&rx, 43));
  struct Observation {
    std::vector<uint64_t> originals;
    int legacy = 0;
  } observation;
  const InputTransportPumpCallbacks callbacks{
      .on_ack = [](void* context, uint32_t seq, bool value) {
        auto& state = *static_cast<Observation*>(context);
        assert(seq == 9 && value); ++state.legacy;
      },
      .context = &observation,
      .on_ack64 = [](void* context, uint64_t seq, bool value) {
        auto& state = *static_cast<Observation*>(context);
        assert(value == (seq == packet_sequence));
        state.originals.push_back(seq);
      }};
  const auto full_wire = wire;
  receive_chunks = {{full_wire.begin(), full_wire.begin() + 6}};
  receive_index = 0;
  assert(PumpInputTransport(&rx, 43, callbacks) == InputTransportStatus::kAccepted);
  assert(observation.originals.empty());
  receive_chunks = {{full_wire.begin() + 6, full_wire.end()}};
  receive_index = 0;
  assert(PumpInputTransport(&rx, 43, callbacks) == InputTransportStatus::kAccepted);
  assert((observation.originals == std::vector<uint64_t>{packet_sequence, 702}));
  assert(observation.legacy == 1);

  // Local completion owes no wire ACK, even with an imported FD present.
  wire.clear(); send_fds.clear();
  for (uint32_t seq = 0; seq < 300; ++seq) {
    assert(old_receiver.Reserve(seq, {ReceiverPacketOrigin::kLocalQueue, seq}, successor));
    assert(!old_receiver.CloseObservation(seq, &handled));
    assert(old_receiver.Finish(seq, true));
  }
  assert(wire.empty() && send_fds.empty());

  // The real receiver owner allocates around live IDs, including wrapped zero.
  ReceiverFinishOwner allocator;
  const InputEventOrigin local_origin{ReceiverPacketOrigin::kLocalQueue, 1};
  assert(allocator.Reserve(UINT32_MAX, local_origin, successor));
  assert(allocator.Reserve(0, local_origin, successor));
  uint32_t next = UINT32_MAX, selected = 55;
  assert(allocator.ReserveNext(&next, &selected, local_origin, successor));
  assert(selected == 1 && next == 2);
  // Accepted ACK with an open Java observation still owns the identity.
  assert(allocator.Finish(0, true));
  next = 0;
  assert(allocator.ReserveNext(&next, &selected, local_origin, successor));
  assert(selected == 2 && next == 3);
  assert(allocator.CloseObservation(0, &handled) && handled);
  next = 0;
  assert(allocator.ReserveNext(&next, &selected, local_origin, successor));
  assert(selected == 0 && next == 1);
  next = 40; selected = 77;
  assert(!allocator.ReserveNext(&next, &selected, local_origin, {}));
  assert(next == 40 && selected == 77);
  assert(wire.empty() && send_fds.empty());

  // Invalid reserved/boolean/version fields fail without publishing an ACK.
  for (int invalid = 0; invalid < 3; ++invalid) {
    InputTransport bad_rx(io, false);
    assert(AdoptRemoteInputTransport(&bad_rx, 44));
    transport_wire::AckFrameV2 bad;
    if (invalid == 0) bad.reserved = 1;
    if (invalid == 1) bad.handled = 2;
    if (invalid == 2) bad.version = 3;
    const auto* bytes = reinterpret_cast<const uint8_t*>(&bad);
    receive_chunks = {{bytes, bytes + sizeof(bad)}};
    receive_index = 0;
    assert(PumpInputTransport(&bad_rx, 44, callbacks) == InputTransportStatus::kTerminal);
    assert(observation.originals.size() == 2 && observation.legacy == 1);
  }
  receive_chunks.clear(); receive_index = 0; wire.clear(); send_fds.clear();
  std::puts("receiver finish: original uint64/sink/epoch/local-no-ACK/V1-V2 separation PASS");
}

int main() {
  TestRetainedAckRetry();
  TestOriginalFinishIdentity();
  TestReadinessService();
  TestReadinessProgressOrdering();
  const darwin_art::input::InputTransportIo io{Send, Receive, Close, Error};
  {
    using namespace darwin_art::input;
    // Real framed FIFO, including EOF: no deferred or budget-stopped item
    // may permit a later control to overtake its consumer.
    InputTransport tx(io, false), rx(io, false);
    assert(AdoptRemoteInputTransport(&tx, 42));
    assert(AdoptRemoteInputTransport(&rx, 42));
    wire.clear();
    assert(SendInputTransportPacket(&tx, Packet(71)) == InputTransportStatus::kAccepted);
    assert(SendInputTransportWindow(&tx, 0, 0, 360, 640, true) == InputTransportStatus::kAccepted);
    assert(SendInputTransportFocus(&tx, 91, true) == InputTransportStatus::kAccepted);
    receive_chunks = {wire, {}};
    receive_index = 0;
    struct Consumption {
      InputTransportConsumptionResult packet = InputTransportConsumptionResult::kDeferred;
      InputTransportConsumptionResult window = InputTransportConsumptionResult::kDeferred;
      int packet_calls = 0, window_calls = 0, focus_calls = 0;
    } state;
    InputTransportPumpCallbacks callbacks{
      .on_focus = [](void* context, uint64_t, bool) noexcept {
        ++static_cast<Consumption*>(context)->focus_calls;
        return FocusControlCallbackResult::kConsumed;
      },
      .context = &state,
      .on_packet_consumption = [](void* context, const darwin_art::DarwinArtInputPacket&) {
        auto& state = *static_cast<Consumption*>(context);
        ++state.packet_calls;
        return state.packet;
      },
      .on_window_consumption = [](void* context, int32_t, int32_t, int32_t, int32_t, bool) {
        auto& state = *static_cast<Consumption*>(context);
        ++state.window_calls;
        return state.window;
      },
    };
    auto conflicting = callbacks;
    conflicting.on_packet = OnPacket;
    InputTransportPumpLease conflicting_lease;
    auto rejected_transport = std::make_shared<InputTransport>(io, false);
    assert(!conflicting_lease.Register(reinterpret_cast<void*>(1), rejected_transport,
                                      42, 1, conflicting));
    bool conflict_rejected = false;
    try { (void)PumpInputTransport(&rx, 42, conflicting); }
    catch (const std::invalid_argument&) { conflict_rejected = true; }
    assert(conflict_rejected && receive_index == 0 && state.packet_calls == 0);
    assert(PumpInputTransport(&rx, 42, callbacks) == InputTransportStatus::kBackpressured);
    assert(state.packet_calls == 1 && state.window_calls == 0 && state.focus_calls == 0);
    state.packet = static_cast<InputTransportConsumptionResult>(255);
    bool invalid_rejected = false;
    try { (void)PumpInputTransport(&rx, 42, callbacks); }
    catch (const std::invalid_argument&) { invalid_rejected = true; }
    assert(invalid_rejected && state.window_calls == 0);
    state.packet = InputTransportConsumptionResult::kConsumedStop;
    assert(PumpInputTransport(&rx, 42, callbacks) == InputTransportStatus::kAccepted);
    assert(state.packet_calls == 3 && state.window_calls == 0 && !rx.IsTerminal());
    assert(PumpInputTransport(&rx, 42, callbacks) == InputTransportStatus::kBackpressured);
    assert(state.window_calls == 1 && state.focus_calls == 0);
    state.window = InputTransportConsumptionResult::kConsumedStop;
    assert(PumpInputTransport(&rx, 42, callbacks) == InputTransportStatus::kAccepted);
    assert(state.window_calls == 2 && state.focus_calls == 0 && !rx.IsTerminal());
    assert(PumpInputTransport(&rx, 42, callbacks) == InputTransportStatus::kTerminal);
    assert(state.focus_calls == 1);
    assert(PumpInputTransport(&rx, 42, callbacks) == InputTransportStatus::kTerminal);
    assert(state.focus_calls == 1);
    wire.clear();
  }
  {
    using namespace darwin_art::input;
    InputTransport transport(io, false);
    assert(AdoptRemoteInputTransport(&transport, 42));
    auto observer = std::make_shared<ProgressObserver>();
    observer->transport = &transport;
    auto subscription = transport.SubscribeProgress(OnProgress, observer);
    const auto frame = transport_wire::EncodeFocusControl({71, true});
    const auto* bytes = reinterpret_cast<const uint8_t*>(&frame);
    receive_chunks = {std::vector<uint8_t>(bytes, bytes + sizeof(frame))};
    receive_index = 0;
    FocusObservation focus;
    focus.result = FocusControlCallbackResult::kDeferred;
    const InputTransportPumpCallbacks callbacks{
        .on_focus = OnFocus, .context = &focus};
    assert(PumpInputTransport(&transport, 42, callbacks) ==
           InputTransportStatus::kBackpressured);
    assert(observer->events == std::vector<InputResourceProgressKind>{
        InputResourceProgressKind::kRxBuffered});
    assert(PumpInputTransport(&transport, 42, callbacks) ==
           InputTransportStatus::kBackpressured);
    assert(observer->events.size() == 1);  // Deferred retry is not progress.
    focus.result = FocusControlCallbackResult::kConsumed;
    observer->check_rx_admission_released = true;
    assert(PumpInputTransport(&transport, 42, callbacks) ==
           InputTransportStatus::kAccepted);
    assert(observer->events.size() == 2 && observer->events.back() ==
           InputResourceProgressKind::kRxConsumed);
    assert(PumpInputTransport(&transport, 42, callbacks) ==
           InputTransportStatus::kAccepted);
    assert(observer->events.size() == 2);  // Empty drain cannot self-wake.
    receive_chunks.clear();
    receive_index = 0;
  }
  {
    using namespace darwin_art::input;
    InputTransport transport(io, false);
    assert(AdoptRemoteInputTransport(&transport, 42));
    auto observer = std::make_shared<ProgressObserver>();
    observer->transport = &transport;
    observer->check_rx_admission_released = true;
    auto subscription = transport.SubscribeProgress(OnProgress, observer);
    assert(SendInputTransportAck(&transport, 71, true) ==
           InputTransportStatus::kAccepted);
    observer->events.clear();
    receive_chunks = {wire};
    wire.clear();
    receive_index = 0;
    bool threw = false;
    try {
      (void)PumpInputTransport(&transport, 42, {.on_ack = ThrowAck});
    } catch (int value) {
      threw = value == 71;
    }
    assert(threw);
    assert((observer->events == std::vector<InputResourceProgressKind>{
        InputResourceProgressKind::kRxBuffered,
        InputResourceProgressKind::kRxConsumed}));
    receive_chunks.clear();
    receive_index = 0;
  }
  using FenceStatus = darwin_art::input::InputTransportTxFenceStatus;
  {
    using namespace darwin_art::input;
    auto transport = std::make_shared<InputTransport>(io, false);
    assert(AdoptRemoteInputTransport(transport.get(), 42));
    InputTransportPumpLease invalid;
    assert(!invalid.RegisterReader(reinterpret_cast<void*>(1), transport, 42,
                                   1, {}, nullptr, nullptr));
    assert(!invalid.RegisterReader(reinterpret_cast<void*>(1), transport, 42,
        2, {}, [](int, int, void*) {
          return InputTransportReaderResult::kKeepReading;
        }, nullptr));
    assert(registered_callback == nullptr);  // Reject before provider admission.
  }
  for (int fatal : {0x0004, 0x0010}) {
    using namespace darwin_art::input;
    auto transport = std::make_shared<InputTransport>(io, false);
    assert(AdoptRemoteInputTransport(transport.get(), 42));
    block_send = true;
    assert(SendInputTransportAck(transport.get(), 72, true) == InputTransportStatus::kAccepted);
    const auto fence = transport->CaptureAcceptedTxFence();
    bool destroyed = false;
    struct ReaderOwner {
      InputTransport* transport;
      bool* destroyed;
      ReaderOwner(InputTransport* t, bool* d) : transport(t), destroyed(d) {}
      ~ReaderOwner() { *destroyed = true; }
    };
    auto owner = std::make_shared<ReaderOwner>(transport.get(), &destroyed);
    InputTransportPumpLease lease;
    int quiescent = 0;
    assert(lease.RegisterReader(reinterpret_cast<void*>(1), transport, 42, 1,
        {.context = &quiescent, .on_quiescent = [](void* p) noexcept {
          ++*static_cast<int*>(p);
        }}, [](int fd, int, void* p) {
          auto& owner = *static_cast<ReaderOwner*>(p);
          assert(!*owner.destroyed);
          assert(PumpInputTransport(owner.transport, fd, {}) == InputTransportStatus::kTerminal);
          return InputTransportReaderResult::kReaderComplete;
        }, owner.get(), owner));
    owner.reset();  // Only actual pump/registration ownership remains.
    assert(!destroyed);
    receive_chunks = {{}};
    receive_index = 0;
    assert(registered_callback(42, 1 | fatal, registered_data) == 0);
    assert(lease.IsQuiescent() && quiescent == 1 && destroyed);
    assert(transport->IsRxTerminal() && transport->IsTxTerminal());
    assert(transport->QueryTxFence(fence) == FenceStatus::kTerminal);
    assert(transport->HasPendingTx());  // Fatal provider did not fake delivery.
    assert(registered_callback == nullptr);
    receive_chunks.clear();
    receive_index = 0;
    block_send = false;
    wire.clear();
  }
  for (const bool initial_add : {false, true}) {
    using namespace darwin_art::input;
    auto transport = std::make_shared<InputTransport>(io, false);
    assert(AdoptRemoteInputTransport(transport.get(), 68));
    InputTransportPumpLease lease;
    if (initial_add) terminate_tx_during_add = transport.get();
    assert(lease.Register(reinterpret_cast<void*>(1), transport, 68,
                          initial_add ? 3 : 1, {}));
    if (!initial_add) {
      terminate_tx_during_add = transport.get();
      assert(lease.SetWritableResult(true) == InputTransportWritableResult::kApplied);
    }
    // Provider Add itself reenters resource termination after an old mask
    // was constructed; post-Add normalization must do another exact rearm.
    assert(terminate_tx_during_add == nullptr && registered_events == 1);
    assert(transport->IsTxTerminal() && !transport->IsRxTerminal());
    assert(lease.Retire() && lease.IsQuiescent());
  }
  for (const bool fail_tx : {false, true}) {
    using namespace darwin_art::input;
    auto transport = std::make_shared<InputTransport>(io, false);
    assert(AdoptRemoteInputTransport(transport.get(), 67));
    block_send = true;
    error_value = 11;
    assert(SendInputTransportAck(transport.get(), 1, true) == InputTransportStatus::kAccepted);
    InputTransportPumpLease lease;
    FocusObservation observation;
    observation.assert_live_during_callback = transport.get();
    observation.refresh_lease = &lease;
    observation.fail_tx_after_refresh = fail_tx;
    const auto focus = transport_wire::EncodeFocusControl({51, false});
    const auto* bytes = reinterpret_cast<const uint8_t*>(&focus);
    receive_chunks = {std::vector<uint8_t>(bytes, bytes + sizeof(focus))};
    if (!fail_tx) receive_chunks.emplace_back();
    receive_index = 0;
    assert(lease.Register(reinterpret_cast<void*>(1), transport, 67, 1,
                          {.on_focus = OnFocus, .context = &observation}));
    assert(registered_callback(67, 1, registered_data) == 1);
    assert(observation.values.size() == 1);
    // The callback's queued INPUT|OUTPUT request predates RX EOF or TX EPIPE.
    // Apply it only after normalizing against the now-terminal direction.
    assert(registered_events == (fail_tx ? 1 : 2));
    assert(transport->IsTxTerminal() == fail_tx);
    assert(transport->IsRxTerminal() != fail_tx);
    error_value = 11;
    assert(lease.Retire());
    block_send = false;
    receive_chunks.clear();
    receive_index = 0;
  }
  {
    using namespace darwin_art::input;
    InputTransport publication(io, false);
    const auto empty = publication.CaptureAcceptedTxFence();
    fail_next_allocation = true;
    assert(SendInputTransportFocusOnFd(&publication, 76, 899, true) == InputTransportStatus::kBackpressured);
    assert(!fail_next_allocation && publication.OutputSnapshot().endpoint_fd == -1);
    assert(!publication.HasPendingTx() && publication.QueryTxFence(empty) == FenceStatus::kFlushed);
    assert(publication.BindOutputEndpoint(75));
    assert(!publication.BindOutputEndpoint(76));
    assert(AdoptRemoteInputTransport(&publication, 42));
    assert(publication.OutputSnapshot().endpoint_fd == 75);
  }
  {
    using namespace darwin_art::input;
    // WMS publications use the server side of a local channel, whereas an
    // imported receiver's ACKs use RemoteEndpointFd. Accepted bytes must keep
    // their original endpoint through partial writes and writable retries.
    InputTransport publication(io, false);
    assert(AdoptRemoteInputTransport(&publication, 42));
    wire.clear();
    send_fds.clear();
    block_send = true;
    assert(SendInputTransportFocusOnFd(&publication, 77, 900, true) ==
           InputTransportStatus::kAccepted);
    const auto prefix = publication.CaptureAcceptedTxFence();
    assert(publication.QueryTxFence(prefix) == FenceStatus::kPending);
    const size_t attempts = send_fds.size();
    assert(SendInputTransportFocusOnFd(&publication, 42, 901, false) ==
           InputTransportStatus::kTerminal);
    assert(send_fds.size() == attempts && !publication.IsTerminal());
    block_send = false;
    send_budget = 3;
    assert(FlushInputTransport(&publication) == InputTransportStatus::kBackpressured);
    send_budget = std::numeric_limits<size_t>::max();
    assert(FlushInputTransport(&publication) == InputTransportStatus::kAccepted);
    assert(publication.QueryTxFence(prefix) == FenceStatus::kFlushed);
    const auto expected = transport_wire::EncodeFocusControl({900, true});
    assert(wire.size() == sizeof(expected));
    assert(std::memcmp(wire.data(), &expected, sizeof(expected)) == 0);
    assert(SendInputTransportFocusOnFd(&publication, 42, 902, false) ==
           InputTransportStatus::kTerminal);
    assert(!publication.IsTerminal() && publication.QueryTxFence(prefix) == FenceStatus::kFlushed);
    assert(!send_fds.empty());
    assert(std::all_of(send_fds.begin(), send_fds.end(),
                       [](int fd) { return fd == 77; }));
    wire.clear();
    send_fds.clear();
  }
  {
    using namespace darwin_art::input;
    InputTransport publication(io, false);
    wire.clear();
    send_fds.clear();
    block_send = true;
    assert(SendInputTransportFocusOnFd(&publication, 78, 903, false) == InputTransportStatus::kAccepted);
    assert(publication.RemoteEndpointFd() == -1);
    block_send = false;
    assert(FlushInputTransport(&publication) == InputTransportStatus::kAccepted);
    assert(std::all_of(send_fds.begin(), send_fds.end(), [](int fd) { return fd == 78; }));
    wire.clear();
    send_fds.clear();
  }
  {
    using namespace darwin_art::input;
    InputTransport publication(io, false);
    AdoptRemoteInputTransport(&publication, 42);
    wire.clear();
    send_fds.clear();
    block_send = true;
    assert(SendInputTransportAck(&publication, 901, true) == InputTransportStatus::kAccepted);
    const auto prefix = publication.CaptureAcceptedTxFence();
    // Replacing the RX endpoint must not redirect an already accepted prefix.
    assert(!AdoptRemoteInputTransport(&publication, 43));
    assert(AdoptRemoteInputTransport(&publication, 42));
    assert(publication.RemoteEndpointFd() == 42);
    block_send = false;
    assert(FlushInputTransport(&publication) == InputTransportStatus::kAccepted);
    assert(publication.QueryTxFence(prefix) == FenceStatus::kFlushed);
    assert(std::all_of(send_fds.begin(), send_fds.end(),
                       [](int fd) { return fd == 42; }));
    wire.clear();
    send_fds.clear();
  }
  {
    using Kind = darwin_art::input::InputResourceProgressKind;
    using Status = darwin_art::input::InputTransportStatus;
    darwin_art::input::InputTransport monitored(io, false);
    darwin_art::input::AdoptRemoteInputTransport(&monitored, 42);
    auto observer = std::make_shared<ProgressObserver>();
    observer->transport = &monitored;
    auto subscription = monitored.SubscribeProgress(OnProgress, observer);
    assert(subscription);
    block_send = true;
    assert(darwin_art::input::SendInputTransportAck(&monitored, 1, true) ==
           Status::kAccepted);
    assert(observer->events == std::vector<Kind>{Kind::kTxAccepted});
    assert(darwin_art::input::FlushInputTransport(&monitored) ==
           Status::kBackpressured);
    assert(observer->events.size() == 1);  // EAGAIN is not progress.
    block_send = false;
    send_budget = 8;
    assert(darwin_art::input::FlushInputTransport(&monitored) ==
           Status::kBackpressured);
    assert(observer->events.back() == Kind::kTxAdvanced);
    send_budget = std::numeric_limits<size_t>::max();
    assert(darwin_art::input::FlushInputTransport(&monitored) == Status::kAccepted);
    assert(observer->events.size() == 3);
    assert(darwin_art::input::FlushInputTransport(&monitored) == Status::kAccepted);
    assert(observer->events.size() == 3);  // No-op flush cannot self-wake.
    darwin_art::input::TerminateInputTransport(&monitored);
    darwin_art::input::TerminateInputTransport(&monitored);
    assert(observer->events.size() == 5 &&
           observer->events[3] == Kind::kTxTerminal &&
           observer->events[4] == Kind::kRxTerminal);
    wire.clear();
  }
  {
    darwin_art::input::InputTransport monitored(io, false);
    darwin_art::input::AdoptRemoteInputTransport(&monitored, 42);
    auto observer = std::make_shared<ProgressObserver>();
    observer->transport = &monitored;
    auto subscription = monitored.SubscribeProgress(OnProgress, observer);
    receive_chunks = {{}};
    receive_index = 0;
    assert(darwin_art::input::PumpInputTransport(&monitored, 42, {}) ==
           darwin_art::input::InputTransportStatus::kTerminal);
    assert(observer->events.size() == 1 && observer->events.front() ==
           darwin_art::input::InputResourceProgressKind::kRxTerminal);
    receive_chunks.clear();
    receive_index = 0;
  }
  {
    darwin_art::input::InputTransport fenced(io, false);
    auto& authority = fenced.RegistrationAuthority();
    assert(&authority == &fenced.RegistrationAuthority());
    darwin_art::input::TransportRegistrationAuthority::Claim claim;
    assert(authority.Reserve(
               71, darwin_art::input::TransportRegistrationRole::kReceiver,
               1, reinterpret_cast<void*>(1), &claim) ==
           darwin_art::input::TransportRegistrationResult::kAcquired);
    assert(claim.BeginRegistration() ==
           darwin_art::input::TransportRegistrationResult::kAcquired);
    assert(claim.ReleaseAfterQuiescence());
    darwin_art::input::AdoptRemoteInputTransport(&fenced, 42);
    const auto empty_prefix = fenced.CaptureAcceptedTxFence();
    assert(fenced.QueryTxFence(empty_prefix) == FenceStatus::kFlushed);
    assert(fenced.QueryTxFence({}) == FenceStatus::kInvalid);
    block_send = true;
    assert(darwin_art::input::SendInputTransportAck(&fenced, 1, true) ==
           darwin_art::input::InputTransportStatus::kAccepted);
    const auto old_prefix = fenced.CaptureAcceptedTxFence();
    assert(darwin_art::input::SendInputTransportAck(&fenced, 2, true) ==
           darwin_art::input::InputTransportStatus::kAccepted);
    const auto successor_prefix = fenced.CaptureAcceptedTxFence();
    assert(fenced.QueryTxFence(old_prefix) == FenceStatus::kPending);
    block_send = false;
    send_budget = 16; // One ACK; leave successor bytes behind.
    assert(darwin_art::input::FlushInputTransport(&fenced) ==
           darwin_art::input::InputTransportStatus::kBackpressured);
    assert(fenced.HasPendingTx());
    assert(fenced.QueryTxFence(old_prefix) == FenceStatus::kFlushed);
    assert(fenced.QueryTxFence(successor_prefix) == FenceStatus::kPending);
    darwin_art::input::InputTransport unrelated(io, false);
    assert(unrelated.QueryTxFence(old_prefix) == FenceStatus::kInvalid);
    block_send = true;
    error_value = 32;
    assert(darwin_art::input::FlushInputTransport(&fenced) ==
           darwin_art::input::InputTransportStatus::kTerminal);
    assert(fenced.QueryTxFence(old_prefix) == FenceStatus::kFlushed);
    assert(fenced.QueryTxFence(successor_prefix) == FenceStatus::kTerminal);
    block_send = false;
    error_value = 11;
    send_budget = std::numeric_limits<size_t>::max();
    wire.clear();
  }
  darwin_art::input::InputTransport tx(io, false);
  darwin_art::input::AdoptRemoteInputTransport(&tx, 42);
  send_limit = 5;
  assert(darwin_art::input::SendInputTransportPacket(&tx, Packet(1)) ==
         darwin_art::input::InputTransportStatus::kAccepted);
  assert(darwin_art::input::SendInputTransportPacket(&tx, Packet(2)) ==
         darwin_art::input::InputTransportStatus::kAccepted);
  const std::vector<uint8_t> frame = wire;
  assert(frame.size() > 5 && frame.size() % 2 == 0);

  darwin_art::input::InputTransport rx(io, false);
  darwin_art::input::AdoptRemoteInputTransport(&rx, 42);
  receive_chunks = {std::vector<uint8_t>(frame.begin(), frame.begin() + 3),
                    std::vector<uint8_t>(frame.begin() + 3, frame.end())};
  receive_index = 0;
  int packets = 0;
  const darwin_art::input::InputTransportPumpCallbacks callbacks{
      .on_packet = OnPacket, .context = &packets};
  assert(darwin_art::input::PumpInputTransport(&rx, 42, callbacks) ==
         darwin_art::input::InputTransportStatus::kAccepted);
  assert(packets == 2);

  wire.clear();
  block_send = true;
  assert(darwin_art::input::SendInputTransportAck(&tx, 9, true) ==
         darwin_art::input::InputTransportStatus::kAccepted);
  block_send = false;
  assert(darwin_art::input::FlushInputTransport(&tx) ==
         darwin_art::input::InputTransportStatus::kAccepted);
  assert(!wire.empty());

  receive_chunks = {};
  receive_index = 0;
  assert(darwin_art::input::PumpInputTransport(&rx, 42, callbacks) ==
         darwin_art::input::InputTransportStatus::kAccepted);
  receive_chunks = {std::vector<uint8_t>()};
  receive_index = 0;
  assert(darwin_art::input::PumpInputTransport(&rx, 42, callbacks) ==
         darwin_art::input::InputTransportStatus::kTerminal);

  // Focus controls share the transport's FIFO and partial-write semantics.
  wire.clear();
  send_limit = 3;
  block_send = true;
  darwin_art::input::InputTransport focus_tx(io, false);
  darwin_art::input::AdoptRemoteInputTransport(&focus_tx, 62);
  assert(darwin_art::input::SendInputTransportFocus(&focus_tx, 41, true) ==
         darwin_art::input::InputTransportStatus::kAccepted);
  block_send = false;
  send_budget = 7;
  assert(darwin_art::input::FlushInputTransport(&focus_tx) ==
         darwin_art::input::InputTransportStatus::kBackpressured);
  send_budget = std::numeric_limits<size_t>::max();
  assert(darwin_art::input::FlushInputTransport(&focus_tx) ==
         darwin_art::input::InputTransportStatus::kAccepted);
  const std::vector<uint8_t> focus_frame = wire;
  assert(focus_frame.size() ==
         sizeof(darwin_art::input::transport_wire::FocusControlFrame));

  darwin_art::input::InputTransport focus_rx(io, false);
  darwin_art::input::AdoptRemoteInputTransport(&focus_rx, 62);
  receive_chunks.clear();
  for (size_t offset = 0; offset < focus_frame.size();) {
    const size_t end = std::min(offset + size_t{2}, focus_frame.size());
    receive_chunks.emplace_back(focus_frame.begin() + offset,
                                focus_frame.begin() + end);
    offset = end;
  }
  receive_index = 0;
  FocusObservation focus_observation;
  const darwin_art::input::InputTransportPumpCallbacks focus_callbacks{
      .on_focus = OnFocus, .context = &focus_observation};
  assert(darwin_art::input::PumpInputTransport(&focus_rx, 62,
                                               focus_callbacks) ==
         darwin_art::input::InputTransportStatus::kAccepted);
  assert((focus_observation.values ==
          std::vector<std::pair<uint64_t, bool>>{{41, true}}));

  darwin_art::input::InputTransport missing_focus_rx(io, false);
  darwin_art::input::AdoptRemoteInputTransport(&missing_focus_rx, 63);
  receive_chunks = {focus_frame};
  receive_index = 0;
  assert(darwin_art::input::PumpInputTransport(&missing_focus_rx, 63, {}) ==
         darwin_art::input::InputTransportStatus::kBackpressured);
  assert(darwin_art::input::PumpInputTransport(&missing_focus_rx, 63,
                                               focus_callbacks) ==
         darwin_art::input::InputTransportStatus::kAccepted);

  // A deferred callback retains the frame at the FIFO head; consumed-stop
  // consumes exactly one frame and leaves the next one for a later pump.
  wire.clear();
  send_limit = 0;
  assert(darwin_art::input::SendInputTransportFocus(&focus_tx, 42, false) ==
         darwin_art::input::InputTransportStatus::kAccepted);
  assert(darwin_art::input::SendInputTransportFocus(&focus_tx, 43, true) ==
         darwin_art::input::InputTransportStatus::kAccepted);
  const std::vector<uint8_t> two_focus_frames = wire;
  receive_chunks = {two_focus_frames};
  receive_index = 0;
  focus_observation.values.clear();
  focus_observation.result =
      darwin_art::input::FocusControlCallbackResult::kDeferred;
  assert(darwin_art::input::PumpInputTransport(&focus_rx, 62,
                                               focus_callbacks) ==
         darwin_art::input::InputTransportStatus::kBackpressured);
  assert((focus_observation.values ==
          std::vector<std::pair<uint64_t, bool>>{{42, false}}));
  focus_observation.result =
      darwin_art::input::FocusControlCallbackResult::kConsumedStop;
  assert(darwin_art::input::PumpInputTransport(&focus_rx, 62,
                                               focus_callbacks) ==
         darwin_art::input::InputTransportStatus::kAccepted);
  assert(focus_observation.values.size() == 2);
  assert(focus_observation.values.back() ==
         std::make_pair(uint64_t{42}, false));
  focus_observation.result =
      darwin_art::input::FocusControlCallbackResult::kConsumed;
  assert(darwin_art::input::PumpInputTransport(&focus_rx, 62,
                                               focus_callbacks) ==
         darwin_art::input::InputTransportStatus::kAccepted);
  assert((focus_observation.values ==
          std::vector<std::pair<uint64_t, bool>>{{42, false}, {42, false},
                                                  {43, true}}));

  darwin_art::input::InputTransport reentrant_rx(io, false);
  darwin_art::input::AdoptRemoteInputTransport(&reentrant_rx, 64);
  receive_chunks = {two_focus_frames};
  receive_index = 0;
  ReentrantFocusObservation reentrant{&reentrant_rx, true, {}};
  const darwin_art::input::InputTransportPumpCallbacks reentrant_callbacks{
      .on_focus = OnReentrantFocus, .context = &reentrant};
  assert(darwin_art::input::PumpInputTransport(&reentrant_rx, 64, reentrant_callbacks) ==
         darwin_art::input::InputTransportStatus::kAccepted);
  assert((reentrant.epochs == std::vector<uint64_t>{42, 43}));

  // A peer may close immediately after writing its final ordered controls.
  // EOF is not permission to discard complete frames already received.
  darwin_art::input::InputTransport eof_rx(io, false);
  darwin_art::input::AdoptRemoteInputTransport(&eof_rx, 65);
  focus_observation.assert_live_during_callback = &eof_rx;
  receive_chunks = {two_focus_frames, {}};
  receive_index = 0;
  focus_observation.values.clear();
  focus_observation.result = darwin_art::input::FocusControlCallbackResult::kDeferred;
  assert(darwin_art::input::PumpInputTransport(&eof_rx, 65, focus_callbacks) ==
         darwin_art::input::InputTransportStatus::kBackpressured);
  assert(!eof_rx.IsTerminal() && focus_observation.values.size() == 1);
  focus_observation.result = darwin_art::input::FocusControlCallbackResult::kConsumedStop;
  assert(darwin_art::input::PumpInputTransport(&eof_rx, 65, focus_callbacks) ==
         darwin_art::input::InputTransportStatus::kAccepted);
  assert(!eof_rx.IsTerminal() && focus_observation.values.size() == 2);
  focus_observation.result = darwin_art::input::FocusControlCallbackResult::kConsumed;
  assert(darwin_art::input::PumpInputTransport(&eof_rx, 65, focus_callbacks) ==
         darwin_art::input::InputTransportStatus::kTerminal);
  assert((focus_observation.values ==
          std::vector<std::pair<uint64_t, bool>>{{42, false}, {42, false}, {43, true}}));
  assert(receive_index == 2); // EOF is remembered, not read again on retry.
  focus_observation.assert_live_during_callback = nullptr;

  for (const size_t prefix : {size_t{0}, size_t{3}, focus_frame.size() - 1}) {
    darwin_art::input::InputTransport truncated_rx(io, false);
    darwin_art::input::AdoptRemoteInputTransport(&truncated_rx, 66);
    receive_chunks.clear();
    if (prefix != 0) receive_chunks.emplace_back(focus_frame.begin(), focus_frame.begin() + prefix);
    receive_chunks.emplace_back();
    receive_index = 0;
    focus_observation.values.clear();
    assert(darwin_art::input::PumpInputTransport(&truncated_rx, 66, focus_callbacks) ==
           darwin_art::input::InputTransportStatus::kTerminal);
    assert(focus_observation.values.empty());
  }

  // Version, epoch, and wire-bool validation are terminal transport errors.
  for (const auto mutate : {0u, 1u, 2u, 3u}) {
    std::vector<uint8_t> malformed = focus_frame;
    if (mutate == 0) {
      const uint32_t version = 99;
      std::memcpy(malformed.data() + 4, &version, sizeof(version));
    } else if (mutate == 1) {
      const uint64_t epoch = 0;
      std::memcpy(malformed.data() + 8, &epoch, sizeof(epoch));
    } else if (mutate == 2) {
      const uint32_t focused = 2;
      std::memcpy(malformed.data() + 16, &focused, sizeof(focused));
    } else {
      const uint32_t reserved = 1;
      std::memcpy(malformed.data() + 20, &reserved, sizeof(reserved));
    }
    darwin_art::input::InputTransport malformed_rx(io, false);
    darwin_art::input::AdoptRemoteInputTransport(&malformed_rx, 62);
    receive_chunks = {std::move(malformed)};
    receive_index = 0;
    assert(darwin_art::input::PumpInputTransport(&malformed_rx, 62,
                                                 focus_callbacks) ==
           darwin_art::input::InputTransportStatus::kTerminal);
  }

  darwin_art::input::TerminateInputTransport(&focus_rx);
  assert(darwin_art::input::SendInputTransportFocus(&focus_rx, 44, true) ==
         darwin_art::input::InputTransportStatus::kTerminal);

  auto leased_transport = std::make_shared<darwin_art::input::InputTransport>(io, false);
  darwin_art::input::AdoptRemoteInputTransport(leased_transport.get(), 52);
  darwin_art::input::InputTransportPumpLease lease;
  receive_chunks = {};
  receive_index = 0;
  assert(lease.Register(reinterpret_cast<void*>(1), leased_transport, 52, 0x0003,
                        {}) );
  assert(registered_callback != nullptr && registered_data != nullptr);
  block_send = true;
  assert(darwin_art::input::SendInputTransportAck(leased_transport.get(), 10, false) ==
         darwin_art::input::InputTransportStatus::kAccepted);
  block_send = false;
  assert(registered_callback(52, 0x0002, registered_data) == 1);
  assert(!wire.empty());
  assert(lease.Retire());
  assert(registered_callback == nullptr && registered_data == nullptr);
  std::weak_ptr<darwin_art::input::InputTransport> weak_transport =
      leased_transport;
  leased_transport.reset();
  assert(weak_transport.expired());

  {
    darwin_art::input::InputTransport owned(io, true);
    assert(darwin_art::input::AdoptRemoteInputTransport(&owned, 44));
    assert(darwin_art::input::AdoptRemoteInputTransport(&owned, 44));
    assert(!darwin_art::input::AdoptRemoteInputTransport(&owned, 45));
    assert(owned.BindOutputEndpoint(44));
  }
  assert(close_count == 1);
  return 0;
}
