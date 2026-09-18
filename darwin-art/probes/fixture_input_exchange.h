#pragma once

#include <array>
#include <cstddef>
#include <cstdint>
#include <cstring>

#include "../runtime/framework/input/input_transport_wire.h"

namespace darwin_art_graphics_fixture {

// TEST ONLY. One outstanding input on a dedicated imported channel. Guest
// Os I/O and owner-Looper progress are supplied by the JNI fixture adapter;
// no receiver registry, finish ledger or production implementation is copied.
enum class FixtureIoStatus { kProgress, kWouldBlock, kTerminal };
struct FixtureIoResult {
  FixtureIoStatus status;
  size_t bytes = 0;
};
struct FixtureExchangeIo {
  FixtureIoResult (*write)(void*, const void*, size_t) = nullptr;
  FixtureIoResult (*read)(void*, void*, size_t) = nullptr;
  void* context = nullptr;
};
enum class FixtureExchangePhase { kIdle, kWriting, kAwaitingAck, kCompleted, kTerminal };
struct FixtureExchangeResult {
  // Complete frame accepted by the channel, not proof of Java completion.
  bool submitted = false;
  bool completed = false;
  bool handled = false;
  uint64_t packet_sequence = 0;
};

class FixtureInputExchange final {
 public:
  bool Submit(const darwin_art::DarwinArtInputPacket& packet) noexcept {
    if (phase_ != FixtureExchangePhase::kIdle) return false;
    frame_ = {};
    frame_.kind = static_cast<uint32_t>(packet.kind);
    frame_.payload = packet;
    expected_sequence_ = packet.kind == darwin_art::DarwinArtInputPacketKind::kPointer
                             ? packet.pointer.sequence : packet.key.sequence;
    written_ = received_ = 0;
    result_ = {};
    phase_ = FixtureExchangePhase::kWriting;
    return true;
  }

  // Bounded nonblocking work: no busy loop on EAGAIN or zero progress. Calling
  // again after a deadline resumes the same retained frame, never resubmits it.
  void Advance(const FixtureExchangeIo& io) noexcept {
    // Owner-Looper work may reenter the fixture. The original partial frame
    // must remain exclusively admitted across the I/O callback.
    if (advancing_) return;
    advancing_ = true;
    struct Admission {
      bool& advancing;
      ~Admission() { advancing = false; }
    } admission{advancing_};
    if (phase_ == FixtureExchangePhase::kWriting) {
      if (io.write == nullptr) { Fail(); return; }
      const size_t remaining = sizeof(frame_) - written_;
      const auto sent = io.write(io.context,
          reinterpret_cast<const uint8_t*>(&frame_) + written_, remaining);
      if (sent.status == FixtureIoStatus::kWouldBlock) return;
      if (sent.status != FixtureIoStatus::kProgress || sent.bytes == 0 ||
          sent.bytes > remaining) { Fail(); return; }
      written_ += sent.bytes;
      if (written_ != sizeof(frame_)) return;
      result_.submitted = true;
      phase_ = FixtureExchangePhase::kAwaitingAck;
    }
    if (phase_ != FixtureExchangePhase::kAwaitingAck) return;
    if (io.read == nullptr) { Fail(); return; }
    const size_t target = received_ < 8 ? 8 : ack_.size();
    const size_t remaining = target - received_;
    const auto read = io.read(io.context, ack_.data() + received_, remaining);
    if (read.status == FixtureIoStatus::kWouldBlock) return;
    if (read.status != FixtureIoStatus::kProgress || read.bytes == 0 ||
        read.bytes > remaining) { Fail(); return; }
    received_ += read.bytes;
    if (received_ >= 8) {
      uint32_t header[2];
      std::memcpy(header, ack_.data(), sizeof(header));
      if (header[0] != darwin_art::input::transport_wire::kAckFrameMagic ||
          header[1] != 2) { Fail(); return; }
    }
    if (received_ != ack_.size()) return;
    darwin_art::input::transport_wire::AckFrameV2 ack;
    std::memcpy(&ack, ack_.data(), sizeof(ack));
    if (ack.magic != darwin_art::input::transport_wire::kAckFrameMagic ||
        ack.version != 2 || ack.handled > 1 || ack.reserved != 0 ||
        ack.sequence != expected_sequence_) { Fail(); return; }
    result_.packet_sequence = ack.sequence;
    result_.completed = true;
    result_.handled = ack.handled != 0;
    phase_ = FixtureExchangePhase::kCompleted;
  }

  FixtureExchangePhase phase() const noexcept { return phase_; }
  FixtureExchangeResult result() const noexcept { return result_; }
  bool TakeCompleted(FixtureExchangeResult* result) noexcept {
    if (result == nullptr || phase_ != FixtureExchangePhase::kCompleted)
      return false;
    *result = result_;
    phase_ = FixtureExchangePhase::kIdle;
    return true;
  }

 private:
  void Fail() noexcept { phase_ = FixtureExchangePhase::kTerminal; }
  darwin_art::input::transport_wire::InputFrame frame_{};
  std::array<uint8_t, sizeof(darwin_art::input::transport_wire::AckFrameV2)> ack_{};
  uint64_t expected_sequence_ = 0;
  size_t written_ = 0;
  size_t received_ = 0;
  FixtureExchangeResult result_{};
  FixtureExchangePhase phase_ = FixtureExchangePhase::kIdle;
  bool advancing_ = false; // Owner-thread only, not cross-thread synchronization.
};

}  // namespace darwin_art_graphics_fixture
