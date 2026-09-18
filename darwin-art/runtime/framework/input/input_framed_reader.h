#pragma once

#include "input_transport_pump.h"

#include <atomic>
#include <cstdint>
#include <vector>

namespace darwin_art::input {

struct FrameInputReaderRecord final {
  bool buffered = false;
  bool consumed = false;
  bool terminal_changed = false;
};

// Owns only the framed RX stream and its admission/terminal state. It has no
// transport descriptors, JNI, Looper registration, routing, or progress
// publication authority.
class FrameInputReader final {
 public:
  FrameInputReader() = default;
  ~FrameInputReader() = default;

  FrameInputReader(const FrameInputReader&) = delete;
  FrameInputReader& operator=(const FrameInputReader&) = delete;

  // Pump drains fd into the bounded RX FIFO and dispatches complete frames in
  // FIFO order. The record is reset and updated before each callback, so its
  // state remains observable if a callback throws. remote_endpoint_fd is the
  // only endpoint that carries framed RX; other endpoints are drained and
  // accepted without parsing, preserving the transport's existing behavior.
  InputTransportStatus Pump(int fd, int remote_endpoint_fd,
                            const InputTransportIo& io,
                            const InputTransportPumpCallbacks& callbacks,
                            FrameInputReaderRecord* record);

  bool IsTerminal() const noexcept {
    return terminal_.load(std::memory_order_acquire);
  }

  // Returns true only for the first terminal transition. No callback or
  // progress publication is performed here; the transport aggregate owns
  // that publication after admission unwinds.
  bool Terminate() noexcept;

 private:
  static constexpr size_t kMaxRxBytes = 4096u * 512u;

  std::atomic<bool> terminal_{false};
  std::atomic_flag admitted_ = ATOMIC_FLAG_INIT;
  bool eof_ = false;  // Protected by admitted_; pending FIFO survives EOF.
  std::vector<uint8_t> rx_;
};

}  // namespace darwin_art::input
