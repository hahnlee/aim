#include "darwin_art_bionic_fd_broker.h"

#include <algorithm>
#include <array>
#include <condition_variable>
#include <cstddef>
#include <cstdint>
#include <cstring>
#include <deque>
#include <map>
#include <memory>
#include <mutex>
#include <optional>
#include <thread>
#include <utility>
#include <vector>

namespace {
constexpr uint32_t kTokenMarker = 0x40000000U;
constexpr uint32_t kSlotBits = 10;
constexpr uint32_t kSlotMask = (1U << kSlotBits) - 1;
constexpr uint32_t kGenerationMask = (1U << 20) - 1;
constexpr size_t kSlotCount = 1U << kSlotBits;
constexpr size_t kCopyChunk = 4096;
constexpr int16_t kPollNval = 0x20;
constexpr uint32_t kEpollOneShot = 0x40000000U;

struct Description;
struct Watch {
  int registration_fd;
  uint32_t events;
  uint64_t data;
  std::weak_ptr<Description> description;
  bool enabled;
};
struct ActivePollWake {
  DarwinArtFdOwnerV1 callbacks{};
  uint64_t object = 0;
  std::mutex mutex;
  bool closed = false;

  void Signal() {
    std::lock_guard lock(mutex);
    if (closed || callbacks.signal_poll_wake == nullptr)
      return;
    int ignored = 0;
    (void)callbacks.signal_poll_wake(callbacks.context, object, &ignored);
  }
  void Close() {
    std::lock_guard lock(mutex);
    if (closed)
      return;
    closed = true;
    if (callbacks.close_poll_wake != nullptr) {
      int ignored = 0;
      (void)callbacks.close_poll_wake(callbacks.context, object, &ignored);
    }
  }
};
struct EpollState {
  std::vector<Watch> watches;
  uint64_t generation = 0;
  std::vector<std::weak_ptr<ActivePollWake>> active_wakes;
};
struct Description {
  DarwinArtFdOwnerHandle owner = 0;
  DarwinArtFdKind kind = DARWIN_ART_FD_FS_FILE;
  DarwinArtFdOwnerV1 callbacks{};
  uint64_t object = 0;
  int status_flags = 0;
  int64_t offset = 0;
  size_t descriptor_refs = 1;
  size_t active = 0;
  bool closing = false;
  bool deferred_close = false;
  std::mutex io_mutex;
  std::shared_ptr<EpollState> epoll;
};
struct Owner {
  DarwinArtFdOwnerHandle handle = 0;
  DarwinArtFdKind kind = DARWIN_ART_FD_FS_FILE;
  DarwinArtFdOwnerV1 callbacks{};
  bool draining = false;
  size_t live_descriptions = 0;
  size_t active = 0;
};
struct Slot {
  uint32_t generation = 1;
  bool retired = false;
  bool live = false;
  bool closing = false;
  int descriptor_flags = 0;
  size_t active = 0;
  std::shared_ptr<Description> description;
  std::condition_variable changed;
};
struct Lease {
  size_t slot_index = kSlotCount;
  uint32_t generation = 0;
  std::shared_ptr<Description> description;
};
struct DarwinArtFdBrokerImpl {
  std::mutex mutex;
  std::condition_variable description_changed;
  std::array<Slot, kSlotCount> slots;
  std::vector<size_t> free_slots;
  std::deque<std::shared_ptr<Description>> deferred_closes;
  size_t pending_deferred_closes = 0;
  std::map<DarwinArtFdOwnerHandle, Owner> owners;
  DarwinArtFdOwnerHandle next_owner = 1;
  bool stopping = false;
  std::thread deferred_close_thread;
  DarwinArtFdBrokerImpl() {
    free_slots.reserve(kSlotCount);
    for (size_t index = kSlotCount; index-- > 0;)
      free_slots.push_back(index);
    deferred_close_thread = std::thread([this] { DeferredCloseLoop(); });
  }
  ~DarwinArtFdBrokerImpl() {
    {
      std::lock_guard lock(mutex);
      stopping = true;
      description_changed.notify_all();
    }
    if (deferred_close_thread.joinable())
      deferred_close_thread.join();
  }

