#pragma once

#include "input_routing.h"
#include "input_transport.h"

#include <cstdint>
#include <memory>

namespace darwin_art::input {

struct PendingReceiverRetirement;
enum class ClaimedPumpState : std::uint8_t;
using PendingReceiverRetirementHandle =
    std::shared_ptr<PendingReceiverRetirement>;

// Optional owner-thread hooks. They deliberately expose only the two
// resource effects a retired receiver may need: refresh a live writable
// registration and wake the current local delivery endpoint. The driver does
// not retain, inspect or call through receiver/channel/JNI objects.
struct ReceiverRetirementDriverHooks {
  bool (*refresh_writable)(void*, const InputRoutingHandle&) noexcept = nullptr;
  bool (*wake_local)(void*) noexcept = nullptr;
  void* context = nullptr;
  std::shared_ptr<void> context_owner;
  std::weak_ptr<void> context_token;
};

class ReceiverRetirementDriver final {
 public:
  // Prepare all task, notification, transport-progress and OUTPUT orphan
  // ownership before the pending record is enlisted or exposed. The record is
  // weak here; the retention list remains the independent lifetime owner.
  static std::shared_ptr<ReceiverRetirementDriver> Prepare(
      PendingReceiverRetirementHandle record, void* looper,
      std::shared_ptr<InputTransport> original_transport, int original_fd,
      ReceiverId registry_id, InputRoutingHandle routing,
      ReceiverRetirementDriverHooks hooks = {});

  ~ReceiverRetirementDriver();
  ReceiverRetirementDriver(const ReceiverRetirementDriver&) = delete;
  ReceiverRetirementDriver& operator=(const ReceiverRetirementDriver&) = delete;

  // Requests are allocation-free after Prepare and may be called from any
  // thread. Work always runs on the original looper owner thread.
  bool Request() noexcept;
  bool Close() noexcept;
  bool IsQuiescent() const;
  bool Failed() const;

 private:
  struct Control;
  explicit ReceiverRetirementDriver(std::shared_ptr<Control> control)
      : control_(std::move(control)) {}

  static void OnTask(void* data) noexcept;
  static void OnTaskQuiescent(void* data) noexcept;
  static void OnTransportProgress(void* data,
                                  InputResourceProgress progress) noexcept;
  static void OnRoutingNotification(
      void* data, const InputRoutingNotification& notification) noexcept;
  static void OnPumpTerminal(void* data) noexcept;
  static void OnPumpQuiescent(void* data) noexcept;
  static void OnPumpState(void* data, ClaimedPumpState state) noexcept;
  static void OnResourceQuiescence(void* data) noexcept;
  static bool RequestControl(const std::shared_ptr<Control>& control) noexcept;
  static void RequestAfterHint(const std::shared_ptr<Control>& control) noexcept;
  static void RequestBoundedRetry(
      const std::shared_ptr<Control>& control) noexcept;
  static void Drive(const std::shared_ptr<Control>& control) noexcept;
  static void TrySettle(const std::shared_ptr<Control>& control) noexcept;

  std::shared_ptr<Control> control_;
};

using ReceiverRetirementDriverHandle =
    std::shared_ptr<ReceiverRetirementDriver>;

}  // namespace darwin_art::input
