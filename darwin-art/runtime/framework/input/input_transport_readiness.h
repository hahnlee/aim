#pragma once

#include "input_transport_pump.h"

#include <memory>
#include <optional>

namespace darwin_art::input {

// A value snapshot for one admitted readiness callback.  The caller owns the
// registration and supplies these pins while it commits reader completion,
// publishes progress, and decides whether the registration can retire.  This
// helper does not acquire or release a looper registration lifetime.
struct InputTransportReadiness final {
  std::shared_ptr<InputTransport> transport;
  InputTransportPumpCallbacks callbacks;

  InputTransportReaderCallback reader_callback = nullptr;
  void* reader_context = nullptr;
  std::shared_ptr<void> reader_owner;

  darwin_art::looper::FdCallback callback = nullptr;
  void* callback_context = nullptr;
  std::shared_ptr<void> callback_owner;
};

struct InputTransportReadinessResult final {
  bool reader_complete_requested = false;
  bool retire = false;
  std::optional<InputTransportStatus> default_progress;
};

// Services one already-admitted readiness event.  The caller must retain the
// resource/admission pins through any exact completion commit, the returned
// default progress callback, and the subsequent fresh TX retirement check.
// Completion is only a request here; this function never invokes on_progress,
// changes looper interests, or decides empty-TX retirement.  Exceptions from
// transport or policy callbacks intentionally propagate to the owner.
InputTransportReadinessResult ServiceInputTransportReadiness(
    const InputTransportReadiness& readiness, int fd, int events,
    bool read_authorized, bool reader_completed);

}  // namespace darwin_art::input
