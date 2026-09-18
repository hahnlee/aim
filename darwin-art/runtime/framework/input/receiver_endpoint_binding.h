#pragma once

#include "claimed_input_transport_pump.h"
#include "receiver_registry.h"

#include <cstdint>
#include <memory>
#include <utility>

namespace darwin_art::input {

enum class ReceiverEndpointBindingSlot : std::uint8_t {
  kLocalWake,
  kRemote,
};

enum class ReceiverEndpointBindingEventResult : std::uint8_t {
  kKeep,
  kTerminal,
  // RX ended, but this slot still owns its pump while retained TX settles.
  kReaderComplete,
};

struct ReceiverEndpointBindingStatus {
  bool local_ready = false;
  bool remote_ready = false;
  bool closed = false;
  std::uint64_t revision = 0;
};

struct ReceiverEndpointBindingCallbacks {
  ReceiverEndpointBindingEventResult (*on_event)(
      void*, ReceiverEndpointBindingSlot, int, int) = nullptr;
  void (*on_status)(void*, ReceiverEndpointBindingStatus) = nullptr;
  void* context = nullptr;
  // The owner is deliberately caller-supplied and independent of the
  // receiver/binding. It keeps an immutable callback token alive across
  // deferred provider callbacks.
  std::shared_ptr<void> context_owner;
  // Called outside the binding mutex when a user callback throws. The
  // callback is a narrow failure boundary and must not throw across a looper
  // or JNI provider boundary.
  void (*on_failure)(void*) noexcept = nullptr;
  // Called exactly once after a closed binding has no retained pump
  // reservations and every slot's pump is quiescent.
  void (*on_retired)(void*) noexcept = nullptr;
};

// Owns the two endpoint registrations for one receiver epoch. The endpoint
// transport is retained immutably by each pump lease; receiver/binding objects
// are never used as callback context.
class ReceiverEndpointBinding final {
 public:
  struct Control;
  struct SlotContext;

  // Resource-only lease for asynchronous retirement. It never retains the
  // containing receiver/channel. Do not put it in this binding's own strong
  // callback context; coordinators must use a weak notification token there.
  class RetirementHandle final {
   public:
    RetirementHandle() = default;
    explicit operator bool() const { return control_ != nullptr; }
    bool Retire();
    bool IsQuiescent() const;
    // Weak completion hint for independent retirement coordinators. The
    // callback runs outside the binding mutex and may be invoked immediately.
    bool SetQuiescenceNotification(void (*notify)(void*) noexcept,
                                   std::weak_ptr<void> context);

   private:
    friend class ReceiverEndpointBinding;
    explicit RetirementHandle(std::shared_ptr<Control> control)
        : control_(std::move(control)) {}
    std::shared_ptr<Control> control_;
  };

  ReceiverEndpointBinding();
  ~ReceiverEndpointBinding();
  ReceiverEndpointBinding(const ReceiverEndpointBinding&) = delete;
  ReceiverEndpointBinding& operator=(const ReceiverEndpointBinding&) = delete;

  bool Register(void* looper, std::shared_ptr<InputTransport> transport,
                int local_fd, int remote_fd,
                const ReceiverEndpointBindingCallbacks& callbacks,
                ReceiverId receiver_id, std::uint64_t generation);
  // Prepare immutable slot ownership before receiver visibility. No FD is
  // registered and no readiness is published until Activate succeeds.
  bool Prepare(void* looper, std::shared_ptr<InputTransport> transport,
               int local_fd, int remote_fd,
               const ReceiverEndpointBindingCallbacks& callbacks,
               ReceiverId receiver_id, std::uint64_t generation);
  bool Activate();
  bool RefreshWritable(ReceiverEndpointBindingSlot slot, int fd, bool enabled);
  bool Retire();
  RetirementHandle RetainRetirement() const;
  ReceiverEndpointBindingStatus Status() const;

 private:
  static bool RetireControl(std::shared_ptr<Control> control);
  static void FinishActivationClose(
      const std::shared_ptr<Control>& control,
      const std::shared_ptr<ClaimedInputTransportPump>& local_lease,
      const std::shared_ptr<ClaimedInputTransportPump>& remote_lease,
      bool retire_local, bool retire_remote);
  static InputTransportReaderResult OnSlotEvent(int fd, int events, void* data);
  static void OnSlotState(void* data, ClaimedPumpState state) noexcept;
  static void OnSlotTerminal(void* data) noexcept;
  static void OnSlotQuiescent(void* data) noexcept;
  static void Notify(const std::shared_ptr<Control>& control);
  static void MaybeNotifyRetired(const std::shared_ptr<Control>& control) noexcept;
  std::shared_ptr<Control> control_;
};

}  // namespace darwin_art::input
