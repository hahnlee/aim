#include "runtime/framework/input/input_transport.h"
#include "runtime/framework/input/input_transport_pump.h"
#include "compat/looper/android_looper_owner.h"
#include "darwin_art_bionic_socket_broker.h"
#include "probes/fixture_input_exchange.h"

#include <android/looper.h>
#include <algorithm>
#include <cassert>
#include <cerrno>
#include <fcntl.h>
#include <map>
#include <poll.h>
#include <sys/socket.h>
#include <unistd.h>
#include <vector>
#include <atomic>
#include <functional>
#include <thread>

namespace {
std::map<int, int> wake_writers;
bool block_send = true;
int send_failures = 0;
std::function<void()> during_send;
intptr_t Send(int fd, const void* bytes, size_t size, int) {
  if (during_send) {
    auto callback = std::move(during_send);
    during_send = nullptr;
    callback();
  }
  if (block_send) { errno = EAGAIN; return -1; }
  const auto result = write(fd, bytes, size);
  if (result < 0 && errno == EPIPE) ++send_failures;
  return result;
}
intptr_t Receive(int fd, void* bytes, size_t size, int) {
  return read(fd, bytes, size);
}
int Close(int fd) { return close(fd); }
int Error() { return errno == EAGAIN || errno == EWOULDBLOCK ? 11 : errno; }
struct Observation {
  darwin_art::input::InputTransport* transport = nullptr;
  std::vector<uint64_t> epochs;
  int terminal = 0;
  int progress = 0;
};
darwin_art::input::FocusControlCallbackResult Focus(
    void* context, uint64_t epoch, bool focused) noexcept {
  assert(!focused);
  auto* observation = static_cast<Observation*>(context);
  assert(!observation->transport->IsRxTerminal());
  observation->epochs.push_back(epoch);
  return darwin_art::input::FocusControlCallbackResult::kConsumed;
}
}

extern "C" int darwin_art_bionic_socket_broker_eventfd(uint32_t, int) {
  int fds[2];
  assert(pipe(fds) == 0);
  wake_writers[fds[0]] = fds[1];
  return fds[0];
}
extern "C" intptr_t darwin_art_bionic_socket_broker_read(int fd, void* p, size_t n) {
  return read(fd, p, n);
}
extern "C" intptr_t darwin_art_bionic_socket_broker_write(int fd, const void* p, size_t n) {
  return write(wake_writers.at(fd), p, n);
}
extern "C" int darwin_art_bionic_socket_broker_poll(DarwinArtBionicPollFd* fds,
                                                    size_t count, int timeout) {
  std::vector<pollfd> host;
  host.reserve(count);
  for (size_t i = 0; i < count; ++i) host.push_back({fds[i].fd, fds[i].events, 0});
  const int result = poll(host.data(), host.size(), timeout);
  for (size_t i = 0; i < count; ++i) fds[i].revents = host[i].revents;
  return result;
}
extern "C" int darwin_art_bionic_socket_broker_fcntl(int fd, int cmd, intptr_t arg) {
  return fcntl(fd, cmd, arg);
}
extern "C" intptr_t darwin_art_bionic_socket_broker_send(int fd, const void* p, size_t n, int) {
  return Send(fd, p, n, 0);
}
extern "C" intptr_t darwin_art_bionic_socket_broker_recv(int fd, void* p, size_t n, int) {
  return Receive(fd, p, n, 0);
}
extern "C" int darwin_art_bionic_socket_broker_socketpair(int, int, int, int* fds) {
  return socketpair(AF_UNIX, SOCK_STREAM, 0, fds);
}
extern "C" int darwin_art_bionic_socket_broker_close(int fd) { return close(fd); }
extern "C" int darwin_art_bionic_errno_load() { return Error(); }

