#pragma once

#include "input_transport.h"
#include "../../../compat/looper/android_looper_owner.h"

#include <cstdint>

namespace darwin_art::input {

// A focus callback may defer before invoking its policy owner (the complete
// frame remains at the FIFO head), or consume and stop after invoking it (for
// example, when the owner has a pending exception). Transport does not inspect
// or otherwise interpret the reason for either result.
enum class InputTransportConsumptionResult : std::uint8_t {
  kConsumed,
  kDeferred,
  kConsumedStop,
};
using FocusControlCallbackResult = InputTransportConsumptionResult;

struct InputTransportPumpCallbacks {
  // Returning false leaves the complete packet at the front of rx and
  // reports kBackpressured; policy can retry after draining its queue.
  bool (*on_packet)(void*, const DarwinArtInputPacket&) = nullptr;
  // Window frame, visibility and InputWindowFlags from the WMS publication.
  void (*on_window)(void*, int32_t, int32_t, int32_t, int32_t, bool, uint32_t) = nullptr;
  void (*on_ack)(void*, uint32_t, bool) = nullptr;
  // kDeferred leaves the complete focus frame at the front of rx and reports
  // kBackpressured. kConsumedStop consumes it and returns kAccepted without
  // parsing subsequent RX bytes. The callback is noexcept so pending-
  // exception handling stays at its owner.
  FocusControlCallbackResult (*on_focus)(void*, uint64_t, bool) noexcept =
      nullptr;
  // Called outside transport/looper locks after flushing and draining. Policy
  // may enqueue ordered retry work; terminal is explicit, never acceptance.
  void (*on_progress)(void*, InputTransportStatus) = nullptr;
  void* context = nullptr;
  std::shared_ptr<void> context_owner;
  // Resource termination (including automatic rearm failure), not a normal
  // input event. Runs once outside pump locks and must not enter JNI or throw.
  void (*on_terminal)(void*) noexcept = nullptr;
  // Once-only completion metadata after closed registration, exact removal,
  // operations and all admitted callback work have ended. Outside pump locks;
  // must not touch the old FD or enter JNI. Context ownership remains pinned.
  void (*on_quiescent)(void*) noexcept = nullptr;
  // Explicit ordered-consumption ports. Deferred keeps the exact FIFO head;
  // ConsumedStop erases that item but retains the suffix, including after EOF.
  // Consumption means owner invocation/rejection, never eventual Java finish.
  // Do not configure either port together with its legacy counterpart.
  InputTransportConsumptionResult (*on_packet_consumption)(
      void*, const DarwinArtInputPacket&) = nullptr;
  InputTransportConsumptionResult (*on_window_consumption)(
      void*, int32_t, int32_t, int32_t, int32_t, bool, uint32_t) = nullptr;
  // Original packet identity, ACK v2 only. Legacy v1 stays on on_ack.
  void (*on_ack64)(void*, uint64_t, bool) = nullptr;
};

enum class InputTransportWritableResult : std::uint8_t {
  kApplied,
  kDeferred,
  kTerminal,
};

enum class InputTransportReaderResult : std::uint8_t {
  kKeepReading,
  // Only the authoritative remote framed reader may complete RX. This
  // removes INPUT, retaining private writable service for matching buffered
  // TX; it does not establish settlement of future policy/JNI ACK obligations.
  kReaderComplete,
  kRetireRegistration,
};

using InputTransportReaderCallback = InputTransportReaderResult (*)(
    int fd, int events, void* context);

// Drains one readable endpoint and decodes complete frames in order. Partial
// frames remain buffered; malformed/overflowed streams become terminal.
InputTransportStatus PumpInputTransport(
    InputTransport* transport, int fd,
    const InputTransportPumpCallbacks& callbacks);

// Owns the looper registration and retains a strong transport lease. Callback
// context is borrowed unless context_owner is supplied; server-only endpoints
// need no JNI or receiver policy.
// Register is one-shot and must finish construction before asynchronous use.
// Initial non-OUTPUT interests are preserved across writable refreshes. A
// default OUTPUT-only lease never reads RX; terminal FD readiness marks the
// transport terminal and retires the registration even with no pending TX.
// Retire preserves its stable control while releasing quiescent resources.
class InputTransportPumpLease {
 public:
  InputTransportPumpLease() = default;
  ~InputTransportPumpLease();
  InputTransportPumpLease(const InputTransportPumpLease&) = delete;
  InputTransportPumpLease& operator=(const InputTransportPumpLease&) = delete;
  bool Register(void* looper, std::shared_ptr<InputTransport> transport, int fd,
                int events, const InputTransportPumpCallbacks& callbacks,
                darwin_art::looper::FdCallback callback = nullptr,
                void* callback_context = nullptr,
                std::shared_ptr<void> callback_owner = {});
  // Named typed port, requiring initial INPUT authority and a non-null reader.
  // Legacy Register's integer zero always means unconditional retirement.
  // After completion this reader is never called again; terminal/quiescent
  // callbacks describe registration retirement, not mere RX completion.
  bool RegisterReader(
      void* looper, std::shared_ptr<InputTransport> transport, int fd,
      int events, const InputTransportPumpCallbacks& callbacks,
      InputTransportReaderCallback reader_callback, void* reader_context,
      std::shared_ptr<void> reader_owner = {});
  bool SetWritable(bool enabled);
  // Typed form for policy owners that must distinguish a coalesced refresh
  // from a provider failure that permanently retired this registration.
  InputTransportWritableResult SetWritableResult(bool enabled);
  bool Retire();
  // Retirement acceptance is not quiescence: deferred callbacks/rearms or a
  // failed exact removal still own this FD. Used before an output-only handoff.
  // Includes progress/terminal callback work, excludes on_quiescent metadata.
  bool IsQuiescent() const;

  // Callback entry points are public only as opaque provider function
  // pointers; callers must use Register/SetWritable/Retire for ownership.
  static int OnFd(int fd, int events, void* data);
  static void Release(void* data);

 private:
  bool RegisterInternal(
      void* looper, std::shared_ptr<InputTransport> transport, int fd,
      int events, const InputTransportPumpCallbacks& callbacks,
      InputTransportReaderCallback reader_callback, void* reader_context,
      std::shared_ptr<void> reader_owner,
      darwin_art::looper::FdCallback callback, void* callback_context,
      std::shared_ptr<void> callback_owner);

 public:
  struct Control;
  struct Registration;

 private:
  std::shared_ptr<Control> control_;
};

}  // namespace darwin_art::input