  void DeferredCloseLoop() {
    for (;;) {
      std::shared_ptr<Description> description;
      {
        std::unique_lock lock(mutex);
        description_changed.wait(
            lock, [&] { return stopping || !deferred_closes.empty(); });
        if (stopping)
          return;
        for (;;) {
          auto ready =
              std::find_if(deferred_closes.begin(), deferred_closes.end(),
                           [](const std::shared_ptr<Description> &candidate) {
                             return candidate && candidate->active == 0;
                           });
          if (ready != deferred_closes.end()) {
            description = std::move(*ready);
            deferred_closes.erase(ready);
            break;
          }
          description_changed.wait(lock);
          if (stopping)
            return;
        }
      }

      int error = 0;
      if (description->kind != DARWIN_ART_FD_EPOLL &&
          description->callbacks.close != nullptr) {
        (void)description->callbacks.close(description->callbacks.context,
                                           description->object, &error);
      }

      std::lock_guard lock(mutex);
      if (description->owner != 0) {
        auto owner = owners.find(description->owner);
        if (owner == owners.end() || owner->second.live_descriptions == 0)
          std::terminate();
        --owner->second.live_descriptions;
      }
      description->deferred_close = false;
      if (pending_deferred_closes == 0)
        std::terminate();
      --pending_deferred_closes;
      description_changed.notify_all();
    }
  }
};

DarwinArtFdBrokerImpl *Impl(DarwinArtFdBroker *broker) {
  return reinterpret_cast<DarwinArtFdBrokerImpl *>(broker);
}
bool ValidOwnerKind(DarwinArtFdKind kind) {
  return kind >= DARWIN_ART_FD_FS_FILE && kind <= DARWIN_ART_FD_BINDER &&
         kind != DARWIN_ART_FD_EPOLL;
}
uint32_t MakeToken(size_t slot, uint32_t generation) {
  return kTokenMarker | (generation << kSlotBits) | static_cast<uint32_t>(slot);
}
bool DecodeToken(int fd, size_t *slot, uint32_t *generation) {
  uint32_t token = static_cast<uint32_t>(fd);
  if (fd < 0 || (token & kTokenMarker) == 0 || (token & 0x80000000U) != 0)
    return false;
  *slot = token & kSlotMask;
  *generation = (token >> kSlotBits) & kGenerationMask;
  return *generation != 0 && *slot < kSlotCount;
}
std::optional<size_t> TakeFreeSlotLocked(DarwinArtFdBrokerImpl *broker,
                                         uint32_t minimum) {
  for (size_t index = broker->free_slots.size(); index-- > 0;) {
    size_t slot = broker->free_slots[index];
    if (slot >= minimum && !broker->slots[slot].retired) {
      broker->free_slots.erase(broker->free_slots.begin() +
                               static_cast<ptrdiff_t>(index));
      return slot;
    }
  }
  return std::nullopt;
}
std::optional<size_t> TakeLowestFreeTokenLocked(DarwinArtFdBrokerImpl *broker,
                                                uint32_t minimum_fd) {
  std::optional<size_t> selected;
  size_t selected_position = 0;
  uint32_t selected_token = 0;
  for (size_t position = 0; position < broker->free_slots.size(); ++position) {
    const size_t slot_index = broker->free_slots[position];
    const Slot &slot = broker->slots[slot_index];
    if (slot.retired)
      continue;
    const uint32_t token = MakeToken(slot_index, slot.generation);
    if (token < minimum_fd || (selected && token >= selected_token))
      continue;
    selected = slot_index;
    selected_position = position;
    selected_token = token;
  }
  if (selected)
    broker->free_slots.erase(broker->free_slots.begin() +
                             static_cast<ptrdiff_t>(selected_position));
  return selected;
}
void RecycleSlotLocked(DarwinArtFdBrokerImpl *broker, size_t slot_index) {
  Slot &slot = broker->slots[slot_index];
  slot.live = false;
  slot.closing = false;
  slot.descriptor_flags = 0;
  slot.active = 0;
  slot.description.reset();
  if (slot.generation == kGenerationMask)
    slot.retired = true;
  else {
    ++slot.generation;
    broker->free_slots.push_back(slot_index);
  }
  slot.changed.notify_all();
}
DarwinArtFdBrokerStatus LookupSlotLocked(DarwinArtFdBrokerImpl *broker, int fd,
                                         size_t *slot_index,
                                         uint32_t *generation, Slot **slot) {
  if (!DecodeToken(fd, slot_index, generation))
    return DARWIN_ART_FD_BROKER_STALE;
  Slot &found = broker->slots[*slot_index];
  if (!found.live || found.closing || found.generation != *generation ||
      !found.description)
    return DARWIN_ART_FD_BROKER_STALE;
  *slot = &found;
  return DARWIN_ART_FD_BROKER_OK;
}
DarwinArtFdBrokerStatus AcquireLocked(DarwinArtFdBrokerImpl *broker, int fd,
                                      std::optional<DarwinArtFdKind> kind,
                                      Lease *lease) {
  size_t slot_index = 0;
  uint32_t generation = 0;
  Slot *slot = nullptr;
  DarwinArtFdBrokerStatus status =
      LookupSlotLocked(broker, fd, &slot_index, &generation, &slot);
  if (status != DARWIN_ART_FD_BROKER_OK)
    return status;
  auto description = slot->description;
  if (description->closing)
    return DARWIN_ART_FD_BROKER_STALE;
  if (kind && description->kind != *kind)
    return DARWIN_ART_FD_BROKER_WRONG_KIND;
  if (description->owner != 0) {
    auto owner = broker->owners.find(description->owner);
    if (owner == broker->owners.end())
      return DARWIN_ART_FD_BROKER_STALE;
    ++owner->second.active;
  }
  ++slot->active;
  ++description->active;
  *lease = Lease{slot_index, generation, std::move(description)};
  return DARWIN_ART_FD_BROKER_OK;
}
DarwinArtFdBrokerStatus
RetainDescriptionLocked(DarwinArtFdBrokerImpl *broker, int fd,
                        DarwinArtFdOwnerHandle expected_owner,
                        std::shared_ptr<Description> *description) {
  size_t slot_index = 0;
  uint32_t generation = 0;
  Slot *slot = nullptr;
  const auto status =
      LookupSlotLocked(broker, fd, &slot_index, &generation, &slot);
  if (status != DARWIN_ART_FD_BROKER_OK)
    return status;
  if (slot->description->owner != expected_owner)
    return DARWIN_ART_FD_BROKER_WRONG_OWNER;
  auto owner = broker->owners.find(expected_owner);
  if (owner == broker->owners.end())
    return DARWIN_ART_FD_BROKER_STALE;
  ++slot->description->active;
  ++owner->second.active;
  *description = slot->description;
  return DARWIN_ART_FD_BROKER_OK;
}
bool AcquireDescriptionLocked(DarwinArtFdBrokerImpl *broker,
                              const std::shared_ptr<Description> &description) {
  if (!description || description->closing || description->owner == 0)
    return false;
  auto owner = broker->owners.find(description->owner);
  if (owner == broker->owners.end())
    return false;
  ++description->active;
  ++owner->second.active;
  return true;
}
void ReleaseDescriptionLocked(DarwinArtFdBrokerImpl *broker,
                              const std::shared_ptr<Description> &description) {
  auto owner = broker->owners.find(description->owner);
  if (description->active == 0 || owner == broker->owners.end() ||
      owner->second.active == 0)
    std::terminate();
  --description->active;
  --owner->second.active;
  if (description->active == 0)
    broker->description_changed.notify_all();
}
void ReleaseLocked(DarwinArtFdBrokerImpl *broker, const Lease &lease) {
  Slot &slot = broker->slots[lease.slot_index];
  if (slot.generation != lease.generation || slot.active == 0 ||
      !lease.description || lease.description->active == 0)
    std::terminate();
  --slot.active;
  if (lease.description->owner != 0) {
    auto owner = broker->owners.find(lease.description->owner);
    if (owner == broker->owners.end() || owner->second.active == 0)
      std::terminate();
    --owner->second.active;
  }
  --lease.description->active;
  if (slot.active == 0) {
    if (slot.closing)
      RecycleSlotLocked(broker, lease.slot_index);
    else
      slot.changed.notify_all();
  }
  if (lease.description->active == 0)
    broker->description_changed.notify_all();
}
void SetResult(DarwinArtFdIoResult *result, intptr_t value, int error) {
  result->value = value;
  result->android_errno = error;
}
bool ValidOutputBuffer(const void *buffer, uint32_t capacity,
                       const uint32_t *length) {
  return (buffer == nullptr && capacity == 0 && length == nullptr) ||
         (buffer != nullptr && length != nullptr);
}
bool ValidSocketRequest(const DarwinArtFdSocketRequestV1 *request) {
  if (!request || request->abi_version != DARWIN_ART_FD_SOCKET_REQUEST_ABI_V1 ||
      request->struct_size < sizeof(*request) ||
      request->operation < DARWIN_ART_FD_SOCKET_BIND ||
      request->operation > DARWIN_ART_FD_SOCKET_ACCEPT4)
    return false;
  const bool input_bytes = request->byte_count == 0 || request->input_bytes;
  const bool output_bytes = request->byte_count == 0 || request->output_bytes;
  const bool output_address = ValidOutputBuffer(
      request->output_address, request->output_address_capacity,
      request->output_address_length);
  const bool option_output =
      ValidOutputBuffer(request->option_output, request->option_output_capacity,
                        request->option_output_length);
  const bool required_output_address =
      request->output_address && request->output_address_length;
  const bool required_option_output =
      request->option_output && request->option_output_length;
  switch (request->operation) {
  case DARWIN_ART_FD_SOCKET_BIND:
  case DARWIN_ART_FD_SOCKET_CONNECT:
    return request->address && request->address_length;
  case DARWIN_ART_FD_SOCKET_LISTEN:
  case DARWIN_ART_FD_SOCKET_SHUTDOWN:
    return true;
  case DARWIN_ART_FD_SOCKET_SEND:
    return input_bytes;
  case DARWIN_ART_FD_SOCKET_RECV:
    return output_bytes;
  case DARWIN_ART_FD_SOCKET_SENDTO:
    return input_bytes &&
           ((request->address == nullptr && request->address_length == 0) ||
            (request->address != nullptr && request->address_length != 0));
  case DARWIN_ART_FD_SOCKET_RECVFROM:
    return output_bytes && output_address;
  case DARWIN_ART_FD_SOCKET_GETSOCKOPT:
    return option_output && required_option_output;
  case DARWIN_ART_FD_SOCKET_SETSOCKOPT:
    return request->option_input_length == 0 || request->option_input;
  case DARWIN_ART_FD_SOCKET_GETPEERNAME:
  case DARWIN_ART_FD_SOCKET_GETSOCKNAME:
    return output_address && required_output_address;
  case DARWIN_ART_FD_SOCKET_ACCEPT4:
    return output_address;
  }
  return false;
}
template <typename Invoke>
DarwinArtFdBrokerStatus DispatchOne(DarwinArtFdBrokerImpl *broker, int fd,
                                    std::optional<DarwinArtFdKind> kind,
                                    DarwinArtFdIoResult *result,
                                    Invoke invoke) {
  if (!broker || !result)
    return DARWIN_ART_FD_BROKER_INVALID_ARGUMENT;
  Lease lease;
  {
    std::lock_guard lock(broker->mutex);
    auto status = AcquireLocked(broker, fd, kind, &lease);
    if (status != DARWIN_ART_FD_BROKER_OK)
      return status;
  }
  int error = 0;
  intptr_t value = invoke(lease, &error);
  {
    std::lock_guard lock(broker->mutex);
    ReleaseLocked(broker, lease);
  }
  SetResult(result, value, value < 0 ? error : 0);
  return DARWIN_ART_FD_BROKER_OK;
}
DarwinArtFdBrokerStatus Duplicate(DarwinArtFdBrokerImpl *broker, int old_fd,
                                  std::optional<uint32_t> minimum_fd, int flags,
                                  int *new_fd) {
  if (!broker || !new_fd || (flags & ~DARWIN_ART_FD_CLOEXEC))
    return DARWIN_ART_FD_BROKER_INVALID_ARGUMENT;
  std::lock_guard lock(broker->mutex);
  size_t old_index = 0;
  uint32_t old_generation = 0;
  Slot *old_slot = nullptr;
  auto status =
      LookupSlotLocked(broker, old_fd, &old_index, &old_generation, &old_slot);
  if (status != DARWIN_ART_FD_BROKER_OK)
    return status;
  auto selected = minimum_fd ? TakeLowestFreeTokenLocked(broker, *minimum_fd)
                             : TakeFreeSlotLocked(broker, 0);
  if (!selected)
    return DARWIN_ART_FD_BROKER_EXHAUSTED;
  Slot &slot = broker->slots[*selected];
  slot.live = true;
  slot.descriptor_flags = flags & DARWIN_ART_FD_CLOEXEC;
  slot.description = old_slot->description;
  ++slot.description->descriptor_refs;
  *new_fd = static_cast<int>(MakeToken(*selected, slot.generation));
  return DARWIN_ART_FD_BROKER_OK;
}
DarwinArtFdBrokerStatus
CloseImpl(DarwinArtFdBrokerImpl *broker,
          std::optional<DarwinArtFdOwnerHandle> expected, int fd,
          DarwinArtFdIoResult *result) {
  if (!broker || !result)
    return DARWIN_ART_FD_BROKER_INVALID_ARGUMENT;
  size_t slot_index = 0;
  uint32_t generation = 0;
  std::shared_ptr<Description> description;
  bool close_now = false;
  {
    std::lock_guard lock(broker->mutex);
    Slot *slot = nullptr;
    auto status = LookupSlotLocked(broker, fd, &slot_index, &generation, &slot);
    if (status != DARWIN_ART_FD_BROKER_OK)
      return status;
    description = slot->description;
    if (expected && description->owner != *expected)
      return DARWIN_ART_FD_BROKER_WRONG_OWNER;
    slot->closing = true;
    if (description->descriptor_refs == 0)
      std::terminate();
    --description->descriptor_refs;
    if (description->descriptor_refs != 0) {
      if (slot->active == 0)
        RecycleSlotLocked(broker, slot_index);
      else
        slot->changed.notify_all();
      SetResult(result, 0, 0);
      return DARWIN_ART_FD_BROKER_OK;
    }
    description->closing = true;
    if (description->active != 0) {
      try {
        broker->deferred_closes.push_back(description);
      } catch (...) {
        // Keep the descriptor fully live if the deferred work item cannot be
        // allocated; callers may retry close without losing the owner lease.
        description->closing = false;
        ++description->descriptor_refs;
        slot->closing = false;
        return DARWIN_ART_FD_BROKER_EXHAUSTED;
      }
      description->deferred_close = true;
      ++broker->pending_deferred_closes;
      if (slot->active == 0)
        RecycleSlotLocked(broker, slot_index);
      broker->description_changed.notify_all();
      SetResult(result, 0, 0);
      return DARWIN_ART_FD_BROKER_OK;
    }
    if (slot->active == 0)
      RecycleSlotLocked(broker, slot_index);
    close_now = true;
  }

  int error = 0;
  int close_result = 0;
  if (close_now && description->kind != DARWIN_ART_FD_EPOLL &&
      description->callbacks.close)
    close_result = description->callbacks.close(description->callbacks.context,
                                                description->object, &error);
  {
    std::lock_guard lock(broker->mutex);
    if (description->owner != 0) {
      auto owner = broker->owners.find(description->owner);
      if (owner == broker->owners.end() || owner->second.live_descriptions == 0)
        std::terminate();
      --owner->second.live_descriptions;
      broker->description_changed.notify_all();
    }
  }
  SetResult(result, close_result, close_result == 0 ? 0 : error);
  return DARWIN_ART_FD_BROKER_OK;
}

DarwinArtFdBrokerStatus Dup2Impl(DarwinArtFdBrokerImpl *broker, int old_fd,
                                 int new_fd, DarwinArtFdIoResult *result) {
  if (!broker || !result)
    return DARWIN_ART_FD_BROKER_INVALID_ARGUMENT;

  std::unique_lock lock(broker->mutex);
  size_t old_index = 0;
  size_t new_index = 0;
  uint32_t old_generation = 0;
  uint32_t new_generation = 0;
  Slot *old_slot = nullptr;
  Slot *new_slot = nullptr;
  auto status =
      LookupSlotLocked(broker, old_fd, &old_index, &old_generation, &old_slot);
  if (status != DARWIN_ART_FD_BROKER_OK)
    return status;
  status =
      LookupSlotLocked(broker, new_fd, &new_index, &new_generation, &new_slot);
  if (status != DARWIN_ART_FD_BROKER_OK)
    return status;

  // POSIX dup2 validates the descriptor before treating an identical pair as
  // a no-op. It also preserves the descriptor flags for that exact case.
  if (old_fd == new_fd) {
    SetResult(result, new_fd, 0);
    return DARWIN_ART_FD_BROKER_OK;
  }

  const auto source = old_slot->description;
  const auto target = new_slot->description;
  if (source == target) {
    // Different aliases of one open description still clear FD_CLOEXEC on the
    // destination, but do not perturb the shared description reference count.
    new_slot->descriptor_flags = 0;
    SetResult(result, new_fd, 0);
    return DARWIN_ART_FD_BROKER_OK;
  }

  ++source->descriptor_refs;
  if (target->descriptor_refs > 1) {
    --target->descriptor_refs;
    new_slot->description = source;
    new_slot->descriptor_flags = 0;
    new_slot->changed.notify_all();
    SetResult(result, new_fd, 0);
    return DARWIN_ART_FD_BROKER_OK;
  }

  // Keep the old Description detached but alive until every operation that
  // acquired it has released. The target Slot is swapped immediately, and
  // its active count is intentionally left untouched for those old leases.
  target->closing = true;
  target->descriptor_refs = 0;
  target->deferred_close = true;
  broker->deferred_closes.push_back(target);
  ++broker->pending_deferred_closes;
  new_slot->description = source;
  new_slot->descriptor_flags = 0;
  new_slot->changed.notify_all();
  broker->description_changed.notify_all();
  SetResult(result, new_fd, 0);
  return DARWIN_ART_FD_BROKER_OK;
}

DarwinArtFdBrokerStatus
PublishBatch(DarwinArtFdBrokerImpl *broker,
             const DarwinArtFdPublishBatchEntryV1 *entries, size_t count,
             int *guest_fds) {
  constexpr size_t kMaxBatch = DARWIN_ART_FD_BROKER_MAX_PUBLISH_BATCH;
  if (!broker || !entries || !guest_fds || count == 0 || count > kMaxBatch)
    return DARWIN_ART_FD_BROKER_INVALID_ARGUMENT;

  // Every check, allocation and reservation stays under one namespace lock.
  // A free slot is not removed until every Description has been allocated, so
  // all failure paths leave both the namespace and output array untouched.
  std::lock_guard lock(broker->mutex);
  std::array<Owner *, kMaxBatch> owners{};
  for (size_t index = 0; index < count; ++index) {
    if (entries[index].owner == 0 ||
        (entries[index].descriptor_flags &
         ~static_cast<int32_t>(DARWIN_ART_FD_CLOEXEC)) != 0)
      return DARWIN_ART_FD_BROKER_INVALID_ARGUMENT;
    auto found = broker->owners.find(entries[index].owner);
    if (found == broker->owners.end())
      return DARWIN_ART_FD_BROKER_STALE;
    if (found->second.draining)
      return DARWIN_ART_FD_BROKER_DRAINING;
    owners[index] = &found->second;
  }

  std::array<size_t, kMaxBatch> selected{};
  size_t selected_count = 0;
  for (size_t position = broker->free_slots.size();
       position != 0 && selected_count != count; --position) {
    const size_t slot_index = broker->free_slots[position - 1];
    if (!broker->slots[slot_index].retired)
      selected[selected_count++] = slot_index;
  }
  if (selected_count != count)
    return DARWIN_ART_FD_BROKER_EXHAUSTED;

  std::array<std::shared_ptr<Description>, kMaxBatch> descriptions;
  try {
    for (size_t index = 0; index < count; ++index) {
      auto description = std::make_shared<Description>();
      description->owner = entries[index].owner;
      description->kind = owners[index]->kind;
      description->callbacks = owners[index]->callbacks;
      description->object = entries[index].object;
      description->status_flags = entries[index].status_flags;
      descriptions[index] = std::move(description);
    }
  } catch (...) {
    // No provider callback runs here: opaque objects remain caller-owned when
    // Description allocation cannot complete.
    return DARWIN_ART_FD_BROKER_EXHAUSTED;
  }

  for (size_t index = 0; index < count; ++index) {
    const size_t slot_index = selected[index];
    auto free_slot = std::find(broker->free_slots.begin(),
                               broker->free_slots.end(), slot_index);
    if (free_slot == broker->free_slots.end())
      std::terminate();
    broker->free_slots.erase(free_slot);
    Slot &slot = broker->slots[slot_index];
    slot.live = true;
    slot.descriptor_flags = entries[index].descriptor_flags;
    slot.description = std::move(descriptions[index]);
    ++owners[index]->live_descriptions;
    guest_fds[index] = static_cast<int>(MakeToken(slot_index, slot.generation));
  }
  return DARWIN_ART_FD_BROKER_OK;
}
} // namespace