void RunScenario(void* looper, bool custom_reader, bool half_close) {
  using namespace darwin_art::input;
  block_send = true;
  send_failures = 0;
  int pair[2];
  assert(socketpair(AF_UNIX, SOCK_STREAM, 0, pair) == 0);
  assert(fcntl(pair[0], F_SETFL, O_NONBLOCK) == 0);
  const int no_signal = 1;
  assert(setsockopt(pair[0], SOL_SOCKET, SO_NOSIGPIPE, &no_signal, sizeof(no_signal)) == 0);
  auto transport = std::make_shared<InputTransport>(
      InputTransportIo{Send, Receive, Close, Error}, true);
  assert(AdoptRemoteInputTransport(transport.get(), pair[0]));
  assert(SendInputTransportAck(transport.get(), 1, true) == InputTransportStatus::kAccepted);
  const auto fence = transport->CaptureAcceptedTxFence();
  assert(transport->QueryTxFence(fence) == InputTransportTxFenceStatus::kPending);
  Observation observation;
  observation.transport = transport.get();
  InputTransportPumpLease lease;
  assert(lease.Register(looper, transport, pair[0], ALOOPER_EVENT_INPUT,
      {.on_focus = Focus,
       .on_progress = [](void* p, InputTransportStatus status) {
         auto* observation = static_cast<Observation*>(p);
         assert(status == (observation->transport->IsRxTerminal()
                              ? InputTransportStatus::kTerminal
                              : InputTransportStatus::kAccepted));
         ++observation->progress;
       }, .context = &observation,
       .on_terminal = [](void* p) noexcept {
         ++static_cast<Observation*>(p)->terminal;
       }},
       custom_reader ? +[](int fd, int, void* p) {
         auto* observation = static_cast<Observation*>(p);
         const InputTransportPumpCallbacks callbacks{.on_focus = Focus, .context = p};
         return PumpInputTransport(observation->transport, fd, callbacks) ==
                        InputTransportStatus::kTerminal ? 0 : 1;
       } : nullptr, &observation));
  // Darwin peer SHUT_RD does not reliably make the writer return EPIPE.
  // Explicitly end only local TX; the independent RX stream remains live.
  if (half_close) assert(shutdown(pair[0], SHUT_WR) == 0);
  const auto final_loss = transport_wire::EncodeFocusControl({51, false});
  assert(write(pair[1], &final_loss, sizeof(final_loss)) == sizeof(final_loss));
  if (!half_close) close(pair[1]);
  block_send = false;
  assert(ALooper_pollOnce(100, nullptr, nullptr, nullptr) == ALOOPER_POLL_CALLBACK);
  assert(send_failures == 1); // Actual closed socket, not simulated terminal.
  assert((observation.epochs == std::vector<uint64_t>{51}));
  assert(transport->QueryTxFence(fence) == InputTransportTxFenceStatus::kTerminal);
  assert(transport->OutputSnapshot().terminal && transport->HasPendingTx());
  if (half_close) {
    assert(transport->IsTxTerminal() && !transport->IsRxTerminal() && !transport->IsTerminal());
    assert(observation.terminal == 0 && !lease.IsQuiescent());
    assert(lease.SetWritableResult(true) == InputTransportWritableResult::kApplied);
    // Failed TX never arms a constantly-writable descriptor or closes live RX.
    assert(ALooper_pollOnce(0, nullptr, nullptr, nullptr) != ALOOPER_POLL_CALLBACK);
    const auto next_loss = transport_wire::EncodeFocusControl({52, false});
    assert(write(pair[1], &next_loss, sizeof(next_loss)) == sizeof(next_loss));
    close(pair[1]);
    assert(ALooper_pollOnce(100, nullptr, nullptr, nullptr) == ALOOPER_POLL_CALLBACK);
    assert((observation.epochs == std::vector<uint64_t>{51, 52}));
    assert(send_failures == 1); // Failed TX is never retried.
  }
  assert(observation.progress == (custom_reader ? 0 : half_close ? 2 : 1));
  assert(observation.terminal == 1 && transport->IsTerminal());
  assert(lease.IsQuiescent());
  assert(ALooper_pollOnce(0, nullptr, nullptr, nullptr) != ALOOPER_POLL_CALLBACK);
  assert(observation.terminal == 1);
}

