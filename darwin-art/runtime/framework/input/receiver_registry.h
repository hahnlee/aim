#pragma once

#include <cstdint>
#include <memory>
#include <utility>

namespace darwin_art::input {

// Defined privately by the receiver resource owner. Registry IDs are opaque positive
// jlong values; zero is invalid and IDs are never reused.
struct InputReceiver;
using ReceiverId = std::uint64_t;

// Reserving identity does not expose a partially initialized receiver. A
// reservation is single-use; abandoned IDs are burned, never recycled.
// Its holder must serialize moves/publication; sharing one reservation across
// concurrent operations is not supported. Registry lookups remain thread-safe.
class InputReceiverReservation final {
 public:
  InputReceiverReservation() = default;
  InputReceiverReservation(const InputReceiverReservation&) = delete;
  InputReceiverReservation& operator=(const InputReceiverReservation&) = delete;
  InputReceiverReservation(InputReceiverReservation&& other) noexcept
      : id_(std::exchange(other.id_, 0)) {}
  InputReceiverReservation& operator=(InputReceiverReservation&& other) noexcept {
    if (this != &other) id_ = std::exchange(other.id_, 0);
    return *this;
  }
  ReceiverId Id() const { return id_; }
 private:
  explicit InputReceiverReservation(ReceiverId id) : id_(id) {}
  friend InputReceiverReservation ReserveInputReceiverId();
  friend ReceiverId PublishInputReceiver(InputReceiverReservation&&,
                                         const std::shared_ptr<InputReceiver>&);
  ReceiverId id_ = 0;
};
InputReceiverReservation ReserveInputReceiverId();
ReceiverId PublishInputReceiver(InputReceiverReservation&& reservation,
                                const std::shared_ptr<InputReceiver>& receiver);

// Compatibility caller; preparation-sensitive product code uses reserve/publish.
ReceiverId RegisterInputReceiver(const std::shared_ptr<InputReceiver>& receiver);
std::shared_ptr<InputReceiver> AcquireInputReceiver(ReceiverId id);
std::shared_ptr<InputReceiver> RetireInputReceiver(ReceiverId id);

#if defined(DARWIN_ART_RECEIVER_REGISTRY_TESTING)
void SetNextInputReceiverIdForTesting(ReceiverId next_id);
#endif

}  // namespace darwin_art::input
