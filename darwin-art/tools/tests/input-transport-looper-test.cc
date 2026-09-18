#include "runtime/framework/input/input_transport.h"
#include "runtime/framework/input/input_transport_pump.h"
#include "compat/looper/android_looper_owner.h"
#include "darwin_art_bionic_socket_broker.h"

#include <android/looper.h>
#include <cassert>
#include <cerrno>
#include <cstring>
#include <ctime>
#include <fcntl.h>
#include <map>
#include <poll.h>
#include <sys/socket.h>
#include <atomic>
#include <thread>
#include <vector>
#include <unistd.h>

namespace {
std::map<int, int> wake_writers;
bool block_send = false;
size_t send_budget = static_cast<size_t>(-1);
int receive_calls = 0;
intptr_t SendFd(int fd, const void* data, size_t size, int) {
  if (block_send || send_budget == 0) { errno = EAGAIN; return -1; }
  const size_t count = size < send_budget ? size : send_budget;
  const auto sent = write(fd, data, count);
  if (sent > 0) send_budget -= static_cast<size_t>(sent);
  return sent;
}
intptr_t RecvFd(int fd, void* data, size_t size, int) {
  ++receive_calls;
  return read(fd, data, size);
}
int CloseFd(int fd) { return close(fd); }
int Error() {
  // InputTransport consumes Bionic errno values, not Darwin's EAGAIN=35.
  return errno == EAGAIN || errno == EWOULDBLOCK ? 11 : errno;
}
std::atomic<bool> callback_entered{false};
std::atomic<bool> callback_release{false};
int Callback(int, int, void* data) {
  ++*static_cast<int*>(data);
  return 0;
}
int BlockingCallback(int, int, void*) {
  callback_entered.store(true, std::memory_order_release);
  while (!callback_release.load(std::memory_order_acquire)) std::this_thread::yield();
  return 0;
}
struct ProgressContext {
  darwin_art::input::InputTransportPumpLease* lease;
  darwin_art::input::InputTransport* transport;
  int calls = 0;
  darwin_art::input::InputTransportStatus expected =
      darwin_art::input::InputTransportStatus::kAccepted;
};
void WritableProgress(void* opaque,
                      darwin_art::input::InputTransportStatus status) {
  auto* context = static_cast<ProgressContext*>(opaque);
  assert(status == context->expected);
  assert(!context->transport->HasPendingTx());
  ++context->calls;
  // Real server progress may synchronously replace or retire a registration.
  // Its callback must not leave OnFd using the released Registration object.
  assert(context->lease->Retire());
}
struct RefreshContext {
  darwin_art::input::InputTransport* transport;
  int calls = 0;
  darwin_art::input::InputTransportStatus expected =
      darwin_art::input::InputTransportStatus::kAccepted;
};
void CountProgress(void* opaque,
                   darwin_art::input::InputTransportStatus status) {
  auto* context = static_cast<RefreshContext*>(opaque);
  assert(status == context->expected);
  assert(!context->transport->HasPendingTx());
  ++context->calls;
}
void ThrowProgress(void*, darwin_art::input::InputTransportStatus) {
  throw std::bad_alloc();
}
void CountTerminal(void* opaque) noexcept {
  ++*static_cast<int*>(opaque);
}
}  // namespace

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
  return write(fd, p, n);
}
extern "C" intptr_t darwin_art_bionic_socket_broker_recv(int fd, void* p, size_t n, int) {
  return read(fd, p, n);
}
extern "C" int darwin_art_bionic_socket_broker_socketpair(int, int, int, int32_t fds[2]) {
  return pipe(fds);
}
extern "C" int darwin_art_bionic_socket_broker_close(int fd) { return close(fd); }
extern "C" int darwin_art_bionic_errno_load() { return errno; }

namespace darwin_art {
uint64_t AndroidUptimeNanos() {
  timespec value{};
  assert(clock_gettime(CLOCK_MONOTONIC, &value) == 0);
  return static_cast<uint64_t>(value.tv_sec) * 1000000000ULL + value.tv_nsec;
}
}  // namespace darwin_art