void RunRxEofWithHealthyTx(void* looper, bool custom_reader) {
  using namespace darwin_art::input;
  int pair[2];
  assert(socketpair(AF_UNIX, SOCK_STREAM, 0, pair) == 0);
  assert(fcntl(pair[0], F_SETFL, O_NONBLOCK) == 0);
  auto transport = std::make_shared<InputTransport>(
      InputTransportIo{Send, Receive, Close, Error}, true);
  assert(AdoptRemoteInputTransport(transport.get(), pair[0]));
  block_send = true;
  assert(SendInputTransportAck(transport.get(), 2, true) == InputTransportStatus::kAccepted);
  const auto fence = transport->CaptureAcceptedTxFence();
  struct EofObservation {
    InputTransport* transport;
    std::vector<InputTransportStatus> progress;
    int terminal = 0;
    int reader_calls = 0;
    int quiescent = 0;
  } observation{transport.get(), {}, 0};
  InputTransportPumpLease lease;
  const InputTransportPumpCallbacks callbacks{
       .on_progress = [](void* p, InputTransportStatus status) {
         auto* observation = static_cast<EofObservation*>(p);
         assert(observation->transport->IsRxTerminal());
         assert(!observation->transport->IsTxTerminal());
         observation->progress.push_back(status);
       }, .context = &observation,
       .on_terminal = [](void* p) noexcept {
         ++static_cast<EofObservation*>(p)->terminal;
       }, .on_quiescent = [](void* p) noexcept {
         ++static_cast<EofObservation*>(p)->quiescent;
       }};
  if (custom_reader) {
    assert(lease.RegisterReader(looper, transport, pair[0], ALOOPER_EVENT_INPUT,
        callbacks, [](int fd, int, void* p) {
          auto& state = *static_cast<EofObservation*>(p);
          ++state.reader_calls;
          const auto status = PumpInputTransport(state.transport, fd, {});
          return status == InputTransportStatus::kTerminal
              ? InputTransportReaderResult::kReaderComplete
              : InputTransportReaderResult::kKeepReading;
        }, &observation));
  } else {
    assert(lease.Register(looper, transport, pair[0], ALOOPER_EVENT_INPUT, callbacks));
  }
  // End peer output only: it can still read the final ACK from local TX.
  assert(shutdown(pair[1], SHUT_WR) == 0);
  assert(ALooper_pollOnce(100, nullptr, nullptr, nullptr) == ALOOPER_POLL_CALLBACK);
  assert(transport->IsRxTerminal() && !transport->IsTxTerminal() && !transport->IsTerminal());
  assert(transport->QueryTxFence(fence) == InputTransportTxFenceStatus::kPending);
  assert(observation.terminal == 0 && !lease.IsQuiescent());
  assert(observation.quiescent == 0);
  if (custom_reader) assert(observation.reader_calls == 1);
  (void)lease.SetWritableResult(false);  // Cannot strand retained accepted TX.
  block_send = false;
  assert(ALooper_pollOnce(100, nullptr, nullptr, nullptr) == ALOOPER_POLL_CALLBACK);
  transport_wire::AckFrame ack;
  assert(read(pair[1], &ack, sizeof(ack)) == sizeof(ack));
  assert(ack.magic == transport_wire::kAckFrameMagic && ack.sequence == 2 && ack.handled == 1);
  assert(transport->QueryTxFence(fence) == InputTransportTxFenceStatus::kFlushed);
  if (custom_reader) {
    assert(observation.reader_calls == 1 && observation.progress.empty());
  } else {
    assert((observation.progress == std::vector<InputTransportStatus>{
        InputTransportStatus::kTerminal, InputTransportStatus::kAccepted}));
  }
  assert(observation.terminal == 1 && lease.IsQuiescent());
  assert(observation.quiescent == 1);
  assert(ALooper_pollOnce(0, nullptr, nullptr, nullptr) != ALOOPER_POLL_CALLBACK);
  close(pair[1]);
}