struct DarwinArtFdDescriptionPin {
  DarwinArtFdBrokerImpl *broker = nullptr;
  std::shared_ptr<Description> description;
};

extern "C" DarwinArtFdBroker *darwin_art_fd_broker_create() {
  return reinterpret_cast<DarwinArtFdBroker *>(new DarwinArtFdBrokerImpl());
}
extern "C" DarwinArtFdBrokerStatus
darwin_art_fd_broker_destroy(DarwinArtFdBroker *broker) {
  if (!broker)
    return DARWIN_ART_FD_BROKER_INVALID_ARGUMENT;
  auto *impl = Impl(broker);
  {
    std::lock_guard lock(impl->mutex);
    for (const Slot &slot : impl->slots)
      if (slot.live || slot.active || slot.closing)
        return DARWIN_ART_FD_BROKER_BUSY;
    if (!impl->owners.empty())
      return DARWIN_ART_FD_BROKER_BUSY;
  }
  delete impl;
  return DARWIN_ART_FD_BROKER_OK;
}
extern "C" DarwinArtFdBrokerStatus darwin_art_fd_broker_install_owner(
    DarwinArtFdBroker *broker, DarwinArtFdKind kind,
    const DarwinArtFdOwnerV1 *callbacks, DarwinArtFdOwnerHandle *owner) {
  if (!broker || !callbacks || !owner || !ValidOwnerKind(kind) ||
      (callbacks->abi_version != DARWIN_ART_FD_OWNER_ABI_V1 &&
       callbacks->abi_version != DARWIN_ART_FD_OWNER_ABI_V2 &&
       callbacks->abi_version != DARWIN_ART_FD_OWNER_ABI_V3 &&
       callbacks->abi_version != DARWIN_ART_FD_OWNER_ABI_V4 &&
       callbacks->abi_version != DARWIN_ART_FD_OWNER_ABI_V5 &&
       callbacks->abi_version != DARWIN_ART_FD_OWNER_ABI_V6 &&
       callbacks->abi_version != DARWIN_ART_FD_OWNER_ABI_V7))
    return DARWIN_ART_FD_BROKER_INVALID_ARGUMENT;
  const size_t required_size =
      callbacks->abi_version == DARWIN_ART_FD_OWNER_ABI_V1
          ? offsetof(DarwinArtFdOwnerV1, read_at)
      : callbacks->abi_version == DARWIN_ART_FD_OWNER_ABI_V2
          ? offsetof(DarwinArtFdOwnerV1, socket_operation)
      : callbacks->abi_version == DARWIN_ART_FD_OWNER_ABI_V3
          ? offsetof(DarwinArtFdOwnerV1, poll_many)
      : callbacks->abi_version == DARWIN_ART_FD_OWNER_ABI_V4
          ? offsetof(DarwinArtFdOwnerV1, set_status_flags)
      : callbacks->abi_version == DARWIN_ART_FD_OWNER_ABI_V5
          ? offsetof(DarwinArtFdOwnerV1, create_poll_wake)
      : callbacks->abi_version == DARWIN_ART_FD_OWNER_ABI_V6
          ? offsetof(DarwinArtFdOwnerV1, export_host_fd)
          : sizeof(DarwinArtFdOwnerV1);
  if (callbacks->struct_size < required_size)
    return DARWIN_ART_FD_BROKER_INVALID_ARGUMENT;
  DarwinArtFdOwnerV1 copied{};
  std::memcpy(&copied, callbacks, required_size);
  if (kind == DARWIN_ART_FD_SOCKET &&
      callbacks->abi_version >= DARWIN_ART_FD_OWNER_ABI_V3 &&
      (!copied.socket_operation || !copied.close))
    return DARWIN_ART_FD_BROKER_INVALID_ARGUMENT;
  auto *impl = Impl(broker);
  std::lock_guard lock(impl->mutex);
  if (impl->next_owner == 0)
    return DARWIN_ART_FD_BROKER_EXHAUSTED;
  auto handle = impl->next_owner++;
  impl->owners.emplace(handle, Owner{handle, kind, copied, false, 0, 0});
  *owner = handle;
  return DARWIN_ART_FD_BROKER_OK;
}
extern "C" DarwinArtFdBrokerStatus
darwin_art_fd_broker_uninstall_owner(DarwinArtFdBroker *broker,
                                     DarwinArtFdOwnerHandle owner) {
  if (!broker || owner == 0)
    return DARWIN_ART_FD_BROKER_INVALID_ARGUMENT;
  auto *impl = Impl(broker);
  std::lock_guard lock(impl->mutex);
  auto found = impl->owners.find(owner);
  if (found == impl->owners.end())
    return DARWIN_ART_FD_BROKER_STALE;
  found->second.draining = true;
  if (found->second.live_descriptions || found->second.active)
    return DARWIN_ART_FD_BROKER_BUSY;
  impl->owners.erase(found);
  return DARWIN_ART_FD_BROKER_OK;
}
extern "C" DarwinArtFdBrokerStatus
darwin_art_fd_broker_wait_owner_quiescent(DarwinArtFdBroker *broker,
                                          DarwinArtFdOwnerHandle owner) {
  if (!broker || owner == 0)
    return DARWIN_ART_FD_BROKER_INVALID_ARGUMENT;
  auto *impl = Impl(broker);
  std::unique_lock lock(impl->mutex);
  auto found = impl->owners.find(owner);
  if (found == impl->owners.end())
    return DARWIN_ART_FD_BROKER_STALE;
  impl->description_changed.wait(lock, [&] {
    auto current = impl->owners.find(owner);
    return current == impl->owners.end() ||
           (current->second.live_descriptions == 0 &&
            current->second.active == 0);
  });
  return impl->owners.find(owner) == impl->owners.end()
             ? DARWIN_ART_FD_BROKER_STALE
             : DARWIN_ART_FD_BROKER_OK;
}
extern "C" DarwinArtFdBrokerStatus
darwin_art_fd_broker_flush_deferred_closes(DarwinArtFdBroker *broker) {
  if (!broker)
    return DARWIN_ART_FD_BROKER_INVALID_ARGUMENT;
  auto *impl = Impl(broker);
  std::unique_lock lock(impl->mutex);
  impl->description_changed.wait(
      lock, [&] { return impl->pending_deferred_closes == 0; });
  return DARWIN_ART_FD_BROKER_OK;
}
extern "C" DarwinArtFdBrokerStatus darwin_art_fd_broker_publish_with_flags(
    DarwinArtFdBroker *broker, DarwinArtFdOwnerHandle owner, uint64_t object,
    int status_flags, int descriptor_flags, int *guest_fd) {
  if (!broker || !owner || !guest_fd ||
      (descriptor_flags & ~DARWIN_ART_FD_CLOEXEC))
    return DARWIN_ART_FD_BROKER_INVALID_ARGUMENT;
  auto *impl = Impl(broker);
  std::lock_guard lock(impl->mutex);
  auto found = impl->owners.find(owner);
  if (found == impl->owners.end())
    return DARWIN_ART_FD_BROKER_STALE;
  if (found->second.draining)
    return DARWIN_ART_FD_BROKER_DRAINING;
  auto selected = TakeFreeSlotLocked(impl, 0);
  if (!selected)
    return DARWIN_ART_FD_BROKER_EXHAUSTED;
  auto description = std::make_shared<Description>();
  description->owner = owner;
  description->kind = found->second.kind;
  description->callbacks = found->second.callbacks;
  description->object = object;
  description->status_flags = status_flags;
  Slot &slot = impl->slots[*selected];
  slot.live = true;
  slot.descriptor_flags = descriptor_flags;
  slot.description = std::move(description);
  ++found->second.live_descriptions;
  *guest_fd = static_cast<int>(MakeToken(*selected, slot.generation));
  return DARWIN_ART_FD_BROKER_OK;
}
extern "C" DarwinArtFdBrokerStatus
darwin_art_fd_broker_publish(DarwinArtFdBroker *broker,
                             DarwinArtFdOwnerHandle owner, uint64_t object,
                             int *guest_fd) {
  return darwin_art_fd_broker_publish_with_flags(broker, owner, object, 0, 0,
                                                 guest_fd);
}
extern "C" DarwinArtFdBrokerStatus
darwin_art_fd_broker_publish_batch_with_flags(
    DarwinArtFdBroker *broker, const DarwinArtFdPublishBatchEntryV1 *entries,
    size_t count, int *guest_fds) {
  return PublishBatch(Impl(broker), entries, count, guest_fds);
}
extern "C" DarwinArtFdBrokerStatus darwin_art_fd_broker_publish_pair_with_flags(
    DarwinArtFdBroker *broker, DarwinArtFdOwnerHandle owner,
    const uint64_t objects[2], const int status_flags[2],
    const int descriptor_flags[2], int guest_fds[2]) {
  if (!objects || !status_flags || !descriptor_flags || !guest_fds)
    return DARWIN_ART_FD_BROKER_INVALID_ARGUMENT;
  const DarwinArtFdPublishBatchEntryV1 entries[2] = {
      {owner, objects[0], status_flags[0], descriptor_flags[0]},
      {owner, objects[1], status_flags[1], descriptor_flags[1]},
  };
  return darwin_art_fd_broker_publish_batch_with_flags(broker, entries, 2,
                                                       guest_fds);
}
extern "C" DarwinArtFdBrokerStatus
darwin_art_fd_broker_dup(DarwinArtFdBroker *broker, int old_fd, int *new_fd) {
  return Duplicate(Impl(broker), old_fd, std::nullopt, 0, new_fd);
}
extern "C" DarwinArtFdBrokerStatus
darwin_art_fd_broker_dup2(DarwinArtFdBroker *broker, int old_fd, int new_fd,
                          DarwinArtFdIoResult *result) {
  return Dup2Impl(Impl(broker), old_fd, new_fd, result);
}
extern "C" DarwinArtFdBrokerStatus
darwin_art_fd_broker_duplicate_with_flags(DarwinArtFdBroker *broker, int old_fd,
                                          int flags, int *new_fd) {
  return Duplicate(Impl(broker), old_fd, std::nullopt, flags, new_fd);
}
extern "C" DarwinArtFdBrokerStatus
darwin_art_fd_broker_fcntl_dupfd_cloexec(DarwinArtFdBroker *broker, int old_fd,
                                         int minimum_fd, int *new_fd) {
  if (minimum_fd < 0)
    return DARWIN_ART_FD_BROKER_INVALID_ARGUMENT;
  return Duplicate(Impl(broker), old_fd, static_cast<uint32_t>(minimum_fd),
                   DARWIN_ART_FD_CLOEXEC, new_fd);
}

