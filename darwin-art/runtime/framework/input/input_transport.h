#pragma once

#include <cstdint>
#include <memory>

#include "input_transport_wire.h"
#include "transport_registration_authority.h"
#include "input_resource_progress.h"

namespace darwin_art::input {

enum class InputTransportStatus : uint8_t {
  kAccepted,
  kBackpressured,
  kTerminal,
};

using TransportSend = intptr_t (*)(int, const void*, size_t, int);
using TransportReceive = intptr_t (*)(int, void*, size_t, int);
using TransportClose = int (*)(int);
using TransportError = int (*)();

struct InputTransportIo {
  TransportSend send = nullptr;
  TransportReceive receive = nullptr;
  TransportClose close = nullptr;
  TransportError error = nullptr;
};

struct InputTransportPumpCallbacks;
class InputTransport;
struct InputTransportOutputSnapshot final {
  int endpoint_fd = -1;
  bool pending = false;
  bool terminal = false;
};
enum class InputTransportTxFenceStatus : uint8_t {
  kPending,
  kFlushed,
  kTerminal,
  kInvalid,
};

// An accepted stream prefix, not a requirement that successor traffic stop.
// Identity retains only a token, never descriptors or the transport itself.
class InputTransportTxFence final {
 public:
  InputTransportTxFence() = default;
 private:
  friend class InputTransport;
  std::shared_ptr<const void> identity_;
  uint64_t accepted_bytes_ = 0;
};
InputTransportStatus PumpInputTransport(InputTransport*, int,
                                        const InputTransportPumpCallbacks&);

// Owns guest descriptors and the bounded byte stream used by an imported
// InputChannel endpoint. It has no JNI, focus, receiver, or framework policy.
class InputTransport {
 public:
  explicit InputTransport(InputTransportIo io = {}, bool owns_descriptors = true);
  ~InputTransport();
  InputTransport(const InputTransport&) = delete;
  InputTransport& operator=(const InputTransport&) = delete;

  int ReadFd() const;
  int WriteFd() const;
  int RemoteEndpointFd() const;
  // Both byte-stream directions failed/ended. Not disposal or quiescence proof.
  bool IsTerminal() const;
  bool IsTxTerminal() const;
  bool IsRxTerminal() const;
  bool HasPendingTx() const;
  // One immutable framed output stream; wake tokens bypass this stream.
  // A borrowed FD's owner must outlive accepted TX and pump quiescence.
  bool BindOutputEndpoint(int fd);
  InputTransportOutputSnapshot OutputSnapshot() const;
  InputResourceProgressSource::SubscriptionHandle SubscribeProgress(
      void (*notify)(void*, InputResourceProgress) noexcept,
      std::weak_ptr<void> context);
  // Returns the authority attached to this transport's shared Impl. This is a
  // non-owning, no-allocation view; callers must not retain it past transport
  // destruction and must use it for every registration of this resource.
  TransportRegistrationAuthority& RegistrationAuthority();
  const TransportRegistrationAuthority& RegistrationAuthority() const;
  InputTransportTxFence CaptureAcceptedTxFence() const;
  InputTransportTxFenceStatus QueryTxFence(
      const InputTransportTxFence& fence) const;

 private:
  struct Impl;
  InputTransportStatus FlushBytes();
  InputTransportStatus SendBytes(const void*, size_t, int);
  InputTransportStatus PumpInternal(int, const InputTransportPumpCallbacks&);
  int LastError() const;
  std::unique_ptr<Impl> impl_;
  friend bool OpenLocalInputTransport(InputTransport*);
  friend bool AdoptRemoteInputTransport(InputTransport*, int);
  friend InputTransportStatus FlushInputTransport(InputTransport*);
  friend void TerminateInputTransport(InputTransport*);
  friend void TerminateInputTransportTx(InputTransport*);
  friend bool TerminateInputTransportTxAndQuiesce(InputTransport*);
  friend InputTransportStatus SendInputTransportPacket(
      InputTransport*, const DarwinArtInputPacket&);
  friend InputTransportStatus SendInputTransportAck(InputTransport*, uint32_t,
                                                    bool);
  friend InputTransportStatus SendInputTransportAck64(InputTransport*, uint64_t,
                                                      bool);
  friend InputTransportStatus SendInputTransportWindow(
      InputTransport*, int32_t, int32_t, int32_t, int32_t, bool, uint32_t);
  friend InputTransportStatus SendInputTransportWindowOnFd(
      InputTransport*, int, int32_t, int32_t, int32_t, int32_t, bool, uint32_t);
  friend InputTransportStatus SendInputTransportFocus(InputTransport*,
                                                       uint64_t, bool);
  friend InputTransportStatus SendInputTransportFocusOnFd(InputTransport*, int,
                                                           uint64_t, bool);
  friend InputTransportStatus PumpInputTransport(
      InputTransport*, int, const InputTransportPumpCallbacks&);
};

bool OpenLocalInputTransport(InputTransport* transport);
// Setup-only single assignment; rejection neither consumes nor closes fd.
bool AdoptRemoteInputTransport(InputTransport* transport, int endpoint_fd);

// Flushes bytes retained after a nonblocking partial write. A caller that
// receives kBackpressured must retry this operation from its looper rather
// than reconstructing or replaying the frame.
InputTransportStatus FlushInputTransport(InputTransport* transport);
// Explicit whole-resource abort. Does not release registrations/FD ownership.
void TerminateInputTransport(InputTransport* transport);
// OUTPUT-only readiness must not revoke another reader's accepted RX work.
void TerminateInputTransportTx(InputTransport* transport);
// Deny future TX admission, then try to join previous send/flush admission.
// False requires a later retry; true abandons pending bytes without counting
// them as flushed. Never closes RX/FDs or proves remote non-delivery.
bool TerminateInputTransportTxAndQuiesce(InputTransport* transport);

// kTerminal can mean nonretryable submission rejection (invalid/mismatched
// endpoint); such rejection does not terminate or mutate the retained stream.
InputTransportStatus SendInputTransportPacket(
    InputTransport* transport, const DarwinArtInputPacket& packet);
InputTransportStatus SendInputTransportAck(InputTransport* transport,
                                            uint32_t sequence, bool handled);
InputTransportStatus SendInputTransportAck64(InputTransport* transport,
                                              uint64_t sequence, bool handled);
InputTransportStatus SendInputTransportWindow(InputTransport* transport,
                                               int32_t left, int32_t top,
                                               int32_t right, int32_t bottom,
                                               bool visible,
                                               uint32_t input_flags = 0);
InputTransportStatus SendInputTransportWindowOnFd(InputTransport* transport,
                                                   int endpoint_fd,
                                                   int32_t left, int32_t top,
                                                   int32_t right, int32_t bottom,
                                                   bool visible,
                                                   uint32_t input_flags = 0);
InputTransportStatus SendInputTransportFocus(InputTransport* transport,
                                              uint64_t epoch, bool focused);
InputTransportStatus SendInputTransportFocusOnFd(InputTransport* transport,
                                                  int endpoint_fd,
                                                  uint64_t epoch, bool focused);

inline InputTransportStatus SendInputTransportFocus(InputTransport* transport,
                                                     FocusControl control) {
  return SendInputTransportFocus(transport, control.epoch, control.focused);
}

inline InputTransportStatus SendInputTransportFocusOnFd(
    InputTransport* transport, int endpoint_fd, FocusControl control) {
  return SendInputTransportFocusOnFd(transport, endpoint_fd, control.epoch,
                                     control.focused);
}

}  // namespace darwin_art::input