// Real socket + actual production framing. This checks the fixture wire
void RunExplicitRetirement(void* looper, bool legacy_zero, bool reentrant) {
  using namespace darwin_art::input;
  int pair[2];
  assert(socketpair(AF_UNIX, SOCK_STREAM, 0, pair) == 0);
  assert(fcntl(pair[0], F_SETFL, O_NONBLOCK) == 0);
  auto transport = std::make_shared<InputTransport>(
      InputTransportIo{Send, Receive, Close, Error}, true);
  assert(AdoptRemoteInputTransport(transport.get(), pair[0]));
  block_send = true;
  assert(SendInputTransportAck(transport.get(), 3, true) == InputTransportStatus::kAccepted);
  InputTransportPumpLease lease;
  struct State {
    InputTransport* transport;
    InputTransportPumpLease* lease;
    bool reentrant;
    int calls = 0;
    int quiescent = 0;
  } state{transport.get(), &lease, reentrant};
  const InputTransportPumpCallbacks callbacks{
      .context = &state, .on_quiescent = [](void* p) noexcept {
        ++static_cast<State*>(p)->quiescent;
      }};
  if (legacy_zero) {
    assert(lease.Register(looper, transport, pair[0], ALOOPER_EVENT_INPUT,
        callbacks, [](int fd, int, void* p) {
          auto& state = *static_cast<State*>(p);
          ++state.calls;
          assert(PumpInputTransport(state.transport, fd, {}) == InputTransportStatus::kTerminal);
          return 0;  // RX-terminal state never changes legacy removal authority.
        }, &state));
  } else {
    assert(lease.RegisterReader(looper, transport, pair[0], ALOOPER_EVENT_INPUT,
        callbacks, [](int fd, int, void* p) {
          auto& state = *static_cast<State*>(p);
          ++state.calls;
          if (state.reentrant) {
            assert(PumpInputTransport(state.transport, fd, {}) == InputTransportStatus::kTerminal);
            assert(state.lease->Retire());
          }
          // Without EOF this is an invalid completion claim and must fail closed.
          return InputTransportReaderResult::kReaderComplete;
        }, &state));
  }
  if (legacy_zero || reentrant) assert(shutdown(pair[1], SHUT_WR) == 0);
  else {
    const char byte = 'x';
    assert(write(pair[1], &byte, 1) == 1);
  }
  assert(ALooper_pollOnce(100, nullptr, nullptr, nullptr) == ALOOPER_POLL_CALLBACK);
  assert(state.calls == 1 && state.quiescent == 1 && lease.IsQuiescent());
  assert(transport->HasPendingTx());  // Explicit removal did not pretend delivery.
  assert(ALooper_pollOnce(0, nullptr, nullptr, nullptr) != ALOOPER_POLL_CALLBACK);
  close(pair[1]);
}