extern "C" DarwinArtFdBrokerStatus
darwin_art_fd_broker_get_descriptor_flags(DarwinArtFdBroker *broker, int fd,
                                          int *flags) {
  if (!broker || !flags)
    return DARWIN_ART_FD_BROKER_INVALID_ARGUMENT;
  auto *impl = Impl(broker);
  std::lock_guard lock(impl->mutex);
  size_t i = 0;
  uint32_t g = 0;
  Slot *slot = nullptr;
  auto status = LookupSlotLocked(impl, fd, &i, &g, &slot);
  if (status == DARWIN_ART_FD_BROKER_OK)
    *flags = slot->descriptor_flags;
  return status;
}
extern "C" DarwinArtFdBrokerStatus
darwin_art_fd_broker_set_descriptor_flags(DarwinArtFdBroker *broker, int fd,
                                          int flags) {
  if (!broker || (flags & ~DARWIN_ART_FD_CLOEXEC))
    return DARWIN_ART_FD_BROKER_INVALID_ARGUMENT;
  auto *impl = Impl(broker);
  std::lock_guard lock(impl->mutex);
  size_t i = 0;
  uint32_t g = 0;
  Slot *slot = nullptr;
  auto status = LookupSlotLocked(impl, fd, &i, &g, &slot);
  if (status == DARWIN_ART_FD_BROKER_OK)
    slot->descriptor_flags = flags;
  return status;
}
extern "C" DarwinArtFdBrokerStatus
darwin_art_fd_broker_get_status_flags(DarwinArtFdBroker *broker, int fd,
                                      int *flags) {
  if (!broker || !flags)
    return DARWIN_ART_FD_BROKER_INVALID_ARGUMENT;
  auto *impl = Impl(broker);
  std::lock_guard lock(impl->mutex);
  size_t i = 0;
  uint32_t g = 0;
  Slot *slot = nullptr;
  auto status = LookupSlotLocked(impl, fd, &i, &g, &slot);
  if (status == DARWIN_ART_FD_BROKER_OK)
    *flags = slot->description->status_flags;
  return status;
}
extern "C" DarwinArtFdBrokerStatus
darwin_art_fd_broker_set_status_flags(DarwinArtFdBroker *broker, int fd,
                                      int flags) {
  if (!broker)
    return DARWIN_ART_FD_BROKER_INVALID_ARGUMENT;
  auto *impl = Impl(broker);
  std::lock_guard lock(impl->mutex);
  size_t i = 0;
  uint32_t g = 0;
  Slot *slot = nullptr;
  auto status = LookupSlotLocked(impl, fd, &i, &g, &slot);
  if (status == DARWIN_ART_FD_BROKER_OK)
    slot->description->status_flags = flags;
  return status;
}
extern "C" DarwinArtFdBrokerStatus darwin_art_fd_broker_set_status_flags_io(
    DarwinArtFdBroker *broker, int fd, int flags, DarwinArtFdIoResult *result) {
  if (!broker || !result)
    return DARWIN_ART_FD_BROKER_INVALID_ARGUMENT;
  auto *impl = Impl(broker);
  Lease lease;
  {
    std::lock_guard lock(impl->mutex);
    const auto status = AcquireLocked(impl, fd, std::nullopt, &lease);
    if (status != DARWIN_ART_FD_BROKER_OK)
      return status;
  }
  int error = 0;
  int value = 0;
  if (lease.description->callbacks.set_status_flags != nullptr) {
    value = lease.description->callbacks.set_status_flags(
        lease.description->callbacks.context, lease.description->object, flags,
        &error);
  }
  {
    std::lock_guard lock(impl->mutex);
    if (value == 0)
      lease.description->status_flags = flags;
    ReleaseLocked(impl, lease);
  }
  SetResult(result, value < 0 ? -1 : 0, value < 0 ? error : 0);
  return DARWIN_ART_FD_BROKER_OK;
}
extern "C" DarwinArtFdBrokerStatus
darwin_art_fd_broker_get_offset(DarwinArtFdBroker *broker, int fd,
                                int64_t *offset) {
  if (!broker || !offset)
    return DARWIN_ART_FD_BROKER_INVALID_ARGUMENT;
  auto *impl = Impl(broker);
  Lease lease;
  {
    std::lock_guard lock(impl->mutex);
    auto s = AcquireLocked(impl, fd, std::nullopt, &lease);
    if (s != DARWIN_ART_FD_BROKER_OK)
      return s;
  }
  {
    std::lock_guard io_lock(lease.description->io_mutex);
    *offset = lease.description->offset;
  }
  {
    std::lock_guard lock(impl->mutex);
    ReleaseLocked(impl, lease);
  }
  return DARWIN_ART_FD_BROKER_OK;
}
extern "C" DarwinArtFdBrokerStatus
darwin_art_fd_broker_export_host_fd(DarwinArtFdBroker *broker, int fd,
                                    int *host_fd, DarwinArtFdIoResult *result) {
  if (!broker || !host_fd || !result)
    return DARWIN_ART_FD_BROKER_INVALID_ARGUMENT;
  auto *impl = Impl(broker);
  Lease lease;
  {
    std::lock_guard lock(impl->mutex);
    const auto status = AcquireLocked(impl, fd, std::nullopt, &lease);
    if (status != DARWIN_ART_FD_BROKER_OK)
      return status;
  }
  int error = 0;
  int value = -1;
  if (lease.description->callbacks.export_host_fd != nullptr) {
    value = lease.description->callbacks.export_host_fd(
        lease.description->callbacks.context, lease.description->object,
        host_fd, &error);
  } else {
    error = 95;
  }
  {
    std::lock_guard lock(impl->mutex);
    ReleaseLocked(impl, lease);
  }
  SetResult(result, value < 0 ? -1 : 0, value < 0 ? error : 0);
  return value < 0 ? DARWIN_ART_FD_BROKER_UNSUPPORTED : DARWIN_ART_FD_BROKER_OK;
}
extern "C" DarwinArtFdBrokerStatus darwin_art_fd_broker_retain_description(
    DarwinArtFdBroker *broker, int fd, DarwinArtFdOwnerHandle expected_owner,
    DarwinArtFdDescriptionOperationV1 operation, void *operation_context,
    DarwinArtFdDescriptionPin **pin, DarwinArtFdIoResult *result) {
  if (!broker || expected_owner == 0 || !operation || !pin || !result)
    return DARWIN_ART_FD_BROKER_INVALID_ARGUMENT;
  *pin = nullptr;
  SetResult(result, -1, 0);

  std::unique_ptr<DarwinArtFdDescriptionPin> retained;
  try {
    retained = std::make_unique<DarwinArtFdDescriptionPin>();
  } catch (...) {
    return DARWIN_ART_FD_BROKER_EXHAUSTED;
  }

  DarwinArtFdDescriptionSnapshotV1 snapshot{
      DARWIN_ART_FD_DESCRIPTION_SNAPSHOT_ABI_V1,
      sizeof(DarwinArtFdDescriptionSnapshotV1),
      0,
      0,
      0,
      0,
  };
  auto *impl = Impl(broker);
  {
    std::lock_guard lock(impl->mutex);
    std::shared_ptr<Description> description;
    const auto status =
        RetainDescriptionLocked(impl, fd, expected_owner, &description);
    if (status != DARWIN_ART_FD_BROKER_OK)
      return status;
    retained->broker = impl;
    retained->description = description;
    snapshot.object = description->object;
    snapshot.owner = description->owner;
    snapshot.kind = static_cast<uint32_t>(description->kind);
    snapshot.status_flags = description->status_flags;
  }

  int error = 0;
  const intptr_t value = operation(operation_context, &snapshot, &error);
  if (value < 0) {
    const int callback_error = error;
    {
      std::lock_guard lock(impl->mutex);
      ReleaseDescriptionLocked(impl, retained->description);
    }
    retained.reset();
    SetResult(result, value, callback_error);
    return DARWIN_ART_FD_BROKER_OK;
  }
  *pin = retained.release();
  SetResult(result, value, 0);
  return DARWIN_ART_FD_BROKER_OK;
}
extern "C" DarwinArtFdBrokerStatus
darwin_art_fd_broker_retain_exported_description(
    DarwinArtFdBroker *broker, int fd,
    DarwinArtFdRetainedExportOperationV1 operation,
    DarwinArtFdRetainedExportReleaseV1 release_export, void *operation_context,
    DarwinArtFdDescriptionPin **pin, DarwinArtFdIoResult *result) {
  if (!broker || !operation || !release_export || !pin || !result)
    return DARWIN_ART_FD_BROKER_INVALID_ARGUMENT;
  *pin = nullptr;
  SetResult(result, -1, 0);

  std::unique_ptr<DarwinArtFdDescriptionPin> retained;
  try {
    retained = std::make_unique<DarwinArtFdDescriptionPin>();
  } catch (...) {
    return DARWIN_ART_FD_BROKER_EXHAUSTED;
  }

  DarwinArtFdDescriptionSnapshotV1 snapshot{
      DARWIN_ART_FD_DESCRIPTION_SNAPSHOT_ABI_V1,
      sizeof(DarwinArtFdDescriptionSnapshotV1),
      0,
      0,
      0,
      0,
  };
  auto *impl = Impl(broker);
  {
    std::lock_guard lock(impl->mutex);
    size_t slot_index = 0;
    uint32_t generation = 0;
    Slot *slot = nullptr;
    const auto status =
        LookupSlotLocked(impl, fd, &slot_index, &generation, &slot);
    if (status != DARWIN_ART_FD_BROKER_OK)
      return status;
    if (slot->description->owner == 0 || slot->description->closing)
      return DARWIN_ART_FD_BROKER_STALE;
    auto owner = impl->owners.find(slot->description->owner);
    if (owner == impl->owners.end())
      return DARWIN_ART_FD_BROKER_STALE;
    ++slot->description->active;
    ++owner->second.active;
    retained->broker = impl;
    retained->description = slot->description;
    snapshot.object = slot->description->object;
    snapshot.owner = slot->description->owner;
    snapshot.kind = static_cast<uint32_t>(slot->description->kind);
    snapshot.status_flags = slot->description->status_flags;
  }

  int host_fd = -1;
  int export_error = 0;
  const auto &callbacks = retained->description->callbacks;
  const int exported =
      callbacks.export_host_fd == nullptr
          ? -1
          : callbacks.export_host_fd(callbacks.context, snapshot.object,
                                     &host_fd, &export_error);
  if (exported < 0 || host_fd < 0) {
    if (host_fd >= 0)
      release_export(operation_context, host_fd);
    if (export_error == 0)
      export_error = exported < 0 ? 95 : 5;
    {
      std::lock_guard lock(impl->mutex);
      ReleaseDescriptionLocked(impl, retained->description);
    }
    retained.reset();
    SetResult(result, -1, export_error);
    return DARWIN_ART_FD_BROKER_OK;
  }

  int operation_error = 0;
  const intptr_t value =
      operation(operation_context, &snapshot, host_fd, &operation_error);
  if (value < 0) {
    const int callback_error = operation_error;
    release_export(operation_context, host_fd);
    {
      std::lock_guard lock(impl->mutex);
      ReleaseDescriptionLocked(impl, retained->description);
    }
    retained.reset();
    SetResult(result, value, callback_error);
    return DARWIN_ART_FD_BROKER_OK;
  }
  *pin = retained.release();
  SetResult(result, value, 0);
  return DARWIN_ART_FD_BROKER_OK;
}
extern "C" DarwinArtFdBrokerStatus
darwin_art_fd_broker_release_description(DarwinArtFdBroker *broker,
                                         DarwinArtFdDescriptionPin *pin) {
  if (!broker || !pin)
    return DARWIN_ART_FD_BROKER_INVALID_ARGUMENT;
  auto *impl = Impl(broker);
  std::shared_ptr<Description> description;
  {
    std::lock_guard lock(impl->mutex);
    if (pin->broker != impl || !pin->description)
      return DARWIN_ART_FD_BROKER_STALE;
    description = std::move(pin->description);
    ReleaseDescriptionLocked(impl, description);
  }
  delete pin;
  return DARWIN_ART_FD_BROKER_OK;
}
extern "C" DarwinArtFdBrokerStatus
darwin_art_fd_broker_get_kind(DarwinArtFdBroker *broker, int fd,
                              DarwinArtFdKind *kind) {
  if (!broker || !kind)
    return DARWIN_ART_FD_BROKER_INVALID_ARGUMENT;
  auto *impl = Impl(broker);
  Lease lease;
  {
    std::lock_guard lock(impl->mutex);
    const auto status = AcquireLocked(impl, fd, std::nullopt, &lease);
    if (status != DARWIN_ART_FD_BROKER_OK)
      return status;
    *kind = lease.description->kind;
    ReleaseLocked(impl, lease);
  }
  return DARWIN_ART_FD_BROKER_OK;
}
extern "C" DarwinArtFdBrokerStatus
darwin_art_fd_broker_close(DarwinArtFdBroker *broker, int fd,
                           DarwinArtFdIoResult *result) {
  return CloseImpl(Impl(broker), std::nullopt, fd, result);
}
extern "C" DarwinArtFdBrokerStatus
darwin_art_fd_broker_close_owned(DarwinArtFdBroker *broker,
                                 DarwinArtFdOwnerHandle owner, int fd,
                                 DarwinArtFdIoResult *result) {
  if (!owner)
    return DARWIN_ART_FD_BROKER_INVALID_ARGUMENT;
  return CloseImpl(Impl(broker), owner, fd, result);
}

