#pragma once

#include "input_transport_pump.h"
#include "transport_registration_authority.h"

#include <cstdint>
#include <memory>

namespace darwin_art::input {

enum class ClaimedPumpState : std::uint8_t {
  kPrepared,
  kWaiting,
  kActive,
  kRetiring,
  kSettled,
};

struct ClaimedPumpObserver {
  void (*on_state)(void*, ClaimedPumpState) noexcept = nullptr;
  void* context = nullptr;
  std::shared_ptr<void> context_owner;
};

// Couples a registration-authority Claim, its exact looper pump, and the
// owner-thread service task.  The process retention list keeps the Control
// alive after callers drop the wrapper, while every asynchronous callback
// reaches it through a weak token.
class ClaimedInputTransportPump final {
 public:
  static std::shared_ptr<ClaimedInputTransportPump> Prepare(
      void* looper, std::shared_ptr<InputTransport> transport, int fd,
      int events, TransportRegistrationRole role, std::uint64_t identity,
      const InputTransportPumpCallbacks& callbacks,
      darwin_art::looper::FdCallback fd_callback = nullptr,
      void* fd_context = nullptr, std::shared_ptr<void> fd_owner = {},
      const ClaimedPumpObserver& observer = {});
  static std::shared_ptr<ClaimedInputTransportPump> PrepareReader(
      void* looper, std::shared_ptr<InputTransport> transport, int fd,
      int events, TransportRegistrationRole role, std::uint64_t identity,
      const InputTransportPumpCallbacks& callbacks,
      InputTransportReaderCallback reader_callback, void* reader_context,
      std::shared_ptr<void> reader_owner = {},
      const ClaimedPumpObserver& observer = {});

  ~ClaimedInputTransportPump();
  ClaimedInputTransportPump(const ClaimedInputTransportPump&) = delete;
  ClaimedInputTransportPump& operator=(const ClaimedInputTransportPump&) = delete;

  // Activate is an admission to the owner-thread service loop.  A deferred
  // authority claim is accepted and remains kWaiting until a fresh hint is
  // observed and BeginRegistration succeeds.
  bool Activate();
  ClaimedPumpState State() const;
  bool Failed() const;
  InputTransportWritableResult SetWritableResult(bool enabled);
  bool SetWritable(bool enabled);
  bool Retire();
  bool IsQuiescent() const;

 public:
  // Opaque to consumers; public only so the implementation can keep the
  // process-retention links and callback tokens out of the public contract.
  struct Control;

 public:
  explicit ClaimedInputTransportPump(std::shared_ptr<Control> control)
      : control_(std::move(control)) {}

 private:
  static std::shared_ptr<ClaimedInputTransportPump> PrepareInternal(
      void* looper, std::shared_ptr<InputTransport> transport, int fd,
      int events, TransportRegistrationRole role, std::uint64_t identity,
      const InputTransportPumpCallbacks& callbacks,
      InputTransportReaderCallback reader_callback, void* reader_context,
      std::shared_ptr<void> reader_owner,
      darwin_art::looper::FdCallback fd_callback, void* fd_context,
      std::shared_ptr<void> fd_owner, const ClaimedPumpObserver& observer);
  std::shared_ptr<Control> control_;
};

using ClaimedInputTransportPumpHandle =
    std::shared_ptr<ClaimedInputTransportPump>;

}  // namespace darwin_art::input
