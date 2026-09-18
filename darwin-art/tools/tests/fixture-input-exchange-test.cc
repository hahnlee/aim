#include "probes/fixture_input_exchange.h"

#include <algorithm>
#include <cassert>
#include <cstring>
#include <vector>
#include <array>
#include <limits>

using namespace darwin_art_graphics_fixture;
using darwin_art::input::transport_wire::AckFrameV2;
struct Io {
  std::vector<uint8_t> sent;
  AckFrameV2 ack{};
  size_t read_offset = 0;
  size_t chunk = 3;
  bool writable = false;
  bool readable = false;
  bool eof = false;
  FixtureInputExchange* reenter = nullptr;
  static FixtureIoResult Write(void* context, const void* data, size_t count) {
    auto& io = *static_cast<Io*>(context);
    if (!io.writable) return {FixtureIoStatus::kWouldBlock};
    if (io.reenter != nullptr) {
      auto* exchange = io.reenter;
      io.reenter = nullptr;
      exchange->Advance(io.port());
    }
    count = std::min(count, io.chunk);
    const auto* bytes = static_cast<const uint8_t*>(data);
    io.sent.insert(io.sent.end(), bytes, bytes + count);
    return {FixtureIoStatus::kProgress, count};
  }
  static FixtureIoResult Read(void* context, void* data, size_t count) {
    auto& io = *static_cast<Io*>(context);
    if (io.eof) return {FixtureIoStatus::kTerminal};
    if (!io.readable) return {FixtureIoStatus::kWouldBlock};
    count = std::min({count, io.chunk, sizeof(io.ack) - io.read_offset});
    std::memcpy(data, reinterpret_cast<uint8_t*>(&io.ack) + io.read_offset, count);
    io.read_offset += count;
    return {FixtureIoStatus::kProgress, count};
  }
  FixtureExchangeIo port() { return {Write, Read, this}; }
};
int main() {
  darwin_art::DarwinArtInputPacket packet{};
  packet.pointer.sequence = (1ULL << 40) + 9000;
  {
    FixtureInputExchange exchange;
    Io io;
    io.writable = true;
    io.reenter = &exchange;
    assert(exchange.Submit(packet));
    exchange.Advance(io.port());
    assert(io.sent.size() == io.chunk); // Recursive owner work cannot send twice.
  }
  for (uint64_t sequence : std::array<uint64_t, 3>{
           0, (1ULL << 40) + 9000, std::numeric_limits<uint64_t>::max()}) {
    packet.pointer.sequence = sequence;
  for (bool handled : {false, true}) {
    FixtureInputExchange exchange;
    Io io;
    io.ack.sequence = packet.pointer.sequence;
    io.ack.handled = handled;
    assert(exchange.Submit(packet));
    exchange.Advance(io.port());
    assert(io.sent.empty() && !exchange.result().submitted);
    assert(!exchange.Submit(packet));
    io.writable = true;
    for (size_t i = 0; i < 1000 && !exchange.result().submitted; ++i)
      exchange.Advance(io.port());
    assert(exchange.phase() == FixtureExchangePhase::kAwaitingAck);
    const auto sent = io.sent;
    for (int i = 0; i < 100; ++i) exchange.Advance(io.port());
    assert(io.sent == sent && !exchange.result().completed);
    assert(!exchange.result().handled && !exchange.Submit(packet));
    FixtureExchangeResult result;
    assert(!exchange.TakeCompleted(&result));
    io.readable = true;
    for (int i = 0; i < 100 && !exchange.result().completed; ++i)
      exchange.Advance(io.port());
    assert(exchange.TakeCompleted(&result));
    assert(result.submitted && result.completed && result.handled == handled);
    assert(result.packet_sequence == packet.pointer.sequence);
    assert(io.sent.size() == sizeof(darwin_art::input::transport_wire::InputFrame));
    assert(exchange.Submit(packet));
  }
  }
  for (int failure = 0; failure < 7; ++failure) {
    FixtureInputExchange exchange;
    Io io;
    io.chunk = 4096;
    io.writable = io.readable = true;
    io.eof = failure == 0;
    io.ack.sequence = packet.pointer.sequence;
    if (failure == 1) io.ack.magic = 0;
    if (failure == 2) io.ack.version = 99;
    if (failure == 3) io.ack.handled = 2;
    if (failure == 4) --io.ack.sequence;
    if (failure == 5) io.ack.reserved = 1;
    if (failure == 6) io.ack.version = 1;
    assert(exchange.Submit(packet));
    for (int i = 0; i < 3; ++i) exchange.Advance(io.port());
    assert(exchange.phase() == FixtureExchangePhase::kTerminal);
    assert(exchange.result().submitted && !exchange.result().completed);
    assert(!exchange.result().handled && !exchange.Submit(packet));
  }
}