int main() {
  // Bound failures of the isolated regression without touching APK processes.
  alarm(10);
  void* looper = darwin_art::looper::PrepareCurrent();
  assert(looper != nullptr);
  auto transport = std::make_shared<darwin_art::input::InputTransport>(
      darwin_art::input::InputTransportIo{SendFd, RecvFd, CloseFd, Error}, true);
  int pair[2];
  assert(pipe(pair) == 0);
  int callbacks = 0;
  darwin_art::input::InputTransportPumpLease lease;
  assert(lease.Register(looper, transport, pair[0], ALOOPER_EVENT_INPUT,
                        {}, Callback, &callbacks));
  assert(!lease.IsQuiescent());
  assert(write(pair[1], "x", 1) == 1);
  assert(ALooper_pollOnce(100, nullptr, nullptr, nullptr) == ALOOPER_POLL_CALLBACK);
  assert(callbacks == 1);
  assert(lease.Retire());
  assert(lease.IsQuiescent());
  close(pair[0]);
  close(pair[1]);
  transport.reset();

  // A provider-side replacement makes exact removal fail. The shared control
  // stays closed and retains callback ownership so a later retry cannot
  // silently reopen or discard the registration.
  {
    auto retained_owner = std::make_shared<int>(7);
    std::weak_ptr<int> retained_weak = retained_owner;
    auto retained_transport = std::make_shared<darwin_art::input::InputTransport>(
        darwin_art::input::InputTransportIo{SendFd, RecvFd, CloseFd, Error}, true);
    int retained_pair[2];
    assert(pipe(retained_pair) == 0);
    darwin_art::input::InputTransportPumpLease retained_lease;
    assert(retained_lease.Register(looper, retained_transport, retained_pair[0],
                                   ALOOPER_EVENT_INPUT, {}, Callback, &callbacks,
                                   retained_owner));
    retained_owner.reset();
    assert(!retained_weak.expired());
    assert(ALooper_removeFd(static_cast<ALooper*>(looper), retained_pair[0]) == 1);
    // Zero means the exact registration is already absent/replaced; it is a
    // settled removal, not a reason to reopen or retain a stale pointer.
    assert(retained_lease.Retire());
    assert(retained_weak.expired());
    close(retained_pair[0]);
    close(retained_pair[1]);
    retained_transport.reset();
  }

  auto race_transport = std::make_shared<darwin_art::input::InputTransport>(
      darwin_art::input::InputTransportIo{SendFd, RecvFd, CloseFd, Error}, true);
  int race_pair[2];
  assert(pipe(race_pair) == 0);
  darwin_art::input::InputTransportPumpLease race_lease;
  assert(race_lease.Register(looper, race_transport, race_pair[0],
                             ALOOPER_EVENT_INPUT, {}, BlockingCallback));
  assert(write(race_pair[1], "r", 1) == 1);
  std::thread retire_thread([&race_lease] {
    while (!callback_entered.load(std::memory_order_acquire))
      std::this_thread::yield();
    assert(race_lease.Retire());
    assert(!race_lease.IsQuiescent());
    // Retirement must complete while the callback is still admitted. Release
    // it from this thread: pollOnce cannot return until the callback returns.
    callback_release.store(true, std::memory_order_release);
  });
  assert(ALooper_pollOnce(100, nullptr, nullptr, nullptr) == ALOOPER_POLL_CALLBACK);
  retire_thread.join();
  assert(race_lease.IsQuiescent());
  close(race_pair[0]);
  close(race_pair[1]);
  race_transport.reset();

  int progress_pair[2];
  assert(::socketpair(AF_UNIX, SOCK_STREAM, 0, progress_pair) == 0);
  assert(fcntl(progress_pair[0], F_SETFL, O_NONBLOCK) == 0);
  auto progress_transport = std::make_shared<darwin_art::input::InputTransport>(
      darwin_art::input::InputTransportIo{SendFd, RecvFd, CloseFd, Error}, true);
  darwin_art::input::AdoptRemoteInputTransport(progress_transport.get(),
                                             progress_pair[0]);
  darwin_art::input::InputTransportPumpLease progress_lease;
  block_send = true;
  assert(darwin_art::input::SendInputTransportAck(progress_transport.get(),
                                                 17, true) ==
         darwin_art::input::InputTransportStatus::kAccepted);
  assert(progress_transport->HasPendingTx());
  block_send = false;
  ProgressContext progress{&progress_lease, progress_transport.get()};
  assert(progress_lease.Register(
      looper, progress_transport, progress_pair[0],
      ALOOPER_EVENT_INPUT | ALOOPER_EVENT_OUTPUT,
      {.on_progress = WritableProgress, .context = &progress}));
  assert(ALooper_pollOnce(100, nullptr, nullptr, nullptr) == ALOOPER_POLL_CALLBACK);
  assert(progress.calls == 1);
  assert(progress_lease.Retire());
  progress_transport.reset();
  close(progress_pair[1]);

  // Refresh uses the same operation control: a pending TX admitted after an
  // input-only registration must publish OUTPUT and eventually flush.
  int refresh_pair[2];
  assert(::socketpair(AF_UNIX, SOCK_STREAM, 0, refresh_pair) == 0);
  assert(fcntl(refresh_pair[0], F_SETFL, O_NONBLOCK) == 0);
  auto refresh_transport = std::make_shared<darwin_art::input::InputTransport>(
      darwin_art::input::InputTransportIo{SendFd, RecvFd, CloseFd, Error}, true);
  darwin_art::input::AdoptRemoteInputTransport(refresh_transport.get(), refresh_pair[0]);
  block_send = true;
  assert(darwin_art::input::SendInputTransportAck(refresh_transport.get(), 18, true) ==
         darwin_art::input::InputTransportStatus::kAccepted);
  block_send = false;
  darwin_art::input::InputTransportPumpLease refresh_lease;
  RefreshContext refresh{refresh_transport.get()};
  assert(refresh_lease.Register(looper, refresh_transport, refresh_pair[0],
                                ALOOPER_EVENT_INPUT,
                                {.on_progress = CountProgress, .context = &refresh}));
  const int refresh_reads_before = receive_calls;
  assert(refresh_lease.SetWritable(true));
  assert(ALooper_pollOnce(100, nullptr, nullptr, nullptr) == ALOOPER_POLL_CALLBACK);
  assert(refresh.calls == 1);
  // Read-capable owners still retry buffered RX on OUTPUT-only readiness.
  assert(receive_calls > refresh_reads_before);
  assert(refresh_lease.Retire());
  refresh_transport.reset();
  close(refresh_pair[1]);

  // WMS server publications have no imported remote endpoint. Bind the real
  // output owner before traffic, retry the same socket, and never consume RX.
  {
    using namespace darwin_art::input;
    int publication_pair[2];
    assert(::socketpair(AF_UNIX, SOCK_STREAM, 0, publication_pair) == 0);
    auto publication = std::make_shared<InputTransport>(
        InputTransportIo{SendFd, RecvFd, CloseFd, Error}, false);
    InputTransportPumpLease owner;
    struct PublicationProgress { int calls = 0; bool partial = false; } context;
    assert(owner.Register(looper, publication, publication_pair[0], 0,
                           {.on_progress = [](void* opaque, InputTransportStatus status) {
                              auto& progress = *static_cast<PublicationProgress*>(opaque);
                              ++progress.calls;
                              progress.partial = status == InputTransportStatus::kBackpressured;
                            }, .context = &context}));
    assert(publication->RemoteEndpointFd() == -1);
    InputTransportPumpLease wrong_owner;
    assert(!wrong_owner.Register(looper, publication, publication_pair[1],
                                 ALOOPER_EVENT_OUTPUT, {}));
    InputTransportPumpLease wake_owner;
    assert(wake_owner.Register(looper, publication, publication_pair[1],
                               ALOOPER_EVENT_INPUT, {}));
    assert(wake_owner.SetWritableResult(true) == InputTransportWritableResult::kTerminal);
    assert(!publication->IsTerminal());
    assert(wake_owner.Retire() && wake_owner.IsQuiescent());
    block_send = true;
    assert(SendInputTransportFocusOnFd(publication.get(), publication_pair[0],
                                       904, true) == InputTransportStatus::kAccepted);
    const auto prefix = publication->CaptureAcceptedTxFence();
    block_send = false;
    send_budget = 3;
    assert(owner.SetWritable(true));
    const int before = receive_calls;
    for (int i = 0; i < 256 && context.calls == 0; ++i)
      (void)ALooper_pollOnce(10, nullptr, nullptr, nullptr);
    assert(context.calls == 1 && context.partial && receive_calls == before);
    assert(publication->QueryTxFence(prefix) == InputTransportTxFenceStatus::kPending);
    send_budget = static_cast<size_t>(-1);
    for (int i = 0; i < 256 && context.calls == 1; ++i)
      (void)ALooper_pollOnce(10, nullptr, nullptr, nullptr);
    assert(context.calls == 2 && !context.partial && receive_calls == before);
    const auto expected = transport_wire::EncodeFocusControl({904, true});
    transport_wire::FocusControlFrame received{};
    assert(read(publication_pair[1], &received, sizeof(received)) == sizeof(received));
    assert(std::memcmp(&received, &expected, sizeof(expected)) == 0);
    assert(publication->QueryTxFence(prefix) == InputTransportTxFenceStatus::kFlushed);
    assert(owner.Retire() && owner.IsQuiescent());
    close(publication_pair[0]);
    close(publication_pair[1]);
  }
  // A retired OUTPUT owner must not consume bytes belonging to the reader.
  int output_pair[2];
  assert(::socketpair(AF_UNIX, SOCK_STREAM, 0, output_pair) == 0);
  assert(fcntl(output_pair[0], F_SETFL, O_NONBLOCK) == 0);
  auto output_transport = std::make_shared<darwin_art::input::InputTransport>(
      darwin_art::input::InputTransportIo{SendFd, RecvFd, CloseFd, Error}, true);
  darwin_art::input::AdoptRemoteInputTransport(output_transport.get(), output_pair[0]);
  block_send = true;
  assert(darwin_art::input::SendInputTransportAck(output_transport.get(), 19, true) ==
         darwin_art::input::InputTransportStatus::kAccepted);
  block_send = false;
  const char marker = 'R';
  assert(write(output_pair[1], &marker, 1) == 1);
  darwin_art::input::InputTransportPumpLease output_lease;
  RefreshContext output{output_transport.get()};
  int output_terminals = 0;
  struct OutputContext {
    RefreshContext progress;
    int* terminals;
    darwin_art::input::InputTransportPumpLease* lease;
    int quiescences = 0;
  };
  // Progress and terminal share one independently owned callback context.
  OutputContext output_context{output, &output_terminals, &output_lease};
  assert(output_lease.Register(looper, output_transport, output_pair[0],
      ALOOPER_EVENT_OUTPUT, {
        .on_progress = [](void* p, darwin_art::input::InputTransportStatus s) {
          CountProgress(&static_cast<OutputContext*>(p)->progress, s);
        }, .context = &output_context,
        .on_terminal = [](void* p) noexcept {
          auto* context = static_cast<OutputContext*>(p);
          assert(!context->lease->IsQuiescent());
          ++*context->terminals;
        },
        .on_quiescent = [](void* p) noexcept {
          auto* context = static_cast<OutputContext*>(p);
          assert(context->lease->IsQuiescent());
          ++context->quiescences;
          // Completion reentry must not notify again or touch stale FD state.
          assert(context->lease->Retire());
        }}));
  assert(output_lease.SetWritable(true));
  const int reads_before = receive_calls;
  assert(ALooper_pollOnce(100, nullptr, nullptr, nullptr) == ALOOPER_POLL_CALLBACK);
  assert(output_context.progress.calls == 1 && receive_calls == reads_before);
  bool idle = false;
  // The pipe-backed test eventfd retains prior registration wake tokens.
  for (int i = 0; i < 256; ++i) {
    const int result = ALooper_pollOnce(10, nullptr, nullptr, nullptr);
    assert(result != ALOOPER_POLL_CALLBACK);
    if (result == ALOOPER_POLL_TIMEOUT) { idle = true; break; }
  }
  assert(idle);
  char preserved = 0;
  assert(read(output_pair[0], &preserved, 1) == 1 && preserved == marker);
  // An OUTPUT-only registration observing peer HUP must terminate
  // without attempting RX or spinning on an empty successful TX flush.
  output_context.progress.expected = darwin_art::input::InputTransportStatus::kTerminal;
  // Darwin poll ignores zero-interest descriptors; re-enable OUTPUT to observe
  // actual peer readiness without granting this lease any RX authority.
  assert(output_lease.SetWritable(true));
  close(output_pair[1]);
  bool ended = false;
  for (int i = 0; i < 256; ++i) {
    const int result = ALooper_pollOnce(10, nullptr, nullptr, nullptr);
    if (result == ALOOPER_POLL_CALLBACK) { ended = true; break; }
    assert(result == ALOOPER_POLL_WAKE || result == ALOOPER_POLL_TIMEOUT);
  }
  assert(ended && output_context.progress.calls == 2 && output_terminals == 1);
  assert(output_context.quiescences == 1);
  assert(output_transport->IsTxTerminal() && !output_transport->IsRxTerminal() && output_lease.IsQuiescent());
  assert(receive_calls == reads_before);
  assert(output_lease.Retire());
  assert(output_context.quiescences == 1);
  output_transport.reset();

  // The no-remote fast path must not mask resource termination either.
  int local_pair[2];
  assert(::socketpair(AF_UNIX, SOCK_STREAM, 0, local_pair) == 0);
  auto local_transport = std::make_shared<darwin_art::input::InputTransport>(
      darwin_art::input::InputTransportIo{SendFd, RecvFd, CloseFd, Error}, false);
  RefreshContext local{local_transport.get(), 0,
      darwin_art::input::InputTransportStatus::kTerminal};
  darwin_art::input::InputTransportPumpLease local_lease;
  assert(local_lease.Register(looper, local_transport, local_pair[0],
      ALOOPER_EVENT_OUTPUT, {.on_progress = CountProgress, .context = &local}));
  close(local_pair[1]);
  for (int i = 0; i < 256 && local.calls == 0; ++i)
    (void)ALooper_pollOnce(10, nullptr, nullptr, nullptr);
  assert(local.calls == 1 && local_transport->IsTxTerminal() && !local_transport->IsRxTerminal());
  assert(local_lease.IsQuiescent() && receive_calls == reads_before);
  assert(local_lease.Retire());
  close(local_pair[0]);

  int terminal_pair[2];
  assert(::socketpair(AF_UNIX, SOCK_STREAM, 0, terminal_pair) == 0);
  assert(fcntl(terminal_pair[0], F_SETFL, O_NONBLOCK) == 0);
  auto terminal_transport = std::make_shared<darwin_art::input::InputTransport>(
      darwin_art::input::InputTransportIo{SendFd, RecvFd, CloseFd, Error}, true);
  darwin_art::input::AdoptRemoteInputTransport(terminal_transport.get(),
                                             terminal_pair[0]);
  darwin_art::input::InputTransportPumpLease terminal_lease;
  ProgressContext terminal{&terminal_lease, terminal_transport.get(), 0,
      darwin_art::input::InputTransportStatus::kTerminal};
  assert(terminal_lease.Register(
      looper, terminal_transport, terminal_pair[0], ALOOPER_EVENT_INPUT,
      {.on_progress = WritableProgress, .context = &terminal}));
  close(terminal_pair[1]);
  assert(ALooper_pollOnce(100, nullptr, nullptr, nullptr) == ALOOPER_POLL_CALLBACK);
  assert(terminal.calls == 1 && terminal_transport->IsRxTerminal() && !terminal_transport->IsTxTerminal());
  assert(terminal_lease.Retire());
  terminal_transport.reset();
  // A throwing policy must not escape the C provider or hold operation=true
  // forever. Resource termination is published once after exact removal.
  int exception_pair[2];
  assert(::socketpair(AF_UNIX, SOCK_STREAM, 0, exception_pair) == 0);
  assert(fcntl(exception_pair[0], F_SETFL, O_NONBLOCK) == 0);
  auto exception_transport = std::make_shared<darwin_art::input::InputTransport>(
      darwin_art::input::InputTransportIo{SendFd, RecvFd, CloseFd, Error}, true);
  darwin_art::input::AdoptRemoteInputTransport(exception_transport.get(), exception_pair[0]);
  darwin_art::input::InputTransportPumpLease exception_lease;
  int terminations = 0;
  assert(exception_lease.Register(
      looper, exception_transport, exception_pair[0], ALOOPER_EVENT_OUTPUT,
      {.on_progress = ThrowProgress, .context = &terminations,
       .on_terminal = CountTerminal}));
  assert(ALooper_pollOnce(100, nullptr, nullptr, nullptr) == ALOOPER_POLL_CALLBACK);
  assert(terminations == 1);
  assert(exception_lease.Retire());
  assert(exception_lease.IsQuiescent());
  assert(exception_lease.Retire());
  assert(terminations == 1);
  // Exact removal may leave the provider's wake notification pending; neither
  // that wake nor a timeout may run the retired callback again.
  assert(ALooper_pollOnce(0, nullptr, nullptr, nullptr) != ALOOPER_POLL_CALLBACK);
  exception_transport.reset();
  close(exception_pair[1]);
  alarm(0);
  return 0;
}
