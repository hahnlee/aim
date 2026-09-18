#pragma once

#include "input_transport.h"

#include <cstdint>
#include <memory>

namespace darwin_art::input {

// Result of an operation on a retired endpoint. Provider failures are kept
// distinct from transport terminality so the owner can retry exact removal or
// registration without fabricating a successful drain.
enum class RetiredTransportDrainResult : std::uint8_t {
  kApplied,
  kDeferred,
  kTerminal,
  kOutOfMemory,
  kProviderFailure,
};

struct RetiredTransportDrainCallbacks {
  // Runs after FlushInputTransport and outside the drain mutex. This callback
  // may request another drain or retire the owner; it must not retain the
  // drain object through a receiver/channel/JNI cycle.
  void (*on_progress)(void*, InputTransportStatus) = nullptr;
  void* context = nullptr;
  std::shared_ptr<void> context_owner;
  // Reports a failure while an already-retained request is being resumed by
  // the owner looper. The request remains pending and may be retried by the
  // owning policy; this callback is never invoked under the drain mutex.
  void (*on_failure)(void*, RetiredTransportDrainResult) = nullptr;
};

struct RetiredTransportDrainStatus {
  bool prepared = false;
  bool active = false;
  bool pending = false;
  bool closed = false;
  bool terminal = false;
  std::uint64_t revision = 0;
};

// Owns only an immutable transport/looper lease and an OUTPUT registration
// used to settle bytes already accepted by an old owner. It never reads the
// transport and has no receiver, channel, JNI, focus, or routing state.
class RetiredTransportDrain final {
 public:
  RetiredTransportDrain();
  ~RetiredTransportDrain();
  RetiredTransportDrain(const RetiredTransportDrain&) = delete;
  RetiredTransportDrain& operator=(const RetiredTransportDrain&) = delete;

  RetiredTransportDrainResult Prepare(
      void* looper, std::shared_ptr<InputTransport> transport, int fd,
      const RetiredTransportDrainCallbacks& callbacks);
  RetiredTransportDrainResult Activate();
  RetiredTransportDrainResult Request();
  RetiredTransportDrainResult Retire();
  RetiredTransportDrainStatus Status() const;

 private:
  struct Control;
  struct Registration;
  struct Continuation;

  static int OnFd(int fd, int events, void* data);
  static void Release(void* data);
  static bool NotifyProgress(const std::shared_ptr<Control>&,
                             InputTransportStatus);
  static bool DeliverPendingTerminal(const std::shared_ptr<Control>&);
  static RetiredTransportDrainResult DrainOnce(
      const std::shared_ptr<Control>&, Registration* registration,
      bool from_callback);
  static RetiredTransportDrainResult ActivateControl(
      const std::shared_ptr<Control>&);
  static RetiredTransportDrainResult RequestControl(
      const std::shared_ptr<Control>&);
  static RetiredTransportDrainResult RemoveRegistration(
      const std::shared_ptr<Control>&, Registration* registration);
  static void ScheduleContinuation(const std::shared_ptr<Control>&);
  static void RunContinuation(void* data);
  static void ReleaseContinuation(void* data);
  static void NotifyFailure(const std::shared_ptr<Control>&,
                            RetiredTransportDrainResult,
                            bool include_closed = false);

  std::shared_ptr<Control> control_;
};

}  // namespace darwin_art::input