// Real socket + actual production framing. This checks the fixture wire
// contract, not Java dispatch or the actual receiver finish ledger.
void RunFixtureExchange() {
  using namespace darwin_art::input;
  using namespace darwin_art_graphics_fixture;
  int pair[2];
  assert(socketpair(AF_UNIX, SOCK_STREAM, 0, pair) == 0);
  for (int fd : pair) assert(fcntl(fd, F_SETFL, O_NONBLOCK) == 0);
  InputTransport transport({Send, Receive, Close, Error}, true);
  assert(AdoptRemoteInputTransport(&transport, pair[1]));
  struct Peer {
    int fd;
    size_t written = 0;
    static FixtureIoResult Write(void* p, const void* bytes, size_t count) {
      auto& peer = *static_cast<Peer*>(p);
      const auto n = write(peer.fd, bytes, std::min(count, size_t{3}));
      if (n < 0 && (errno == EAGAIN || errno == EWOULDBLOCK))
        return {FixtureIoStatus::kWouldBlock};
      if (n <= 0) return {FixtureIoStatus::kTerminal};
      peer.written += static_cast<size_t>(n);
      return {FixtureIoStatus::kProgress, static_cast<size_t>(n)};
    }
    static FixtureIoResult Read(void* p, void* bytes, size_t count) {
      auto& peer = *static_cast<Peer*>(p);
      const auto n = read(peer.fd, bytes, std::min(count, size_t{3}));
      if (n < 0 && (errno == EAGAIN || errno == EWOULDBLOCK))
        return {FixtureIoStatus::kWouldBlock};
      if (n <= 0) return {FixtureIoStatus::kTerminal};
      return {FixtureIoStatus::kProgress, static_cast<size_t>(n)};
    }
  } peer{pair[0]};
  FixtureInputExchange exchange;
  darwin_art::DarwinArtInputPacket packet;
  packet.kind = darwin_art::DarwinArtInputPacketKind::kKey;
  packet.key.version = 1;
  packet.key.size = sizeof(packet.key);
  packet.key.sequence = (1ULL << 48) + 9000;
  packet.key.key_code = 7;
  assert(exchange.Submit(packet));
  struct Admission { bool allowed = false; int delivered = 0; } admission;
  InputTransportPumpCallbacks callbacks{
    .on_packet = [](void* p, const darwin_art::DarwinArtInputPacket& received) {
      auto& admission = *static_cast<Admission*>(p);
      assert(received.key.sequence == (1ULL << 48) + 9000 && received.key.key_code == 7);
      if (!admission.allowed) return false;
      ++admission.delivered;
      return true;
    }, .context = &admission};
  const FixtureExchangeIo io{Peer::Write, Peer::Read, &peer};
  for (int i = 0; i < 1000 && !exchange.result().submitted; ++i) {
    exchange.Advance(io);
    PumpInputTransport(&transport, pair[1], callbacks);
  }
  assert(exchange.result().submitted && !exchange.result().completed);
  assert(admission.delivered == 0);
  for (int i = 0; i < 20; ++i) {
    exchange.Advance(io);
    assert(PumpInputTransport(&transport, pair[1], callbacks) == InputTransportStatus::kBackpressured);
  }
  assert(peer.written == sizeof(transport_wire::InputFrame));
  assert(!exchange.Submit(packet)); // A timeout does not permit a duplicate.
  admission.allowed = true;
  assert(PumpInputTransport(&transport, pair[1], callbacks) == InputTransportStatus::kAccepted);
  assert(admission.delivered == 1 && !exchange.result().completed);
  block_send = false;
  assert(SendInputTransportAck64(&transport, packet.key.sequence, false) == InputTransportStatus::kAccepted);
  for (int i = 0; i < 20 && !exchange.result().completed; ++i) exchange.Advance(io);
  FixtureExchangeResult result;
  assert(exchange.TakeCompleted(&result));
  assert(result.completed && !result.handled && result.packet_sequence == packet.key.sequence);
  assert(admission.delivered == 1 && peer.written == sizeof(transport_wire::InputFrame));
  close(pair[0]);
}