extern "C" DarwinArtFdBrokerStatus
darwin_art_fd_broker_read(DarwinArtFdBroker *broker, int fd, void *bytes,
                          size_t count, DarwinArtFdIoResult *result) {
  if (count && !bytes)
    return DARWIN_ART_FD_BROKER_INVALID_ARGUMENT;
  return DispatchOne(
      Impl(broker), fd, std::nullopt, result, [&](const Lease &l, int *e) {
        std::lock_guard io(l.description->io_mutex);
        auto &c = l.description->callbacks;
        if (c.read_at)
          return c.read_at(c.context, l.description->object,
                           &l.description->offset, bytes, count, e);
        if (c.read)
          return c.read(c.context, l.description->object, bytes, count, e);
        *e = 9;
        return intptr_t{-1};
      });
}
extern "C" DarwinArtFdBrokerStatus
darwin_art_fd_broker_write(DarwinArtFdBroker *broker, int fd, const void *bytes,
                           size_t count, DarwinArtFdIoResult *result) {
  if (count && !bytes)
    return DARWIN_ART_FD_BROKER_INVALID_ARGUMENT;
  return DispatchOne(
      Impl(broker), fd, std::nullopt, result, [&](const Lease &l, int *e) {
        std::lock_guard io(l.description->io_mutex);
        auto &c = l.description->callbacks;
        if (c.write_at)
          return c.write_at(c.context, l.description->object,
                            &l.description->offset, bytes, count, e);
        if (c.write)
          return c.write(c.context, l.description->object, bytes, count, e);
        *e = 9;
        return intptr_t{-1};
      });
}
extern "C" DarwinArtFdBrokerStatus
darwin_art_fd_broker_ioctl(DarwinArtFdBroker *broker, int fd,
                           DarwinArtFdKind kind, uint64_t request,
                           void *argument, DarwinArtFdIoResult *result) {
  if (!ValidOwnerKind(kind))
    return DARWIN_ART_FD_BROKER_INVALID_ARGUMENT;
  return DispatchOne(
      Impl(broker), fd, kind, result, [&](const Lease &l, int *e) {
        auto &c = l.description->callbacks;
        if (!c.ioctl) {
          *e = 25;
          return intptr_t{-1};
        }
        return static_cast<intptr_t>(
            c.ioctl(c.context, l.description->object, request, argument, e));
      });
}
extern "C" DarwinArtFdBrokerStatus
darwin_art_fd_broker_poll(DarwinArtFdBroker *broker,
                          DarwinArtFdPollEntry *entries, size_t count,
                          DarwinArtFdIoResult *result) {
  return darwin_art_fd_broker_poll_wait(broker, entries, count, 0, result);
}

