#include "channel_endpoint.h"

#include <new>
#include <utility>

extern "C" intptr_t darwin_art_bionic_socket_broker_send(int, const void*, size_t, int);
extern "C" intptr_t darwin_art_bionic_socket_broker_recv(int, void*, size_t, int);
extern "C" int darwin_art_bionic_socket_broker_close(int);

namespace darwin_art::input {
namespace {
struct OwnedEndpointFd {
  int fd;
  ~OwnedEndpointFd() {
    if (fd >= 0) (void)darwin_art_bionic_socket_broker_close(fd);
  }
};
}
struct ChannelEndpoint::State {
  explicit State(std::shared_ptr<InputTransport> value) : transport(std::move(value)) {}
  const std::shared_ptr<InputTransport> transport;
  FinishLedger finishes;
};

ChannelEndpoint::ChannelEndpoint(std::shared_ptr<InputTransport> transport)
    : state_(std::make_shared<State>(std::move(transport))) {}
ChannelEndpoint::~ChannelEndpoint() = default;
std::shared_ptr<ChannelEndpoint> ChannelEndpoint::CreateLocal(
    ChannelEndpointCreationError* error) {
  if (error != nullptr) *error = ChannelEndpointCreationError::kNone;
  try {
    auto transport = std::make_shared<InputTransport>();
    if (!OpenLocalInputTransport(transport.get())) {
      if (error != nullptr) *error = ChannelEndpointCreationError::kTransportUnavailable;
      return {};
    }
    return std::make_shared<ChannelEndpoint>(std::move(transport));
  } catch (const std::bad_alloc&) {
    if (error != nullptr) *error = ChannelEndpointCreationError::kOutOfMemory;
    return {};
  }
}
std::shared_ptr<ChannelEndpoint> ChannelEndpoint::AdoptRemote(
    int owned_fd, ChannelEndpointCreationError* error) {
  if (error != nullptr) *error = ChannelEndpointCreationError::kNone;
  OwnedEndpointFd guard{owned_fd};
  if (owned_fd < 0) {
    if (error != nullptr) *error = ChannelEndpointCreationError::kInvalidDescriptor;
    return {};
  }
  try {
    auto transport = std::make_shared<InputTransport>();
    if (!AdoptRemoteInputTransport(transport.get(), owned_fd)) {
      if (error != nullptr) *error = ChannelEndpointCreationError::kInvalidDescriptor;
      return {};
    }
    guard.fd = -1;
    if (!OpenLocalInputTransport(transport.get())) {
      if (error != nullptr) *error = ChannelEndpointCreationError::kTransportUnavailable;
      return {};
    }
    return std::make_shared<ChannelEndpoint>(std::move(transport));
  } catch (const std::bad_alloc&) {
    if (error != nullptr) *error = ChannelEndpointCreationError::kOutOfMemory;
    return {};
  }
}
std::shared_ptr<InputTransport> ChannelEndpoint::Transport() const {
  return state_->transport;
}
bool ChannelEndpoint::TryRegisterFinish(uint32_t sequence) {
  return state_->finishes.TryRegister(sequence);
}
bool ChannelEndpoint::CancelFinish(uint32_t sequence) {
  return state_->finishes.Cancel(sequence);
}
bool ChannelEndpoint::CloseFinishObservation(uint32_t sequence, bool* handled) {
  return state_->finishes.CloseObservation(sequence, handled);
}
bool ChannelEndpoint::MarkFinishAckAccepted(uint32_t sequence) {
  return state_->finishes.MarkAckAccepted(sequence);
}
void ChannelEndpoint::RegisterFinish(uint32_t sequence) {
  (void)TryRegisterFinish(sequence);
}
bool ChannelEndpoint::RecordFinish(uint32_t sequence, bool handled) {
  return state_->finishes.Record(sequence, handled);
}
bool ChannelEndpoint::TakeFinish(uint32_t sequence, bool* handled) {
  return state_->finishes.Take(sequence, handled);
}
uint64_t ChannelEndpoint::OverflowCount() const {
  return state_->finishes.OverflowCount();
}
InputTransportStatus ChannelEndpoint::SendAck(uint32_t sequence, bool handled) {
  const auto transport = Transport();
  return transport == nullptr ? InputTransportStatus::kTerminal
      : SendInputTransportAck(transport.get(), sequence, handled);
}
bool ChannelEndpoint::WakeLocal() {
  const auto transport = Transport();
  if (transport == nullptr || transport->WriteFd() < 0) return false;
  const uint8_t token = 1;
  return darwin_art_bionic_socket_broker_send(transport->WriteFd(), &token, 1,
                                            0x40 | 0x4000) == 1;
}
void ChannelEndpoint::DrainLocalWake() {
  const auto transport = Transport();
  if (transport == nullptr || transport->ReadFd() < 0) return;
  uint8_t bytes[64];
  while (darwin_art_bionic_socket_broker_recv(transport->ReadFd(), bytes,
                                            sizeof(bytes), 0x40) > 0) {}
}
int ChannelEndpoint::BorrowParcelFd(bool server_side) const {
  const auto transport = Transport();
  if (transport == nullptr) return -1;
  if (transport->RemoteEndpointFd() >= 0) return transport->RemoteEndpointFd();
  return server_side ? transport->ReadFd() : transport->WriteFd();
}
InputTransportStatus ChannelEndpoint::PublishWindow(
    int32_t left, int32_t top, int32_t right, int32_t bottom, bool visible,
    uint32_t input_flags) {
  const auto transport = Transport();
  return transport == nullptr ? InputTransportStatus::kTerminal
      : SendInputTransportWindowOnFd(transport.get(), transport->ReadFd(),
                                     left, top, right, bottom, visible,
                                     input_flags);
}
InputTransportStatus ChannelEndpoint::PublishFocus(uint64_t epoch,
                                                   bool focused) {
  const auto transport = Transport();
  return transport == nullptr ? InputTransportStatus::kTerminal
      : SendInputTransportFocusOnFd(transport.get(), transport->ReadFd(),
                                     epoch, focused);
}
}  // namespace darwin_art::input