void RunTxQuiescence() {
  using namespace darwin_art::input;
  auto create = [](int* peer) {
    int pair[2];
    assert(socketpair(AF_UNIX, SOCK_STREAM, 0, pair) == 0);
    assert(fcntl(pair[0], F_SETFL, O_NONBLOCK) == 0);
    auto result = std::make_shared<InputTransport>(InputTransportIo{Send, Receive, Close, Error}, true);
    assert(AdoptRemoteInputTransport(result.get(), pair[0]));
    *peer = pair[1];
    return result;
  };
  assert(!TerminateInputTransportTxAndQuiesce(nullptr));
  int peer;
  auto pending = create(&peer);
  block_send = true;
  assert(SendInputTransportAck(pending.get(), 71, true) == InputTransportStatus::kAccepted);
  const auto incomplete = pending->CaptureAcceptedTxFence();
  assert(pending->HasPendingTx());
  assert(TerminateInputTransportTxAndQuiesce(pending.get()));
  assert(!pending->HasPendingTx() && !pending->IsRxTerminal());
  assert(pending->QueryTxFence(incomplete) == InputTransportTxFenceStatus::kTerminal);
  assert(FlushInputTransport(pending.get()) == InputTransportStatus::kTerminal);
  assert(TerminateInputTransportTxAndQuiesce(pending.get()));
  close(peer);

  int outer_peer, inner_peer;
  auto outer = create(&outer_peer), inner = create(&inner_peer);
  block_send = false;
  auto observation = std::make_shared<Observation>();
  observation->transport = outer.get();
  auto subscription = outer->SubscribeProgress([](void* p, InputResourceProgress progress) noexcept {
    auto& state = *static_cast<Observation*>(p);
    if (progress.kind == InputResourceProgressKind::kTxTerminal) {
      ++state.terminal;
      assert(state.transport->OutputSnapshot().terminal); // Must not hold TX lock.
      assert(TerminateInputTransportTxAndQuiesce(state.transport)); // No recursive hints.
    }
  }, observation);
  during_send = [&] {
    during_send = [&] { assert(!TerminateInputTransportTxAndQuiesce(outer.get())); };
    assert(SendInputTransportAck(inner.get(), 72, true) == InputTransportStatus::kAccepted);
  };
  assert(SendInputTransportAck(outer.get(), 73, true) == InputTransportStatus::kAccepted);
  assert(observation->terminal == 1);
  const auto completed = outer->CaptureAcceptedTxFence();
  assert(outer->QueryTxFence(completed) == InputTransportTxFenceStatus::kFlushed);
  assert(TerminateInputTransportTxAndQuiesce(outer.get()) && observation->terminal == 1);
  assert(!outer->IsRxTerminal() && !inner->IsTxTerminal());
  close(outer_peer);
  close(inner_peer);

  auto flushing = create(&peer);
  block_send = true;
  assert(SendInputTransportAck(flushing.get(), 74, true) == InputTransportStatus::kAccepted);
  const auto flush_fence = flushing->CaptureAcceptedTxFence();
  block_send = false;
  std::atomic<bool> entered{false}, release{false};
  during_send = [&] {
    entered.store(true, std::memory_order_release);
    while (!release.load(std::memory_order_acquire)) std::this_thread::yield();
  };
  InputTransportStatus status = InputTransportStatus::kTerminal;
  std::thread writer([&] { status = FlushInputTransport(flushing.get()); });
  while (!entered.load(std::memory_order_acquire)) std::this_thread::yield();
  assert(!TerminateInputTransportTxAndQuiesce(flushing.get()));
  assert(flushing->IsTxTerminal() && !flushing->IsRxTerminal());
  release.store(true, std::memory_order_release);
  writer.join();
  assert(status == InputTransportStatus::kAccepted);
  assert(TerminateInputTransportTxAndQuiesce(flushing.get()));
  assert(flushing->QueryTxFence(flush_fence) == InputTransportTxFenceStatus::kFlushed);
  close(peer);
}

int main() {
  alarm(10);
  auto* looper = darwin_art::looper::PrepareCurrent();
  assert(looper != nullptr);
  for (bool custom_reader : {false, true}) {
    for (bool half_close : {false, true}) RunScenario(looper, custom_reader, half_close);
  }
  for (bool custom_reader : {false, true}) RunRxEofWithHealthyTx(looper, custom_reader);
  RunExplicitRetirement(looper, true, false);
  RunExplicitRetirement(looper, false, false);
  RunExplicitRetirement(looper, false, true);
  RunFixtureExchange();
  RunTxQuiescence();
  alarm(0);
}
