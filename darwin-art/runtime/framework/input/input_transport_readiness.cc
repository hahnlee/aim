#include "input_transport_readiness.h"

namespace darwin_art::input {
namespace {

constexpr int kHangupEvent = 0x0008;
constexpr int kErrorEvent = 0x0004;
constexpr int kInvalidEvent = 0x0010;

}  // namespace

InputTransportReadinessResult ServiceInputTransportReadiness(
    const InputTransportReadiness& readiness, int fd, int events,
    bool read_authorized, bool reader_completed) {
  InputTransportReadinessResult result;
  // Keep the strong resource pin local for the complete service operation,
  // including reentrant callbacks.  A missing resource cannot be serviced.
  const auto transport = readiness.transport;
  if (transport == nullptr) {
    result.retire = true;
    return result;
  }

  // OUTPUT-only terminal readiness is allowed to revoke TX, but a completed
  // reader deliberately ignores HUP.  ERROR/INVALID remain fatal in either
  // state; the registration owner observes those events after this helper
  // commits any requested completion/progress.
  if (!read_authorized) {
    const int terminal_events =
        reader_completed ? (kErrorEvent | kInvalidEvent)
                         : (kHangupEvent | kErrorEvent | kInvalidEvent);
    if ((events & terminal_events) != 0)
      TerminateInputTransportTx(transport.get());
  }

  // OUTPUT-only terminal admission precedes flush, so its status remains
  // terminal even with an empty TX queue. Dispatch follows this flush.
  const InputTransportStatus flush = FlushInputTransport(transport.get());

  if (readiness.reader_callback != nullptr && !reader_completed &&
      read_authorized) {
    const auto reader_result = readiness.reader_callback(
        fd, events, readiness.reader_context);
    switch (reader_result) {
      case InputTransportReaderResult::kKeepReading:
        break;
      case InputTransportReaderResult::kReaderComplete:
        result.reader_complete_requested = true;
        break;
      case InputTransportReaderResult::kRetireRegistration:
        result.retire = true;
        break;
      default:
        // Treat an ABI-corrupt/unknown reader result as a failed registration;
        // never keep a callback whose ownership result is not understood.
        result.retire = true;
        break;
    }
  } else if (readiness.reader_callback != nullptr && !reader_completed) {
    // A typed reader is an admitted INPUT owner, not an OUTPUT callback.  A
    // stale/malformed snapshot must fail closed rather than invoke it without
    // the authority that the callback contract requires.
    result.retire = true;
  } else if (readiness.callback != nullptr) {
    // Preserve the legacy callback's historical contract: it can service a
    // read-authorized event, or a healthy flush when OUTPUT is the only
    // admitted direction.  A terminal flush suppresses the latter callback.
    if (read_authorized || flush != InputTransportStatus::kTerminal) {
      if (readiness.callback(fd, events, readiness.callback_context) == 0)
        result.retire = true;
    } else {
      result.retire = true;
    }
  } else if (readiness.reader_callback == nullptr) {
    // The default reader is deliberately policy-free.  Its terminal result
    // only asks the registration owner to commit exact RX completion.
    const InputTransportStatus status =
        read_authorized ? PumpInputTransport(transport.get(), fd,
                                             readiness.callbacks)
                        : flush;
    result.default_progress = status;
    if (read_authorized && status == InputTransportStatus::kTerminal &&
        fd == transport->RemoteEndpointFd() && transport->IsRxTerminal()) {
      result.reader_complete_requested = true;
    }
  }

  return result;
}

}  // namespace darwin_art::input
