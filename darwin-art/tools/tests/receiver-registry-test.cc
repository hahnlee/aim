#include "runtime/framework/input/receiver_registry.h"

#include <atomic>
#include <cassert>
#include <cstdint>
#include <limits>
#include <memory>
#include <thread>
#include <type_traits>
#include <cstdlib>
#include <new>

// Test-only allocation failure at the real registry's map insertion boundary.
static std::atomic<bool> fail_next_allocation{false};
void* operator new(std::size_t size) {
  if (fail_next_allocation.exchange(false)) throw std::bad_alloc();
  if (auto* allocation = std::malloc(size == 0 ? 1 : size)) return allocation;
  throw std::bad_alloc();
}
void operator delete(void* allocation) noexcept { std::free(allocation); }

namespace darwin_art::input {

struct InputReceiver {
  explicit InputReceiver(ReceiverId observed_id = 0)
      : observed_id(observed_id) {}
  ~InputReceiver() {
    ++destructions;
    if (observed_id != 0) {
      destructor_reentered = AcquireInputReceiver(observed_id) == nullptr;
    }
  }

  ReceiverId observed_id;
  static std::atomic<int> destructions;
  static std::atomic<bool> destructor_reentered;
};

std::atomic<int> InputReceiver::destructions{0};
std::atomic<bool> InputReceiver::destructor_reentered{false};

}  // namespace darwin_art::input

int main() {
  using darwin_art::input::AcquireInputReceiver;
  using darwin_art::input::InputReceiver;
  using darwin_art::input::ReceiverId;
  using darwin_art::input::RegisterInputReceiver;
  using darwin_art::input::RetireInputReceiver;
  using darwin_art::input::SetNextInputReceiverIdForTesting;

  static_assert(!std::is_copy_constructible_v<darwin_art::input::InputReceiverReservation>);
  auto reservation = darwin_art::input::ReserveInputReceiverId();
  const auto reserved_id = reservation.Id();
  assert(reserved_id > 0 && AcquireInputReceiver(reserved_id) == nullptr);
  assert(RetireInputReceiver(reserved_id) == nullptr);
  auto moved = std::move(reservation);
  assert(reservation.Id() == 0 && moved.Id() == reserved_id);
  auto prepared = std::make_shared<InputReceiver>(reserved_id);
  assert(darwin_art::input::PublishInputReceiver(std::move(moved), prepared) == reserved_id);
  assert(moved.Id() == 0 && AcquireInputReceiver(reserved_id) == prepared);
  assert(darwin_art::input::PublishInputReceiver(std::move(moved), prepared) == 0);
  assert(RetireInputReceiver(reserved_id) == prepared);
  prepared.reset();
  ReceiverId abandoned_id = 0;
  {
    auto abandoned = darwin_art::input::ReserveInputReceiverId();
    abandoned_id = abandoned.Id();
  }
  auto after_abandonment = darwin_art::input::ReserveInputReceiverId();
  assert(after_abandonment.Id() > abandoned_id);
  const auto failed_id = after_abandonment.Id();
  assert(darwin_art::input::PublishInputReceiver(std::move(after_abandonment), {}) == 0);
  assert(after_abandonment.Id() == 0 && AcquireInputReceiver(abandoned_id) == nullptr);
  auto after_failure = darwin_art::input::ReserveInputReceiverId();
  assert(after_failure.Id() > failed_id && AcquireInputReceiver(failed_id) == nullptr);
  auto allocation_failure = darwin_art::input::ReserveInputReceiverId();
  const auto allocation_failure_id = allocation_failure.Id();
  auto unpublished = std::make_shared<InputReceiver>();
  fail_next_allocation = true;
  bool allocation_threw = false;
  try {
    (void)darwin_art::input::PublishInputReceiver(std::move(allocation_failure), unpublished);
  } catch (const std::bad_alloc&) {
    allocation_threw = true;
  }
  assert(allocation_threw && !fail_next_allocation);
  assert(allocation_failure.Id() == 0 && AcquireInputReceiver(allocation_failure_id) == nullptr);
  auto after_allocation_failure = darwin_art::input::ReserveInputReceiverId();
  assert(after_allocation_failure.Id() > allocation_failure_id);

  auto receiver = std::make_shared<InputReceiver>();
  const ReceiverId id = RegisterInputReceiver(receiver);
  assert(id > 0);
  assert(AcquireInputReceiver(id) != nullptr);
  auto lease = AcquireInputReceiver(id);
  auto retired = RetireInputReceiver(id);
  assert(retired == receiver);
  assert(AcquireInputReceiver(id) == nullptr);  // stale callback has no lease
  assert(RetireInputReceiver(id) == nullptr);  // only one concurrent winner
  receiver->observed_id = id;
  receiver.reset();
  retired.reset();
  assert(lease != nullptr);  // retirement does not invalidate an in-flight lease
  lease.reset();
  assert(InputReceiver::destructor_reentered.load());

  auto concurrent_receiver = std::make_shared<InputReceiver>();
  const ReceiverId concurrent_id = RegisterInputReceiver(concurrent_receiver);
  std::shared_ptr<InputReceiver> winners[2];
  std::thread first([&] { winners[0] = RetireInputReceiver(concurrent_id); });
  std::thread second([&] { winners[1] = RetireInputReceiver(concurrent_id); });
  first.join();
  second.join();
  assert((winners[0] != nullptr) != (winners[1] != nullptr));
  concurrent_receiver.reset();
  winners[0].reset();
  winners[1].reset();

  auto first_receiver = std::make_shared<InputReceiver>();
  const ReceiverId first_id = RegisterInputReceiver(first_receiver);
  assert(RetireInputReceiver(first_id) != nullptr);
  first_receiver.reset();
  auto second_receiver = std::make_shared<InputReceiver>();
  const ReceiverId second_id = RegisterInputReceiver(second_receiver);
  assert(second_id > first_id);  // IDs are monotonic and never reused.
  assert(RetireInputReceiver(second_id) != nullptr);
  second_receiver.reset();

  SetNextInputReceiverIdForTesting(
      static_cast<ReceiverId>(std::numeric_limits<std::int64_t>::max()));
  auto last_receiver = std::make_shared<InputReceiver>();
  const ReceiverId last_id = RegisterInputReceiver(last_receiver);
  assert(last_id ==
         static_cast<ReceiverId>(std::numeric_limits<std::int64_t>::max()));
  assert(RetireInputReceiver(last_id) != nullptr);
  last_receiver.reset();
  auto exhausted_receiver = std::make_shared<InputReceiver>();
  assert(RegisterInputReceiver(exhausted_receiver) == 0);
  assert(AcquireInputReceiver(last_id) == nullptr);

  return 0;
}