extern "C" DarwinArtFdBrokerStatus
darwin_art_fd_broker_poll_wait(DarwinArtFdBroker *broker,
                               DarwinArtFdPollEntry *entries, size_t count,
                               int timeout_ms, DarwinArtFdIoResult *result) {
  if (!broker || !result || (count && !entries))
    return DARWIN_ART_FD_BROKER_INVALID_ARGUMENT;
  if (timeout_ms < -1)
    return DARWIN_ART_FD_BROKER_INVALID_ARGUMENT;
  auto *impl = Impl(broker);
  std::vector<Lease> leases(count);
  std::vector<size_t> leased_indices;
  leased_indices.reserve(count);
  intptr_t invalid = 0;
  {
    std::lock_guard lock(impl->mutex);
    for (size_t index = 0; index < count; ++index) {
      entries[index].revents = 0;
      if (entries[index].fd < 0)
        continue;
      const DarwinArtFdBrokerStatus status =
          AcquireLocked(impl, entries[index].fd, std::nullopt, &leases[index]);
      if (status == DARWIN_ART_FD_BROKER_STALE) {
        entries[index].revents = kPollNval;
        ++invalid;
        continue;
      }
      if (status != DARWIN_ART_FD_BROKER_OK) {
        for (size_t acquired : leased_indices)
          ReleaseLocked(impl, leases[acquired]);
        return status;
      }
      leased_indices.push_back(index);
    }
  }

  decltype(DarwinArtFdOwnerV1::poll_many) batch = nullptr;
  void *batch_context = nullptr;
  bool one_batch = !leased_indices.empty();
  for (size_t index : leased_indices) {
    const auto &callbacks = leases[index].description->callbacks;
    if (callbacks.poll_many == nullptr) {
      one_batch = false;
      break;
    }
    if (batch == nullptr) {
      batch = callbacks.poll_many;
      batch_context = callbacks.context;
    } else if (batch != callbacks.poll_many ||
               batch_context != callbacks.context) {
      one_batch = false;
      break;
    }
  }

  intptr_t ready = invalid;
  int android_errno = 0;
  if (one_batch) {
    std::vector<uint64_t> objects;
    std::vector<int16_t> events;
    std::vector<int16_t> revents(leased_indices.size(), 0);
    objects.reserve(leased_indices.size());
    events.reserve(leased_indices.size());
    for (size_t index : leased_indices) {
      objects.push_back(leases[index].description->object);
      events.push_back(entries[index].events);
    }
    const int value =
        batch(batch_context, objects.data(), events.data(), revents.data(),
              revents.size(), invalid == 0 ? timeout_ms : 0, &android_errno);
    if (value < 0) {
      ready = -1;
    } else {
      for (size_t position = 0; position < leased_indices.size(); ++position) {
        entries[leased_indices[position]].revents = revents[position];
        if (revents[position] != 0)
          ++ready;
      }
    }
  } else {
    for (size_t index : leased_indices) {
      auto &callbacks = leases[index].description->callbacks;
      if (callbacks.poll == nullptr) {
        entries[index].revents = kPollNval;
        ++ready;
        continue;
      }
      const int value = callbacks.poll(
          callbacks.context, leases[index].description->object,
          entries[index].events, &entries[index].revents, &android_errno);
      if (value < 0) {
        ready = -1;
        break;
      }
      if (value > 0 || entries[index].revents != 0)
        ++ready;
    }
    if (ready == 0 && timeout_ms != 0) {
      android_errno = 38;
      ready = -1;
    }
  }

  {
    std::lock_guard lock(impl->mutex);
    for (size_t index : leased_indices)
      ReleaseLocked(impl, leases[index]);
  }
  SetResult(result, ready, ready < 0 ? android_errno : 0);
  return DARWIN_ART_FD_BROKER_OK;
}

