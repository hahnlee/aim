#include "receiver_registry.h"

#include <limits>
#include <mutex>
#include <unordered_map>

namespace darwin_art::input {
namespace {

constexpr ReceiverId kInvalidReceiverId = 0;
constexpr ReceiverId kMaxReceiverId =
    static_cast<ReceiverId>(std::numeric_limits<std::int64_t>::max());

std::mutex g_receiver_registry_mutex;
std::unordered_map<ReceiverId, std::shared_ptr<InputReceiver>>
    g_receiver_registry;
ReceiverId g_next_receiver_id = 1;

}  // namespace

InputReceiverReservation ReserveInputReceiverId() {
  std::lock_guard<std::mutex> lock(g_receiver_registry_mutex);
  if (g_next_receiver_id == kInvalidReceiverId ||
      g_next_receiver_id > kMaxReceiverId) {
    return {};
  }
  return InputReceiverReservation(g_next_receiver_id++);
}

ReceiverId PublishInputReceiver(InputReceiverReservation&& reservation,
                                const std::shared_ptr<InputReceiver>& receiver) {
  const ReceiverId id = std::exchange(reservation.id_, kInvalidReceiverId);
  if (id == kInvalidReceiverId || receiver == nullptr) return kInvalidReceiverId;
  std::lock_guard<std::mutex> lock(g_receiver_registry_mutex);
  return g_receiver_registry.emplace(id, receiver).second ? id : kInvalidReceiverId;
}

ReceiverId RegisterInputReceiver(const std::shared_ptr<InputReceiver>& receiver) {
  if (receiver == nullptr) return kInvalidReceiverId;
  auto reservation = ReserveInputReceiverId();
  return PublishInputReceiver(std::move(reservation), receiver);
}

std::shared_ptr<InputReceiver> AcquireInputReceiver(ReceiverId id) {
  if (id == kInvalidReceiverId) return nullptr;
  std::lock_guard<std::mutex> lock(g_receiver_registry_mutex);
  const auto it = g_receiver_registry.find(id);
  return it == g_receiver_registry.end() ? nullptr : it->second;
}

std::shared_ptr<InputReceiver> RetireInputReceiver(ReceiverId id) {
  if (id == kInvalidReceiverId) return nullptr;
  // Keep the returned lease alive independently of the map entry. Erasing
  // under the lock therefore cannot run InputReceiver's destructor there.
  std::shared_ptr<InputReceiver> retired;
  {
    std::lock_guard<std::mutex> lock(g_receiver_registry_mutex);
    const auto it = g_receiver_registry.find(id);
    if (it == g_receiver_registry.end()) return nullptr;
    retired = it->second;
    g_receiver_registry.erase(it);
  }
  return retired;
}

#if defined(DARWIN_ART_RECEIVER_REGISTRY_TESTING)
void SetNextInputReceiverIdForTesting(ReceiverId next_id) {
  std::lock_guard<std::mutex> lock(g_receiver_registry_mutex);
  g_next_receiver_id = next_id;
}
#endif

}  // namespace darwin_art::input
