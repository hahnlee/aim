#include "input_framed_reader.h"

#include "../../../tools/bionic-errno-tls/include/darwin_art_bionic_errno.h"

#include <cerrno>
#include <cstring>
#include <stdexcept>

namespace darwin_art::input {
namespace {

bool ValidPacket(const DarwinArtInputPacket& packet) {
  switch (packet.kind) {
    case DarwinArtInputPacketKind::kPointer:
      return packet.pointer.version == 2 &&
             packet.pointer.size >= sizeof(DarwinArtPointerEventV2) &&
             packet.pointer.action <= DARWIN_ART_POINTER_OUTSIDE &&
             packet.pointer.pointer_count > 0;
    case DarwinArtInputPacketKind::kKey:
      return packet.key.version == 1 &&
             packet.key.size >= sizeof(DarwinArtKeyEventV1) &&
             packet.key.action <= 1;
  }
  return false;
}

int LastError(const InputTransportIo& io) {
  return io.error != nullptr ? io.error() : errno;
}

InputTransportConsumptionResult ValidateConsumption(
    InputTransportConsumptionResult result) {
  switch (result) {
    case InputTransportConsumptionResult::kConsumed:
    case InputTransportConsumptionResult::kDeferred:
    case InputTransportConsumptionResult::kConsumedStop:
      return result;
  }
  throw std::invalid_argument("invalid framed input consumption result");
}

}  // namespace

bool FrameInputReader::Terminate() noexcept {
  return !terminal_.exchange(true, std::memory_order_acq_rel);
}

InputTransportStatus FrameInputReader::Pump(
    int fd, int remote_endpoint_fd, const InputTransportIo& io,
    const InputTransportPumpCallbacks& callbacks,
    FrameInputReaderRecord* record) {
  FrameInputReaderRecord ignored;
  if (record == nullptr) record = &ignored;
  *record = {};
  if ((callbacks.on_packet != nullptr && callbacks.on_packet_consumption != nullptr) ||
      (callbacks.on_window != nullptr && callbacks.on_window_consumption != nullptr)) {
    throw std::invalid_argument("multiple owners for framed input consumption");
  }
  if (fd < 0 || io.receive == nullptr ||
      (fd == remote_endpoint_fd && IsTerminal())) {
    return InputTransportStatus::kTerminal;
  }

  // RX's FIFO head remains present while policy admits its callback. Nested
  // owner-Looper entry must neither deliver that head again nor erase it from
  // under the original callback. No lock is held across policy/JNI.
  if (admitted_.test_and_set(std::memory_order_acquire))
    return InputTransportStatus::kBackpressured;
  struct Admission final {
    std::atomic_flag& flag;
    ~Admission() { flag.clear(std::memory_order_release); }
  } admission{admitted_};

  constexpr int kMsgDontWait = 0x40;
  uint8_t buffer[4096];
  if (fd != remote_endpoint_fd) {
    while (io.receive(fd, buffer, sizeof(buffer), kMsgDontWait) > 0) {
    }
    return InputTransportStatus::kAccepted;
  }

  while (!eof_) {
    const intptr_t received =
        io.receive(fd, buffer, sizeof(buffer), kMsgDontWait);
    if (received == 0) {
      eof_ = true;
      break;
    }
    if (received < 0) {
      const int error = LastError(io);
      if (error == 4) continue;
      if (error == 11) break;
      if (Terminate()) record->terminal_changed = true;
      return InputTransportStatus::kTerminal;
    }
    if (rx_.size() + static_cast<size_t>(received) > kMaxRxBytes) {
      if (Terminate()) record->terminal_changed = true;
      return InputTransportStatus::kTerminal;
    }
    rx_.insert(rx_.end(), buffer, buffer + received);
    record->buffered = true;
  }

  for (;;) {
    if (rx_.size() < sizeof(uint32_t)) break;
    uint32_t magic = 0;
    std::memcpy(&magic, rx_.data(), sizeof(magic));
    if (magic == transport_wire::kAckFrameMagic) {
      if (rx_.size() < 8) break;
      uint32_t version = 0;
      std::memcpy(&version, rx_.data() + 4, sizeof(version));
      if (version == 2) {
        if (rx_.size() < sizeof(transport_wire::AckFrameV2)) break;
        transport_wire::AckFrameV2 frame{};
        std::memcpy(&frame, rx_.data(), sizeof(frame));
        if (frame.handled > 1 || frame.reserved != 0) {
          if (Terminate()) record->terminal_changed = true;
          return InputTransportStatus::kTerminal;
        }
        rx_.erase(rx_.begin(), rx_.begin() + sizeof(frame));
        record->consumed = true;
        if (callbacks.on_ack64 != nullptr)
          callbacks.on_ack64(callbacks.context, frame.sequence, frame.handled != 0);
        continue;
      }
      if (version != transport_wire::kFrameVersion) {
        if (Terminate()) record->terminal_changed = true;
        return InputTransportStatus::kTerminal;
      }
      if (rx_.size() < sizeof(transport_wire::AckFrame)) break;
      transport_wire::AckFrame frame{};
      std::memcpy(&frame, rx_.data(), sizeof(frame));
      if (frame.version != transport_wire::kFrameVersion || frame.handled > 1) {
        if (Terminate()) record->terminal_changed = true;
        return InputTransportStatus::kTerminal;
      }
      rx_.erase(rx_.begin(), rx_.begin() + sizeof(frame));
      record->consumed = true;
      if (callbacks.on_ack != nullptr)
        callbacks.on_ack(callbacks.context, frame.sequence,
                         frame.handled != 0);
      continue;
    }
    if (magic == transport_wire::kWindowFrameMagic) {
      if (rx_.size() < sizeof(transport_wire::WindowFrame)) break;
      transport_wire::WindowFrame frame{};
      std::memcpy(&frame, rx_.data(), sizeof(frame));
      if (frame.version != transport_wire::kFrameVersion || frame.visible > 1) {
        if (Terminate()) record->terminal_changed = true;
        return InputTransportStatus::kTerminal;
      }
      auto result = InputTransportConsumptionResult::kConsumed;
      if (callbacks.on_window_consumption != nullptr) {
        result = ValidateConsumption(callbacks.on_window_consumption(
            callbacks.context, frame.left, frame.top, frame.right, frame.bottom,
            frame.visible != 0, frame.input_flags));
        if (result == InputTransportConsumptionResult::kDeferred)
          return InputTransportStatus::kBackpressured;
      }
      rx_.erase(rx_.begin(), rx_.begin() + sizeof(frame));
      record->consumed = true;
      if (callbacks.on_window != nullptr) {
        callbacks.on_window(callbacks.context, frame.left, frame.top,
                            frame.right, frame.bottom, frame.visible != 0,
                            frame.input_flags);
      }
      if (result == InputTransportConsumptionResult::kConsumedStop)
        return InputTransportStatus::kAccepted;
      continue;
    }
    if (magic == transport_wire::kFocusControlFrameMagic) {
      if (rx_.size() < sizeof(transport_wire::FocusControlFrame)) break;
      transport_wire::FocusControlFrame frame{};
      std::memcpy(&frame, rx_.data(), sizeof(frame));
      FocusControl control;
      if (!transport_wire::DecodeFocusControl(frame, &control)) {
        if (Terminate()) record->terminal_changed = true;
        return InputTransportStatus::kTerminal;
      }
      // A focus control without a consumer cannot be acknowledged by
      // transport: retain it until policy supplies a callback, just like a
      // callback that reports kDeferred.
      if (callbacks.on_focus == nullptr)
        return InputTransportStatus::kBackpressured;
      const FocusControlCallbackResult result =
          ValidateConsumption(callbacks.on_focus(
              callbacks.context, control.epoch, control.focused));
      if (result == FocusControlCallbackResult::kDeferred)
        return InputTransportStatus::kBackpressured;
      rx_.erase(rx_.begin(), rx_.begin() + sizeof(frame));
      record->consumed = true;
      if (result == FocusControlCallbackResult::kConsumedStop)
        return InputTransportStatus::kAccepted;
      continue;
    }
    if (magic != transport_wire::kInputFrameMagic) {
      if (Terminate()) record->terminal_changed = true;
      return InputTransportStatus::kTerminal;
    }
    if (rx_.size() < sizeof(transport_wire::InputFrame)) break;
    transport_wire::InputFrame frame{};
    std::memcpy(&frame, rx_.data(), sizeof(frame));
    if (frame.version != transport_wire::kFrameVersion ||
        frame.payload_size != sizeof(frame.payload) ||
        frame.kind != static_cast<uint32_t>(frame.payload.kind) ||
        !ValidPacket(frame.payload)) {
      if (Terminate()) record->terminal_changed = true;
      return InputTransportStatus::kTerminal;
    }
    auto result = InputTransportConsumptionResult::kConsumed;
    if (callbacks.on_packet_consumption != nullptr) {
      result = ValidateConsumption(
          callbacks.on_packet_consumption(callbacks.context, frame.payload));
      if (result == InputTransportConsumptionResult::kDeferred)
        return InputTransportStatus::kBackpressured;
    } else if (callbacks.on_packet != nullptr &&
        !callbacks.on_packet(callbacks.context, frame.payload))
      return InputTransportStatus::kBackpressured;
    rx_.erase(rx_.begin(), rx_.begin() + sizeof(frame));
    record->consumed = true;
    if (result == InputTransportConsumptionResult::kConsumedStop)
      return InputTransportStatus::kAccepted;
  }

  if (eof_) {
    // Dispatch complete final frames before publishing terminal. A deferred
    // or consumed-stop callback returns earlier and retains its FIFO suffix;
    // an incomplete final frame cannot become complete after peer EOF.
    if (Terminate()) record->terminal_changed = true;
    return InputTransportStatus::kTerminal;
  }
  return InputTransportStatus::kAccepted;
}

}  // namespace darwin_art::input