extern "C" DarwinArtFdBrokerStatus
darwin_art_fd_broker_sendfile(DarwinArtFdBroker *broker, int out_fd, int in_fd,
                              size_t count, DarwinArtFdIoResult *result) {
  if (!broker || !result)
    return DARWIN_ART_FD_BROKER_INVALID_ARGUMENT;
  auto *impl = Impl(broker);
  Lease in, out;
  {
    std::lock_guard lock(impl->mutex);
    auto s = AcquireLocked(impl, in_fd, DARWIN_ART_FD_FS_FILE, &in);
    if (s != DARWIN_ART_FD_BROKER_OK)
      return s;
    s = AcquireLocked(impl, out_fd, std::nullopt, &out);
    if (s != DARWIN_ART_FD_BROKER_OK) {
      ReleaseLocked(impl, in);
      return s;
    }
    if (out.description->kind != DARWIN_ART_FD_FS_FILE &&
        out.description->kind != DARWIN_ART_FD_SOCKET) {
      ReleaseLocked(impl, out);
      ReleaseLocked(impl, in);
      return DARWIN_ART_FD_BROKER_WRONG_KIND;
    }
  }
  if (in.description == out.description) {
    std::lock_guard lock(impl->mutex);
    ReleaseLocked(impl, out);
    ReleaseLocked(impl, in);
    return DARWIN_ART_FD_BROKER_UNSUPPORTED;
  }
  std::scoped_lock io(in.description->io_mutex, out.description->io_mutex);
  std::array<uint8_t, kCopyChunk> buffer{};
  size_t transferred = 0;
  int error = 0;
  while (transferred < count) {
    size_t requested = std::min(buffer.size(), count - transferred);
    auto &ic = in.description->callbacks;
    auto &oc = out.description->callbacks;
    if ((!ic.read_at && !ic.read) || (!oc.write_at && !oc.write)) {
      error = 9;
      transferred = static_cast<size_t>(-1);
      break;
    }
    intptr_t r = ic.read_at ? ic.read_at(ic.context, in.description->object,
                                         &in.description->offset, buffer.data(),
                                         requested, &error)
                            : ic.read(ic.context, in.description->object,
                                      buffer.data(), requested, &error);
    if (r > static_cast<intptr_t>(requested) || r < 0) {
      if (r > static_cast<intptr_t>(requested))
        error = 5;
      transferred = static_cast<size_t>(-1);
      break;
    }
    if (r == 0)
      break;
    intptr_t w = oc.write_at
                     ? oc.write_at(oc.context, out.description->object,
                                   &out.description->offset, buffer.data(),
                                   static_cast<size_t>(r), &error)
                     : oc.write(oc.context, out.description->object,
                                buffer.data(), static_cast<size_t>(r), &error);
    if (w < 0 || w > r) {
      if (w > r)
        error = 5;
      transferred = static_cast<size_t>(-1);
      break;
    }
    transferred += static_cast<size_t>(w);
    if (w < r)
      break;
  }
  {
    std::lock_guard lock(impl->mutex);
    ReleaseLocked(impl, out);
    ReleaseLocked(impl, in);
  }
  SetResult(result,
            transferred == static_cast<size_t>(-1)
                ? -1
                : static_cast<intptr_t>(transferred),
            transferred == static_cast<size_t>(-1) ? error : 0);
  return DARWIN_ART_FD_BROKER_OK;
}

extern "C" DarwinArtFdBrokerStatus darwin_art_fd_broker_socket_operation(
    DarwinArtFdBroker *broker, DarwinArtFdOwnerHandle owner, int fd,
    const DarwinArtFdSocketRequestV1 *request, DarwinArtFdIoResult *result) {
  if (!broker || !owner || !result || !ValidSocketRequest(request))
    return DARWIN_ART_FD_BROKER_INVALID_ARGUMENT;
  auto *impl = Impl(broker);
  Lease lease;
  {
    std::lock_guard lock(impl->mutex);
    auto status = AcquireLocked(impl, fd, DARWIN_ART_FD_SOCKET, &lease);
    if (status != DARWIN_ART_FD_BROKER_OK)
      return status;
    if (lease.description->owner != owner) {
      ReleaseLocked(impl, lease);
      return DARWIN_ART_FD_BROKER_WRONG_OWNER;
    }
    if (!lease.description->callbacks.socket_operation) {
      ReleaseLocked(impl, lease);
      return DARWIN_ART_FD_BROKER_UNSUPPORTED;
    }
  }

  DarwinArtFdSocketAcceptResultV1 accepted{
      DARWIN_ART_FD_SOCKET_ACCEPT_RESULT_ABI_V1,
      sizeof(DarwinArtFdSocketAcceptResultV1),
      0,
      0,
      0,
      0};
  int error = 0;
  auto &callbacks = lease.description->callbacks;
  const intptr_t value = callbacks.socket_operation(
      callbacks.context, lease.description->object, request, &accepted, &error);
  if (request->operation != DARWIN_ART_FD_SOCKET_ACCEPT4 || value < 0) {
    std::lock_guard lock(impl->mutex);
    ReleaseLocked(impl, lease);
    SetResult(result, value, value < 0 ? error : 0);
    return DARWIN_ART_FD_BROKER_OK;
  }

  DarwinArtFdBrokerStatus publication = DARWIN_ART_FD_BROKER_OK;
  int accepted_fd = -1;
  {
    std::lock_guard lock(impl->mutex);
    auto found = impl->owners.find(owner);
    if (accepted.abi_version != DARWIN_ART_FD_SOCKET_ACCEPT_RESULT_ABI_V1 ||
        accepted.struct_size < sizeof(accepted) ||
        accepted.kind != DARWIN_ART_FD_SOCKET ||
        (accepted.descriptor_flags & ~DARWIN_ART_FD_CLOEXEC)) {
      publication = DARWIN_ART_FD_BROKER_INVALID_ARGUMENT;
    } else if (found == impl->owners.end()) {
      publication = DARWIN_ART_FD_BROKER_STALE;
    } else if (found->second.draining) {
      publication = DARWIN_ART_FD_BROKER_DRAINING;
    } else {
      auto selected = TakeFreeSlotLocked(impl, 0);
      if (!selected) {
        publication = DARWIN_ART_FD_BROKER_EXHAUSTED;
      } else {
        auto description = std::make_shared<Description>();
        description->owner = owner;
        description->kind = DARWIN_ART_FD_SOCKET;
        description->callbacks = found->second.callbacks;
        description->object = accepted.object;
        description->status_flags = accepted.status_flags;
        Slot &slot = impl->slots[*selected];
        slot.live = true;
        slot.descriptor_flags = accepted.descriptor_flags;
        slot.description = std::move(description);
        ++found->second.live_descriptions;
        accepted_fd = static_cast<int>(MakeToken(*selected, slot.generation));
      }
    }
    if (publication == DARWIN_ART_FD_BROKER_OK)
      ReleaseLocked(impl, lease);
  }
  if (publication != DARWIN_ART_FD_BROKER_OK) {
    int ignored_error = 0;
    if (callbacks.close)
      (void)callbacks.close(callbacks.context, accepted.object, &ignored_error);
    {
      std::lock_guard lock(impl->mutex);
      ReleaseLocked(impl, lease);
    }
    return publication;
  }
  SetResult(result, accepted_fd, 0);
  return DARWIN_ART_FD_BROKER_OK;
}

