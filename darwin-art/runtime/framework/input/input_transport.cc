#include "input_transport.h"
#include "input_framed_reader.h"
#include "input_transport_pump.h"
#include "../../../tools/bionic-errno-tls/include/darwin_art_bionic_errno.h"

#include <algorithm>
#include <atomic>
#include <cerrno>
#include <mutex>
#include <limits>
#include <vector>

extern "C" int darwin_art_bionic_socket_broker_socketpair(int, int, int, int*);
extern "C" intptr_t darwin_art_bionic_socket_broker_send(int, const void*, size_t, int);
extern "C" intptr_t darwin_art_bionic_socket_broker_recv(int, void*, size_t, int);
extern "C" int darwin_art_bionic_socket_broker_close(int);

namespace darwin_art::input {

struct InputTransport::Impl {
  int read_fd = -1;
  int write_fd = -1;
  int remote_endpoint_fd = -1;
  bool owns_descriptors = true;
  std::atomic<bool> tx_terminal{false};
  std::atomic<bool> tx_deferred_terminal_progress{false};
  std::unique_ptr<FrameInputReader> rx_reader =
      std::make_unique<FrameInputReader>();
  mutable std::mutex tx_mutex;
  int tx_endpoint_fd = -1; // Aliases an owned or externally pinned descriptor.
  std::vector<uint8_t> tx_pending;
  const std::shared_ptr<const void> tx_identity = std::make_shared<const char>(0);
  TransportRegistrationAuthority registration_authority;
  const std::shared_ptr<InputResourceProgressSource> progress =
      std::make_shared<InputResourceProgressSource>();
  uint64_t tx_accepted_bytes = 0;
  uint64_t tx_completed_bytes = 0;
  InputTransportIo io;
};

namespace {

// Created before the TX lock, hence destroyed after it. Retain only the
// notification source so callback destruction/reentry cannot invalidate later
// publications; do not retain the transport or receiver through callbacks.
struct TransportProgressPublication {
  std::shared_ptr<InputResourceProgressSource> source;
  std::atomic<bool>* deferred_terminal = nullptr;
  InputResourceProgressKind terminal_kind = InputResourceProgressKind::kTxTerminal;
  bool accepted = false;
  bool advanced = false;
  bool terminal = false;
  void Terminate(std::atomic<bool>& state) noexcept {
    terminal = !state.exchange(true, std::memory_order_acq_rel) || terminal;
  }
  ~TransportProgressPublication() {
    // Consume before the first callback: a callback may retire its transport.
    // Busy/reentrant quiescence never notifies while a provider holds TX.
    if (deferred_terminal != nullptr &&
        deferred_terminal->exchange(false, std::memory_order_acq_rel)) terminal = true;
    if (accepted) source->Notify(InputResourceProgressKind::kTxAccepted);
    if (advanced) source->Notify(InputResourceProgressKind::kTxAdvanced);
    if (terminal) source->Notify(terminal_kind);
  }
};

// Allocation-free linked stack: nested providers can reenter termination of
// an outer transport. std::mutex::try_lock on our own mutex is not permitted.
struct TxAdmissionScope {
  const void* identity;
  TxAdmissionScope* previous;
  static thread_local TxAdmissionScope* current;
  explicit TxAdmissionScope(const void* value) : identity(value), previous(current) {
    current = this;
  }
  ~TxAdmissionScope() { current = previous; }
  static bool Owns(const void* value) {
    for (auto* scope = current; scope != nullptr; scope = scope->previous)
      if (scope->identity == value) return true;
    return false;
  }
};
thread_local TxAdmissionScope* TxAdmissionScope::current = nullptr;

constexpr int kMsgNoSignal = 0x4000;
constexpr size_t kMaxTxBytes = 4096u * 512u;
InputTransportIo DefaultIo() {
  return {darwin_art_bionic_socket_broker_send,
          darwin_art_bionic_socket_broker_recv,
          darwin_art_bionic_socket_broker_close,
          darwin_art_bionic_errno_load};
}

// TX validates the caller-owned packet before framing it. RX has an
// independent copy of this wire validation in FrameInputReader so the reader
// remains independently testable and owns all RX parsing state.
bool ValidPacket(const DarwinArtInputPacket& packet) {
  switch (packet.kind) {
    case DarwinArtInputPacketKind::kPointer:
      return packet.pointer.version == 2 &&
             packet.pointer.size >= sizeof(DarwinArtPointerEventV2) &&
             packet.pointer.action <= DARWIN_ART_POINTER_CANCEL &&
             packet.pointer.pointer_count > 0;
    case DarwinArtInputPacketKind::kKey:
      return packet.key.version == 1 &&
             packet.key.size >= sizeof(DarwinArtKeyEventV1) &&
             packet.key.action <= 1;
  }
  return false;
}

}  // namespace

InputTransportStatus InputTransport::FlushBytes() {
  if (impl_ == nullptr ||
      impl_->tx_terminal.load(std::memory_order_acquire) ||
      impl_->io.send == nullptr) {
    return InputTransportStatus::kTerminal;
  }
  TransportProgressPublication publication{impl_->progress, &impl_->tx_deferred_terminal_progress};
  std::lock_guard<std::mutex> lock(impl_->tx_mutex);
  TxAdmissionScope admission(impl_.get());
  if (impl_->tx_terminal.load(std::memory_order_acquire))
    return InputTransportStatus::kTerminal;
  try {
    while (!impl_->tx_pending.empty()) {
      const intptr_t sent = impl_->io.send(
          impl_->tx_endpoint_fd, impl_->tx_pending.data(),
          impl_->tx_pending.size(), kMsgNoSignal);
      if (sent <= 0) {
        const int error = LastError();
        if (sent < 0 && error == 4) continue;
        if (sent < 0 && error == 11) return InputTransportStatus::kBackpressured;
        publication.Terminate(impl_->tx_terminal);
        return InputTransportStatus::kTerminal;
      }
      const size_t count = static_cast<size_t>(sent);
      impl_->tx_completed_bytes += std::min(count, impl_->tx_pending.size());
      publication.advanced = true;
      if (count >= impl_->tx_pending.size()) {
        impl_->tx_pending.clear();
      } else {
        impl_->tx_pending.erase(impl_->tx_pending.begin(),
                                impl_->tx_pending.begin() + count);
      }
    }
  } catch (...) {
    // A provider exception may follow a partial write. The retained prefix
    // must never be retried as a fresh healthy submission.
    publication.Terminate(impl_->tx_terminal);
    throw;
  }
  return InputTransportStatus::kAccepted;
}

InputTransportStatus InputTransport::SendBytes(const void* data, size_t size,
                                               int endpoint_fd) {
  if (impl_ == nullptr || impl_->tx_terminal.load(std::memory_order_acquire) ||
      impl_->io.send == nullptr) {
    return InputTransportStatus::kTerminal;
  }
  TransportProgressPublication publication{impl_->progress, &impl_->tx_deferred_terminal_progress};
  std::lock_guard<std::mutex> lock(impl_->tx_mutex);
  TxAdmissionScope admission(impl_.get());
  if (impl_->tx_terminal.load(std::memory_order_acquire))
    return InputTransportStatus::kTerminal;
  if (endpoint_fd < 0) endpoint_fd = impl_->remote_endpoint_fd;
  if (endpoint_fd < 0) return InputTransportStatus::kTerminal;
  // Nonretryable submission rejection, not failure of the retained stream.
  if (impl_->tx_endpoint_fd >= 0 && impl_->tx_endpoint_fd != endpoint_fd)
    return InputTransportStatus::kTerminal;
  if (size > kMaxTxBytes - impl_->tx_pending.size())
    return InputTransportStatus::kBackpressured;
  if (size > std::numeric_limits<uint64_t>::max() - impl_->tx_accepted_bytes)
    return InputTransportStatus::kBackpressured;
  const auto* bytes = static_cast<const uint8_t*>(data);
  try {
    impl_->tx_pending.insert(impl_->tx_pending.end(), bytes, bytes + size);
  } catch (...) {
    return InputTransportStatus::kBackpressured;
  }
  impl_->tx_endpoint_fd = endpoint_fd;
  impl_->tx_accepted_bytes += size;
  publication.accepted = size != 0;
  try {
    while (!impl_->tx_pending.empty()) {
      const intptr_t sent = impl_->io.send(impl_->tx_endpoint_fd, impl_->tx_pending.data(),
                                           impl_->tx_pending.size(), kMsgNoSignal);
      if (sent <= 0) {
        const int error = LastError();
        if (sent < 0 && error == 4) continue;
        if (sent < 0 && error == 11) return InputTransportStatus::kAccepted;
        publication.Terminate(impl_->tx_terminal);
        return InputTransportStatus::kTerminal;
      }
      const size_t count = static_cast<size_t>(sent);
      impl_->tx_completed_bytes += std::min(count, impl_->tx_pending.size());
      publication.advanced = true;
      if (count >= impl_->tx_pending.size()) impl_->tx_pending.clear();
      else impl_->tx_pending.erase(impl_->tx_pending.begin(),
                                   impl_->tx_pending.begin() + count);
    }
  } catch (...) {
    // The complete frame is already queued. Submission is now uncertain;
    // terminalize this exact TX lane before releasing any policy claim.
    publication.Terminate(impl_->tx_terminal);
    throw;
  }
  return InputTransportStatus::kAccepted;
}

InputTransport::InputTransport(InputTransportIo transport_io,
                               bool owns_transport_descriptors)
    : impl_(std::make_unique<Impl>()) {
  impl_->owns_descriptors = owns_transport_descriptors;
  impl_->io = transport_io.send == nullptr || transport_io.receive == nullptr ||
                      transport_io.close == nullptr
                  ? DefaultIo()
                  : transport_io;
}

int InputTransport::LastError() const {
  return impl_ != nullptr && impl_->io.error != nullptr ? impl_->io.error()
                                                         : errno;
}

InputTransport::~InputTransport() {
  if (impl_ == nullptr || !impl_->owns_descriptors || impl_->io.close == nullptr)
    return;
  if (impl_->read_fd >= 0) impl_->io.close(impl_->read_fd);
  if (impl_->write_fd >= 0 && impl_->write_fd != impl_->read_fd)
    impl_->io.close(impl_->write_fd);
  if (impl_->remote_endpoint_fd >= 0 &&
      impl_->remote_endpoint_fd != impl_->read_fd &&
      impl_->remote_endpoint_fd != impl_->write_fd)
    impl_->io.close(impl_->remote_endpoint_fd);
}

int InputTransport::ReadFd() const { return impl_ == nullptr ? -1 : impl_->read_fd; }
int InputTransport::WriteFd() const { return impl_ == nullptr ? -1 : impl_->write_fd; }
int InputTransport::RemoteEndpointFd() const {
  return impl_ == nullptr ? -1 : impl_->remote_endpoint_fd;
}
bool InputTransport::IsTerminal() const {
  return IsTxTerminal() && IsRxTerminal();
}
bool InputTransport::IsTxTerminal() const {
  return impl_ == nullptr || impl_->tx_terminal.load(std::memory_order_acquire);
}
bool InputTransport::IsRxTerminal() const {
  return impl_ == nullptr || impl_->rx_reader == nullptr ||
         impl_->rx_reader->IsTerminal();
}
bool InputTransport::HasPendingTx() const {
  if (impl_ == nullptr) return false;
  std::lock_guard<std::mutex> lock(impl_->tx_mutex);
  return !impl_->tx_pending.empty();
}
bool InputTransport::BindOutputEndpoint(int fd) {
  if (impl_ == nullptr || fd < 0) return false;
  std::lock_guard<std::mutex> lock(impl_->tx_mutex);
  if (impl_->tx_terminal.load(std::memory_order_acquire)) return false;
  if (impl_->tx_endpoint_fd >= 0 && impl_->tx_endpoint_fd != fd) return false;
  impl_->tx_endpoint_fd = fd;
  return true;
}
InputTransportOutputSnapshot InputTransport::OutputSnapshot() const {
  if (impl_ == nullptr) return {};
  std::lock_guard<std::mutex> lock(impl_->tx_mutex);
  return {impl_->tx_endpoint_fd, !impl_->tx_pending.empty(),
          impl_->tx_terminal.load(std::memory_order_acquire)};
}

InputResourceProgressSource::SubscriptionHandle InputTransport::SubscribeProgress(
    void (*notify)(void*, InputResourceProgress) noexcept,
    std::weak_ptr<void> context) {
  return impl_->progress->Subscribe(notify, std::move(context));
}

TransportRegistrationAuthority& InputTransport::RegistrationAuthority() {
  return impl_->registration_authority;
}

const TransportRegistrationAuthority& InputTransport::RegistrationAuthority() const {
  return impl_->registration_authority;
}

InputTransportTxFence InputTransport::CaptureAcceptedTxFence() const {
  InputTransportTxFence fence;
  if (impl_ == nullptr) return fence;
  std::lock_guard<std::mutex> lock(impl_->tx_mutex);
  fence.identity_ = impl_->tx_identity;
  fence.accepted_bytes_ = impl_->tx_accepted_bytes;
  return fence;
}

InputTransportTxFenceStatus InputTransport::QueryTxFence(
    const InputTransportTxFence& fence) const {
  if (impl_ == nullptr || fence.identity_ == nullptr ||
      fence.identity_ != impl_->tx_identity)
    return InputTransportTxFenceStatus::kInvalid;
  std::lock_guard<std::mutex> lock(impl_->tx_mutex);
  if (impl_->tx_completed_bytes >= fence.accepted_bytes_)
    return InputTransportTxFenceStatus::kFlushed;
  return impl_->tx_terminal.load(std::memory_order_acquire)
             ? InputTransportTxFenceStatus::kTerminal
             : InputTransportTxFenceStatus::kPending;
}

bool OpenLocalInputTransport(InputTransport* transport) {
  if (transport == nullptr || transport->impl_ == nullptr ||
      transport->impl_->io.close == nullptr)
    return false;
  int fds[2] = {-1, -1};
  if (darwin_art_bionic_socket_broker_socketpair(1, 0x80801, 0, fds) != 0)
    return false;
  transport->impl_->read_fd = fds[0];
  transport->impl_->write_fd = fds[1];
  return true;
}

bool AdoptRemoteInputTransport(InputTransport* transport, int endpoint_fd) {
  if (transport == nullptr || transport->impl_ == nullptr || endpoint_fd < 0)
    return false;
  auto& impl = *transport->impl_;
  std::lock_guard<std::mutex> lock(impl.tx_mutex);
  if (impl.remote_endpoint_fd >= 0 && impl.remote_endpoint_fd != endpoint_fd)
    return false;
  impl.remote_endpoint_fd = endpoint_fd;
  return true;
}

InputTransportStatus FlushInputTransport(InputTransport* transport) {
  return transport == nullptr ? InputTransportStatus::kTerminal
                              : transport->FlushBytes();
}

void TerminateInputTransport(InputTransport* transport) {
  if (transport == nullptr || transport->impl_ == nullptr) return;
  const auto source = transport->impl_->progress;
  const bool tx_changed =
      !transport->impl_->tx_terminal.exchange(true, std::memory_order_acq_rel);
  const bool rx_changed = transport->impl_->rx_reader != nullptr &&
                          transport->impl_->rx_reader->Terminate();
  if (tx_changed) source->Notify(InputResourceProgressKind::kTxTerminal);
  if (rx_changed) source->Notify(InputResourceProgressKind::kRxTerminal);
}

void TerminateInputTransportTx(InputTransport* transport) {
  if (transport == nullptr || transport->impl_ == nullptr) return;
  const auto source = transport->impl_->progress;
  if (!transport->impl_->tx_terminal.exchange(true, std::memory_order_acq_rel))
    source->Notify(InputResourceProgressKind::kTxTerminal);
}

bool TerminateInputTransportTxAndQuiesce(InputTransport* transport) {
  if (transport == nullptr || transport->impl_ == nullptr) return false;
  auto& impl = *transport->impl_;
  const auto source = impl.progress;
  if (!impl.tx_terminal.exchange(true, std::memory_order_acq_rel))
    impl.tx_deferred_terminal_progress.store(true, std::memory_order_release);
  if (TxAdmissionScope::Owns(&impl)) return false;
  bool notify = false;
  {
    std::unique_lock<std::mutex> lock(impl.tx_mutex, std::try_to_lock);
    if (!lock.owns_lock()) return false;
    // All older admission has unwound, and terminal denies every new writer.
    // Unsent bytes are abandoned, not added to tx_completed_bytes.
    impl.tx_pending.clear();
    notify = impl.tx_deferred_terminal_progress.exchange(false, std::memory_order_acq_rel);
  }
  if (notify) source->Notify(InputResourceProgressKind::kTxTerminal);
  return true;
}

InputTransportStatus SendInputTransportPacket(
    InputTransport* transport, const DarwinArtInputPacket& packet) {
  if (!ValidPacket(packet)) return InputTransportStatus::kTerminal;
  transport_wire::InputFrame frame;
  frame.kind = static_cast<uint32_t>(packet.kind);
  frame.payload = packet;
  return transport == nullptr ? InputTransportStatus::kTerminal
                              : transport->SendBytes(&frame, sizeof(frame), -1);
}

InputTransportStatus SendInputTransportAck(InputTransport* transport,
                                            uint32_t sequence, bool handled) {
  transport_wire::AckFrame frame;
  frame.sequence = sequence;
  frame.handled = handled ? 1u : 0u;
  return transport == nullptr ? InputTransportStatus::kTerminal
                              : transport->SendBytes(&frame, sizeof(frame), -1);
}

InputTransportStatus SendInputTransportAck64(InputTransport* transport,
                                              uint64_t sequence, bool handled) {
  transport_wire::AckFrameV2 frame;
  frame.sequence = sequence;
  frame.handled = handled ? 1u : 0u;
  return transport == nullptr ? InputTransportStatus::kTerminal
                              : transport->SendBytes(&frame, sizeof(frame), -1);
}

InputTransportStatus SendInputTransportWindow(InputTransport* transport,
                                               int32_t left, int32_t top,
                                               int32_t right, int32_t bottom,
                                               bool visible) {
  transport_wire::WindowFrame frame;
  frame.left = left;
  frame.top = top;
  frame.right = right;
  frame.bottom = bottom;
  frame.visible = visible ? 1u : 0u;
  return transport == nullptr ? InputTransportStatus::kTerminal
                              : transport->SendBytes(&frame, sizeof(frame), -1);
}

InputTransportStatus SendInputTransportWindowOnFd(InputTransport* transport,
                                                   int endpoint_fd,
                                                   int32_t left, int32_t top,
                                                   int32_t right, int32_t bottom,
                                                   bool visible) {
  transport_wire::WindowFrame frame;
  frame.left = left;
  frame.top = top;
  frame.right = right;
  frame.bottom = bottom;
  frame.visible = visible ? 1u : 0u;
  return transport == nullptr ? InputTransportStatus::kTerminal
                              : transport->SendBytes(&frame, sizeof(frame), endpoint_fd);
}

InputTransportStatus SendInputTransportFocus(InputTransport* transport,
                                              uint64_t epoch, bool focused) {
  if (epoch == 0) return InputTransportStatus::kTerminal;
  const transport_wire::FocusControlFrame frame =
      transport_wire::EncodeFocusControl(FocusControl{epoch, focused});
  return transport == nullptr ? InputTransportStatus::kTerminal
                              : transport->SendBytes(&frame, sizeof(frame), -1);
}

InputTransportStatus SendInputTransportFocusOnFd(InputTransport* transport,
                                                  int endpoint_fd,
                                                  uint64_t epoch, bool focused) {
  if (epoch == 0) return InputTransportStatus::kTerminal;
  const transport_wire::FocusControlFrame frame =
      transport_wire::EncodeFocusControl(FocusControl{epoch, focused});
  return transport == nullptr ? InputTransportStatus::kTerminal
                              : transport->SendBytes(&frame, sizeof(frame),
                                                     endpoint_fd);
}

InputTransportStatus InputTransport::PumpInternal(
    int fd, const InputTransportPumpCallbacks& callbacks) {
  if (impl_ == nullptr || impl_->rx_reader == nullptr)
    return InputTransportStatus::kTerminal;
  FrameInputReaderRecord record;
  struct ReaderPublication final {
    std::shared_ptr<InputResourceProgressSource> source;
    const FrameInputReaderRecord& record;
    ~ReaderPublication() {
      // This runs after FrameInputReader's admission guard has unwound. The
      // aggregate, not the reader, owns progress publication and callback
      // lifetime.
      if (record.buffered) source->Notify(InputResourceProgressKind::kRxBuffered);
      if (record.consumed) source->Notify(InputResourceProgressKind::kRxConsumed);
      if (record.terminal_changed)
        source->Notify(InputResourceProgressKind::kRxTerminal);
    }
  } reader_publication{impl_->progress, record};
  return impl_->rx_reader->Pump(fd, impl_->remote_endpoint_fd, impl_->io,
                               callbacks, &record);
}

}  // namespace darwin_art::input