extern "C" DarwinArtFdBrokerStatus
darwin_art_fd_broker_epoll_create1(DarwinArtFdBroker *broker, int flags,
                                   int *epoll_fd) {
  if (!broker || !epoll_fd || (flags & ~DARWIN_ART_EPOLL_CLOEXEC))
    return DARWIN_ART_FD_BROKER_INVALID_ARGUMENT;
  auto *impl = Impl(broker);
  std::lock_guard lock(impl->mutex);
  auto selected = TakeFreeSlotLocked(impl, 0);
  if (!selected)
    return DARWIN_ART_FD_BROKER_EXHAUSTED;
  auto d = std::make_shared<Description>();
  d->kind = DARWIN_ART_FD_EPOLL;
  d->epoll = std::make_shared<EpollState>();
  Slot &slot = impl->slots[*selected];
  slot.live = true;
  slot.descriptor_flags = flags ? DARWIN_ART_FD_CLOEXEC : 0;
  slot.description = std::move(d);
  *epoll_fd = static_cast<int>(MakeToken(*selected, slot.generation));
  return DARWIN_ART_FD_BROKER_OK;
}
extern "C" DarwinArtFdBrokerStatus darwin_art_fd_broker_epoll_ctl(
    DarwinArtFdBroker *broker, int epoll_fd, int operation, int target_fd,
    const DarwinArtFdEpollEvent *event, DarwinArtFdIoResult *result) {
  if (!broker || !result || (operation != DARWIN_ART_EPOLL_CTL_DEL && !event))
    return DARWIN_ART_FD_BROKER_INVALID_ARGUMENT;
  auto *impl = Impl(broker);
  std::unique_lock lock(impl->mutex);
  size_t ei = 0, ti = 0;
  uint32_t eg = 0, tg = 0;
  Slot *es = nullptr;
  Slot *ts = nullptr;
  auto status = LookupSlotLocked(impl, epoll_fd, &ei, &eg, &es);
  if (status != DARWIN_ART_FD_BROKER_OK)
    return status;
  if (es->description->kind != DARWIN_ART_FD_EPOLL)
    return DARWIN_ART_FD_BROKER_WRONG_KIND;
  status = LookupSlotLocked(impl, target_fd, &ti, &tg, &ts);
  if (status != DARWIN_ART_FD_BROKER_OK)
    return status;
  if (ts->description->kind == DARWIN_ART_FD_EPOLL ||
      ts->description->callbacks.poll == nullptr)
    return DARWIN_ART_FD_BROKER_WRONG_KIND;
  auto &watches = es->description->epoll->watches;
  auto found =
      std::find_if(watches.begin(), watches.end(), [&](const Watch &w) {
        return w.registration_fd == target_fd;
      });
  if (operation == DARWIN_ART_EPOLL_CTL_ADD) {
    if (found != watches.end())
      return DARWIN_ART_FD_BROKER_ALREADY_EXISTS;
    watches.push_back(
        Watch{target_fd, event->events, event->data, ts->description, true});
  } else if (operation == DARWIN_ART_EPOLL_CTL_MOD) {
    if (found == watches.end())
      return DARWIN_ART_FD_BROKER_STALE;
    found->events = event->events;
    found->data = event->data;
    found->enabled = true;
  } else if (operation == DARWIN_ART_EPOLL_CTL_DEL) {
    if (found == watches.end())
      return DARWIN_ART_FD_BROKER_STALE;
    watches.erase(found);
  } else
    return DARWIN_ART_FD_BROKER_INVALID_ARGUMENT;
  auto &epoll_state = *es->description->epoll;
  ++epoll_state.generation;
  std::vector<std::shared_ptr<ActivePollWake>> wakes;
  auto wake = epoll_state.active_wakes.begin();
  while (wake != epoll_state.active_wakes.end()) {
    if (auto active = wake->lock()) {
      wakes.push_back(std::move(active));
      ++wake;
    } else {
      wake = epoll_state.active_wakes.erase(wake);
    }
  }
  SetResult(result, 0, 0);
  lock.unlock();
  for (const auto &active : wakes)
    active->Signal();
  return DARWIN_ART_FD_BROKER_OK;
}
extern "C" DarwinArtFdBrokerStatus
darwin_art_fd_broker_epoll_wait(DarwinArtFdBroker *broker, int epoll_fd,
                                DarwinArtFdEpollEvent *events, size_t capacity,
                                int timeout_ms, DarwinArtFdIoResult *result) {
  if (!broker || !events || !capacity || !result || timeout_ms < -1)
    return DARWIN_ART_FD_BROKER_INVALID_ARGUMENT;
  auto *impl = Impl(broker);
  Lease epoll;
  std::vector<Watch> watches;
  uint64_t generation = 0;
  {
    std::lock_guard lock(impl->mutex);
    auto s = AcquireLocked(impl, epoll_fd, DARWIN_ART_FD_EPOLL, &epoll);
    if (s != DARWIN_ART_FD_BROKER_OK)
      return s;
    watches = epoll.description->epoll->watches;
    generation = epoll.description->epoll->generation;
  }
  if (timeout_ms != 0 && !watches.empty()) {
    std::vector<std::shared_ptr<Description>> descriptions;
    std::vector<Watch> active_watches;
    std::vector<DarwinArtFdOwnerV1> callbacks;
    std::vector<uint64_t> objects;
    std::vector<int16_t> requested;
    descriptions.reserve(watches.size());
    active_watches.reserve(watches.size());
    callbacks.reserve(watches.size());
    objects.reserve(watches.size());
    requested.reserve(watches.size());
    {
      std::lock_guard lock(impl->mutex);
      for (const Watch &watch : watches) {
        if (!watch.enabled)
          continue;
        auto description = watch.description.lock();
        if (!AcquireDescriptionLocked(impl, description))
          continue;
        descriptions.push_back(description);
        active_watches.push_back(watch);
        callbacks.push_back(description->callbacks);
        objects.push_back(description->object);
        requested.push_back(static_cast<int16_t>(watch.events));
      }
    }
    bool homogeneous = !callbacks.empty() &&
                       callbacks[0].create_poll_wake != nullptr &&
                       callbacks[0].signal_poll_wake != nullptr &&
                       callbacks[0].close_poll_wake != nullptr &&
                       callbacks[0].poll_many_with_wake != nullptr;
    for (size_t index = 1; homogeneous && index < callbacks.size(); ++index) {
      homogeneous =
          callbacks[index].context == callbacks[0].context &&
          callbacks[index].create_poll_wake == callbacks[0].create_poll_wake &&
          callbacks[index].signal_poll_wake == callbacks[0].signal_poll_wake &&
          callbacks[index].close_poll_wake == callbacks[0].close_poll_wake &&
          callbacks[index].poll_many_with_wake ==
              callbacks[0].poll_many_with_wake;
    }
    if (homogeneous) {
      std::vector<int16_t> returned(callbacks.size());
      int error = 0;
      auto active_wake = std::make_shared<ActivePollWake>();
      active_wake->callbacks = callbacks[0];
      active_wake->object =
          callbacks[0].create_poll_wake(callbacks[0].context, &error);
      if (active_wake->object == 0) {
        std::lock_guard lock(impl->mutex);
        for (const auto &description : descriptions)
          ReleaseDescriptionLocked(impl, description);
        ReleaseLocked(impl, epoll);
        SetResult(result, -1, error);
        return DARWIN_ART_FD_BROKER_OK;
      }
      bool retry = false;
      {
        std::lock_guard lock(impl->mutex);
        if (epoll.description->epoll->generation != generation) {
          retry = true;
        } else {
          epoll.description->epoll->active_wakes.push_back(active_wake);
        }
      }
      if (retry) {
        active_wake->Close();
        {
          std::lock_guard lock(impl->mutex);
          for (const auto &description : descriptions)
            ReleaseDescriptionLocked(impl, description);
          ReleaseLocked(impl, epoll);
        }
        return darwin_art_fd_broker_epoll_wait(broker, epoll_fd, events,
                                               capacity, timeout_ms, result);
      }
      int woke = 0;
      const int polled = callbacks[0].poll_many_with_wake(
          callbacks[0].context, objects.data(), requested.data(),
          returned.data(), returned.size(), timeout_ms, active_wake->object,
          &woke, &error);
      size_t ready = 0;
      std::vector<int> consumed_one_shot;
      for (size_t index = 0;
           polled >= 0 && index < returned.size() && ready < capacity;
           ++index) {
        const uint32_t matched =
            static_cast<uint16_t>(returned[index]) &
            (active_watches[index].events | 0x0008u | 0x0010u);
        if (matched != 0) {
          events[ready++] =
              DarwinArtFdEpollEvent{matched, active_watches[index].data};
          if ((active_watches[index].events & kEpollOneShot) != 0)
            consumed_one_shot.push_back(active_watches[index].registration_fd);
        }
      }
      {
        std::lock_guard lock(impl->mutex);
        auto &active_wakes = epoll.description->epoll->active_wakes;
        active_wakes.erase(
            std::remove_if(active_wakes.begin(), active_wakes.end(),
                           [&](const auto &candidate) {
                             auto shared = candidate.lock();
                             return !shared || shared == active_wake;
                           }),
            active_wakes.end());
        for (int registration_fd : consumed_one_shot) {
          auto found = std::find_if(
              epoll.description->epoll->watches.begin(),
              epoll.description->epoll->watches.end(), [&](const Watch &watch) {
                return watch.registration_fd == registration_fd;
              });
          if (found != epoll.description->epoll->watches.end())
            found->enabled = false;
        }
        for (const auto &description : descriptions)
          ReleaseDescriptionLocked(impl, description);
        ReleaseLocked(impl, epoll);
      }
      active_wake->Close();
      if (polled >= 0 && woke != 0 && ready == 0)
        return darwin_art_fd_broker_epoll_wait(broker, epoll_fd, events,
                                               capacity, timeout_ms, result);
      SetResult(result, polled < 0 ? -1 : static_cast<intptr_t>(ready),
                polled < 0 ? error : 0);
      return DARWIN_ART_FD_BROKER_OK;
    }
    {
      std::lock_guard lock(impl->mutex);
      for (const auto &description : descriptions)
        ReleaseDescriptionLocked(impl, description);
      ReleaseLocked(impl, epoll);
    }
    return DARWIN_ART_FD_BROKER_UNSUPPORTED;
  }
  size_t ready = 0;
  int error = 0;
  for (const Watch &watch : watches) {
    if (ready == capacity)
      break;
    if (!watch.enabled)
      continue;
    auto d = watch.description.lock();
    DarwinArtFdOwnerV1 c{};
    uint64_t object = 0;
    {
      std::lock_guard lock(impl->mutex);
      if (!AcquireDescriptionLocked(impl, d))
        continue;
      c = d->callbacks;
      object = d->object;
    }
    int16_t revents = 0;
    int pr = c.poll
                 ? c.poll(c.context, object, static_cast<int16_t>(watch.events),
                          &revents, &error)
                 : 0;
    const uint32_t requested_events = watch.events | 0x0008u | 0x0010u;
    const uint32_t matched =
        pr < 0 ? 0 : static_cast<uint16_t>(revents) & requested_events;
    {
      std::lock_guard lock(impl->mutex);
      if (matched != 0 && (watch.events & kEpollOneShot) != 0) {
        auto found = std::find_if(epoll.description->epoll->watches.begin(),
                                  epoll.description->epoll->watches.end(),
                                  [&](const Watch &candidate) {
                                    return candidate.registration_fd ==
                                           watch.registration_fd;
                                  });
        if (found != epoll.description->epoll->watches.end())
          found->enabled = false;
      }
      ReleaseDescriptionLocked(impl, d);
    }
    if (pr < 0) {
      std::lock_guard lock(impl->mutex);
      ReleaseLocked(impl, epoll);
      SetResult(result, -1, error);
      return DARWIN_ART_FD_BROKER_OK;
    }
    if (matched)
      events[ready++] = DarwinArtFdEpollEvent{matched, watch.data};
  }
  {
    std::lock_guard lock(impl->mutex);
    ReleaseLocked(impl, epoll);
  }
  SetResult(result, static_cast<intptr_t>(ready), 0);
  return DARWIN_ART_FD_BROKER_OK;
}
