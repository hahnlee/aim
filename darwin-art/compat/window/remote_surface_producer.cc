#include "remote_surface_producer.h"

#include "../graphics/hardware_buffer_owner.h"

#include <android/hardware_buffer.h>
#include <binder/Binder.h>
#include <binder/Parcel.h>
#include <utils/Errors.h>

#include <cerrno>
#include <cstdint>
#include <atomic>
#include <memory>
#include <mutex>
#include <new>
#include <new>
#include <unordered_map>
#include <utility>

extern "C" int sync_wait(int, int);
extern "C" int darwin_art_bionic_socket_broker_close(int);
// The owner retires an imported/uncertain lease instead of making its slot
// immediately reusable. The declaration is kept here so this module remains
// independent of the facade header; the owner implementation is its narrow
// integration point.
extern "C" int darwin_art_android_ANativeWindow_remote_owner_quarantine(
    void*, int32_t, uint64_t, uint64_t, int);

namespace darwin_art::window {
namespace {

constexpr int32_t kMagic = 0x44525350;  // DRSP
constexpr int32_t kVersion = 1;
enum : uint32_t {
  kRegister = android::IBinder::FIRST_CALL_TRANSACTION,
  kDequeue,
  kImportAck,
  kQueue,
  kCancel,
  kPrepare,
  kSetPresentMode,
  kNextFrame,
  kRelease,
  kQuarantine,
};

void CloseFence(int fence) {
  if (fence >= 0) (void)darwin_art_bionic_socket_broker_close(fence);
}

// Binder FD transfer is deliberately not used until the transport has a
// verified ownership adapter. Waiting at each boundary is conservative but
// preserves the queue's real dependency semantics and avoids borrowing an FD
// whose process-local lifetime Binder cannot prove.
bool ConsumeFence(int fence) {
  if (fence < 0) return true;
  const bool signaled = sync_wait(fence, -1) == 0;
  CloseFence(fence);
  return signaled;
}

struct LeaseKey {
  int32_t slot = -1;
  uint64_t generation = 0;
  uint64_t lease = 0;
  bool operator==(const LeaseKey& other) const {
    return slot == other.slot && generation == other.generation &&
           lease == other.lease;
  }
};
struct LeaseHash {
  size_t operator()(const LeaseKey& key) const {
    size_t result = static_cast<size_t>(key.slot);
    result = result * 1315423911u ^ static_cast<size_t>(key.generation);
    result = result * 1315423911u ^ static_cast<size_t>(key.lease);
    return result;
  }
};

bool WriteIdentity(android::Parcel* parcel,
                   const DarwinArtHardwareBufferIdentity& identity) {
  if (parcel == nullptr) return false;
  const auto& desc = identity.description;
  return parcel->writeUint32(identity.surface_id) == android::NO_ERROR &&
         parcel->writeUint32(desc.width) == android::NO_ERROR &&
         parcel->writeUint32(desc.height) == android::NO_ERROR &&
         parcel->writeUint32(desc.layers) == android::NO_ERROR &&
         parcel->writeUint32(desc.format) == android::NO_ERROR &&
         parcel->writeUint64(desc.usage) == android::NO_ERROR &&
         parcel->writeUint32(desc.stride) == android::NO_ERROR;
}

bool ReadIdentity(const android::Parcel& parcel,
                  DarwinArtHardwareBufferIdentity* identity) {
  if (identity == nullptr) return false;
  identity->surface_id = parcel.readUint32();
  identity->description.width = parcel.readUint32();
  identity->description.height = parcel.readUint32();
  identity->description.layers = parcel.readUint32();
  identity->description.format = parcel.readUint32();
  identity->description.usage = parcel.readUint64();
  identity->description.stride = parcel.readUint32();
  return identity->surface_id != 0 && identity->description.width != 0 &&
         identity->description.height != 0 && identity->description.layers == 1;
}

bool WriteHeader(android::Parcel* parcel,
                 const android::sp<android::IBinder>& endpoint) {
  if (parcel == nullptr || endpoint == nullptr) return false;
  // Binder fixes the Parcel transport format before its first byte. Every
  // client transaction has a fresh Parcel; marking it after WriteHeader
  // aborts in AOSP Parcel::markForBinder.
  parcel->markForBinder(endpoint);
  return parcel->writeInt32(kMagic) == android::NO_ERROR &&
         parcel->writeInt32(kVersion) == android::NO_ERROR;
}

bool ReadHeader(const android::Parcel& parcel) {
  return parcel.readInt32() == kMagic && parcel.readInt32() == kVersion;
}

void WriteStatus(android::Parcel* reply, int status) {
  if (reply != nullptr) (void)reply->writeInt32(status);
}

class RemoteSurfaceProducerEndpoint;
class ClientDeathRecipient final : public android::IBinder::DeathRecipient {
 public:
  explicit ClientDeathRecipient(const android::wp<RemoteSurfaceProducerEndpoint>& endpoint)
      : endpoint_(endpoint) {}
  void binderDied(const android::wp<android::IBinder>&) override;

 private:
  android::wp<RemoteSurfaceProducerEndpoint> endpoint_;
};

class RemoteSurfaceProducerEndpoint final : public android::BBinder {
 public:
  explicit RemoteSurfaceProducerEndpoint(void* owner)
      : owner_(owner) {
    darwin_art_android_ANativeWindow_acquire(owner_);
  }

  ~RemoteSurfaceProducerEndpoint() override {
    CleanupAll();
    if (owner_ != nullptr) darwin_art_android_ANativeWindow_release(owner_);
  }

  android::status_t onTransact(uint32_t code, const android::Parcel& data,
                               android::Parcel* reply,
                               uint32_t flags) override {
    (void)flags;
    if (reply == nullptr || !ReadHeader(data)) return android::BAD_VALUE;
    switch (code) {
      case kRegister:
        return Register(data, reply);
      case kDequeue:
        return Dequeue(reply);
      case kImportAck:
        return ImportAck(data, reply);
      case kQueue:
        return Queue(data, reply);
      case kCancel:
        return Cancel(data, reply);
      case kPrepare:
        return Prepare(data, reply);
      case kSetPresentMode:
        return SetPresentMode(data, reply);
      case kNextFrame:
        return NextFrame(reply);
      case kRelease:
        CleanupAll();
        WriteStatus(reply, android::NO_ERROR);
        return android::NO_ERROR;
      case kQuarantine:
        return Quarantine(data, reply);
      default:
        return android::UNKNOWN_TRANSACTION;
    }
  }

 private:
  friend class ClientDeathRecipient;
  struct LeaseRecord {
    LeaseKey key;
    void* native_buffer = nullptr;
    AHardwareBuffer* buffer = nullptr;  // endpoint-owned +1
    bool imported = false;
  };

  android::status_t Register(const android::Parcel& data,
                             android::Parcel* reply) {
    android::sp<android::IBinder> token = data.readStrongBinder();
    if (token == nullptr) {
      WriteStatus(reply, -EINVAL);
      return android::NO_ERROR;
    }
    {
      std::lock_guard<std::mutex> lock(mutex_);
      if (registered_) {
        WriteStatus(reply, -EALREADY);
        return android::NO_ERROR;
      }
    }
    // A same-process Parcel copy carries a local BBinder, for which
    // linkToDeath is unsupported; ClientRelease still closes that lifetime.
    const bool local_client = token->localBinder() != nullptr;
    android::sp<ClientDeathRecipient> recipient;
    if (!local_client)
      recipient = android::sp<ClientDeathRecipient>::make(
          android::wp<RemoteSurfaceProducerEndpoint>(this));
    const android::status_t link_status =
        local_client ? android::NO_ERROR : token->linkToDeath(recipient);
    if (link_status != android::NO_ERROR) {
      WriteStatus(reply, link_status);
      return android::NO_ERROR;
    }
    {
      std::lock_guard<std::mutex> lock(mutex_);
      if (registered_) {
        if (!local_client)
          (void)token->unlinkToDeath(recipient);
        WriteStatus(reply, -EALREADY);
        return android::NO_ERROR;
      }
      registered_ = true;
      client_token_ = token;
      death_recipient_ = recipient;
    }
    WriteStatus(reply, android::NO_ERROR);
    return android::NO_ERROR;
  }

  android::status_t Dequeue(android::Parcel* reply) {
    {
      std::lock_guard<std::mutex> lock(mutex_);
      if (!registered_) {
        WriteStatus(reply, -EPERM);
        return android::NO_ERROR;
      }
    }
    DarwinArtRemoteOwnerDequeue owner{};
    int status = darwin_art_android_ANativeWindow_remote_owner_dequeue(
        owner_, &owner);
    if (status != 0) {
      WriteStatus(reply, status);
      return android::NO_ERROR;
    }
    DarwinArtHardwareBufferIdentity identity{};
    status = darwin_art_android_hardware_buffer_export_identity(owner.buffer,
                                                                &identity);
    if (status != 0 || owner.buffer == nullptr || owner.native_buffer == nullptr ||
        identity.surface_id == 0) {
      if (owner.buffer != nullptr) AHardwareBuffer_release(owner.buffer);
      (void)darwin_art_android_ANativeWindow_remote_owner_cancel(
          owner_, owner.slot, owner.generation, owner.lease, -1);
      WriteStatus(reply, status != 0 ? status : -EIO);
      return android::NO_ERROR;
    }
    if (!ConsumeFence(owner.acquire_fence)) {
      AHardwareBuffer_release(owner.buffer);
      (void)darwin_art_android_ANativeWindow_remote_owner_quarantine(
          owner_, owner.slot, owner.generation, owner.lease, -1);
      WriteStatus(reply, -EIO);
      return android::NO_ERROR;
    }
    LeaseRecord record{{owner.slot, owner.generation, owner.lease},
                       owner.native_buffer, owner.buffer, false};
    int admission = android::NO_ERROR;
    try {
      std::lock_guard<std::mutex> lock(mutex_);
      if (!registered_ || leases_.find(record.key) != leases_.end())
        admission = -EALREADY;
      else
        leases_.emplace(record.key, record);
    } catch (const std::bad_alloc&) {
      admission = -ENOMEM;
    }
    if (admission != android::NO_ERROR) {
      AHardwareBuffer_release(owner.buffer);
      (void)darwin_art_android_ANativeWindow_remote_owner_cancel(
          owner_, owner.slot, owner.generation, owner.lease, -1);
      WriteStatus(reply, admission);
      return android::NO_ERROR;
    }
    if (!WriteStatusAndIdentity(reply, record, identity)) {
      CleanupLease(record.key);
      return android::NO_ERROR;
    }
    return android::NO_ERROR;
  }

  bool WriteStatusAndIdentity(android::Parcel* reply, const LeaseRecord& record,
                              const DarwinArtHardwareBufferIdentity& identity) {
    return reply != nullptr && reply->writeInt32(android::NO_ERROR) == android::NO_ERROR &&
           reply->writeInt32(record.key.slot) == android::NO_ERROR &&
           reply->writeUint64(record.key.generation) == android::NO_ERROR &&
           reply->writeUint64(record.key.lease) == android::NO_ERROR &&
           WriteIdentity(reply, identity);
  }

  android::status_t ImportAck(const android::Parcel& data,
                              android::Parcel* reply) {
    LeaseKey key{data.readInt32(), data.readUint64(), data.readUint64()};
    const bool success = data.readInt32() != 0;
    bool known = false;
    {
      std::lock_guard<std::mutex> lock(mutex_);
      auto found = leases_.find(key);
      if (found != leases_.end() && !found->second.imported) {
        known = true;
        found->second.imported = success;
      }
    }
    if (known && !success) CleanupLease(key);
    WriteStatus(reply, known ? android::NO_ERROR : -EINVAL);
    return android::NO_ERROR;
  }

  android::status_t Queue(const android::Parcel& data,
                          android::Parcel* reply) {
    const LeaseKey key{data.readInt32(), data.readUint64(), data.readUint64()};
    const int32_t dataspace = data.readInt32();
    LeaseRecord record;
    if (!TakeImportedLease(key, &record)) {
      WriteStatus(reply, -EINVAL);
      return android::NO_ERROR;
    }
    const int status = darwin_art_android_ANativeWindow_remote_owner_queue(
        owner_, key.slot, key.generation, key.lease, record.native_buffer, -1,
        dataspace);
    if (status != 0) {
      (void)darwin_art_android_ANativeWindow_remote_owner_quarantine(
          owner_, key.slot, key.generation, key.lease, -1);
    }
    if (record.buffer != nullptr) AHardwareBuffer_release(record.buffer);
    WriteStatus(reply, status);
    return android::NO_ERROR;
  }

  android::status_t Cancel(const android::Parcel& data,
                           android::Parcel* reply) {
    const LeaseKey key{data.readInt32(), data.readUint64(), data.readUint64()};
    LeaseRecord record;
    if (!TakeLease(key, &record)) {
      WriteStatus(reply, -EINVAL);
      return android::NO_ERROR;
    }
    const int status = darwin_art_android_ANativeWindow_remote_owner_cancel(
        owner_, key.slot, key.generation, key.lease, -1);
    if (record.buffer != nullptr) AHardwareBuffer_release(record.buffer);
    WriteStatus(reply, status);
    return android::NO_ERROR;
  }

  android::status_t Quarantine(const android::Parcel& data,
                               android::Parcel* reply) {
    const LeaseKey key{data.readInt32(), data.readUint64(), data.readUint64()};
    LeaseRecord record;
    if (!TakeLease(key, &record)) {
      WriteStatus(reply, -EINVAL);
      return android::NO_ERROR;
    }
    const int status = darwin_art_android_ANativeWindow_remote_owner_quarantine(
        owner_, key.slot, key.generation, key.lease, -1);
    if (record.buffer != nullptr) AHardwareBuffer_release(record.buffer);
    WriteStatus(reply, status);
    return android::NO_ERROR;
  }

  android::status_t Prepare(const android::Parcel& data,
                            android::Parcel* reply) {
    const int32_t width = data.readInt32();
    const int32_t height = data.readInt32();
    WriteStatus(reply, darwin_art_android_ANativeWindow_prepare_swapchain(
                           owner_, width, height));
    return android::NO_ERROR;
  }

  android::status_t SetPresentMode(const android::Parcel& data,
                                   android::Parcel* reply) {
    WriteStatus(reply, darwin_art_android_ANativeWindow_set_present_mode(
                           owner_, data.readInt32()));
    return android::NO_ERROR;
  }

  android::status_t NextFrame(android::Parcel* reply) {
    if (reply == nullptr) return android::BAD_VALUE;
    (void)reply->writeInt32(android::NO_ERROR);
    (void)reply->writeUint64(
        darwin_art_android_ANativeWindow_next_frame_number(owner_));
    return android::NO_ERROR;
  }

  bool TakeImportedLease(LeaseKey key, LeaseRecord* out) {
    std::lock_guard<std::mutex> lock(mutex_);
    auto found = leases_.find(key);
    if (found == leases_.end() || !found->second.imported) return false;
    *out = found->second;
    leases_.erase(found);
    return true;
  }

  bool TakeLease(LeaseKey key, LeaseRecord* out) {
    std::lock_guard<std::mutex> lock(mutex_);
    auto found = leases_.find(key);
    if (found == leases_.end()) return false;
    *out = found->second;
    leases_.erase(found);
    return true;
  }

  void CleanupLease(LeaseKey key) {
    LeaseRecord record;
    if (!TakeLease(key, &record)) return;
    (void)darwin_art_android_ANativeWindow_remote_owner_cancel(
        owner_, key.slot, key.generation, key.lease, -1);
    if (record.buffer != nullptr) AHardwareBuffer_release(record.buffer);
  }

  void CleanupAll() {
    android::sp<android::IBinder> token;
    android::sp<ClientDeathRecipient> recipient;
    {
      std::lock_guard<std::mutex> lock(mutex_);
      token = client_token_.promote();
      recipient = death_recipient_;
      client_token_.clear();
      death_recipient_.clear();
      registered_ = false;
    }
    if (token != nullptr && recipient != nullptr)
      (void)token->unlinkToDeath(recipient);
    for (;;) {
      LeaseRecord record;
      {
        std::lock_guard<std::mutex> lock(mutex_);
        if (leases_.empty()) break;
        auto node = leases_.extract(leases_.begin());
        record = node.mapped();
      }
      if (record.imported) {
        (void)darwin_art_android_ANativeWindow_remote_owner_quarantine(
            owner_, record.key.slot, record.key.generation, record.key.lease, -1);
      } else {
        (void)darwin_art_android_ANativeWindow_remote_owner_cancel(
            owner_, record.key.slot, record.key.generation, record.key.lease, -1);
      }
      if (record.buffer != nullptr) AHardwareBuffer_release(record.buffer);
    }
  }

  void* owner_ = nullptr;
  std::mutex mutex_;
  bool registered_ = false;
  android::wp<android::IBinder> client_token_;
  android::sp<ClientDeathRecipient> death_recipient_;
  std::unordered_map<LeaseKey, LeaseRecord, LeaseHash> leases_;
};

void ClientDeathRecipient::binderDied(const android::wp<android::IBinder>&) {
  android::sp<RemoteSurfaceProducerEndpoint> endpoint = endpoint_.promote();
  if (endpoint != nullptr) endpoint->CleanupAll();
}

struct RemoteLease {
  LeaseKey key;
  AHardwareBuffer* buffer = nullptr;  // client-owned +1 until queue/cancel
};

struct RemoteClient final {
  android::sp<android::IBinder> endpoint;
  android::sp<android::BBinder> token;
  std::mutex mutex;
  std::unordered_map<void*, std::unique_ptr<RemoteLease>> leases;
};

bool Transact(const android::sp<android::IBinder>& endpoint, uint32_t code,
              android::Parcel* data, android::Parcel* reply) {
  if (endpoint == nullptr || data == nullptr || reply == nullptr) return false;
  return endpoint->transact(code, *data, reply, 0) == android::NO_ERROR;
}

bool RegisterClient(RemoteClient* client) {
  android::Parcel data;
  android::Parcel reply;
  if (!WriteHeader(&data, client->endpoint) ||
      data.writeStrongBinder(client->token) != android::NO_ERROR ||
      !Transact(client->endpoint, kRegister, &data, &reply)) return false;
  return reply.readInt32() == android::NO_ERROR;
}

int ClientRetireLease(RemoteClient* client, const LeaseKey& key,
                      uint32_t code);

int ClientDequeue(void* opaque, AHardwareBuffer** out_buffer,
                  void** out_native_buffer, int* out_fence) {
  auto* client = static_cast<RemoteClient*>(opaque);
  if (client == nullptr || out_buffer == nullptr || out_native_buffer == nullptr ||
      out_fence == nullptr) return -EINVAL;
  *out_buffer = nullptr;
  *out_native_buffer = nullptr;
  *out_fence = -1;
  android::Parcel data;
  android::Parcel reply;
  if (!WriteHeader(&data, client->endpoint) ||
      !Transact(client->endpoint, kDequeue, &data, &reply))
    return -EPIPE;
  const int status = reply.readInt32();
  if (status != android::NO_ERROR) return status;
  LeaseKey key{reply.readInt32(), reply.readUint64(), reply.readUint64()};
  DarwinArtHardwareBufferIdentity identity{};
  if (!ReadIdentity(reply, &identity)) {
    (void)ClientRetireLease(client, key, kCancel);
    return -EIO;
  }
  AHardwareBuffer* buffer = nullptr;
  int import_status = darwin_art_android_hardware_buffer_import_identity(
      &identity, &buffer);
  const bool imported = import_status == 0 && buffer != nullptr;
  android::Parcel ack;
  android::Parcel ack_reply;
  if (!WriteHeader(&ack, client->endpoint) ||
      ack.writeInt32(key.slot) != android::NO_ERROR ||
      ack.writeUint64(key.generation) != android::NO_ERROR ||
      ack.writeUint64(key.lease) != android::NO_ERROR ||
      ack.writeInt32(imported ? 1 : 0) !=
          android::NO_ERROR ||
      !Transact(client->endpoint, kImportAck, &ack, &ack_reply) ||
      ack_reply.readInt32() != android::NO_ERROR) {
    if (buffer != nullptr) AHardwareBuffer_release(buffer);
    (void)ClientRetireLease(client, key,
                            imported ? kQuarantine : kCancel);
    return import_status != 0 ? import_status : -EIO;
  }
  std::unique_ptr<RemoteLease> lease;
  try {
    lease = std::make_unique<RemoteLease>();
  } catch (const std::bad_alloc&) {
    (void)ClientRetireLease(client, key, kQuarantine);
    if (buffer != nullptr) AHardwareBuffer_release(buffer);
    return -ENOMEM;
  }
  lease->key = key;
  lease->buffer = buffer;
  void* native_buffer =
      darwin_art_android_hardware_buffer_native_window_buffer(buffer);
  if (native_buffer == nullptr) {
    (void)ClientRetireLease(client, key, kQuarantine);
    AHardwareBuffer_release(buffer);
    return -EIO;
  }
  bool inserted = false;
  try {
    std::lock_guard<std::mutex> lock(client->mutex);
    inserted = client->leases.emplace(native_buffer, std::move(lease)).second;
  } catch (const std::bad_alloc&) {
    (void)ClientRetireLease(client, key, kQuarantine);
    AHardwareBuffer_release(buffer);
    return -ENOMEM;
  }
  if (!inserted) {
    // The imported identity is live, but the client cannot represent a
    // second active alias safely. Retire the owner lease before dropping it.
    (void)ClientRetireLease(client, key, kQuarantine);
    AHardwareBuffer_release(buffer);
    return -EALREADY;
  }
  *out_buffer = buffer;
  *out_native_buffer = native_buffer;
  return 0;
}

int ClientRetireLease(RemoteClient* client, const LeaseKey& key,
                      uint32_t code) {
  android::Parcel data;
  android::Parcel reply;
  if (!WriteHeader(&data, client->endpoint) ||
      data.writeInt32(key.slot) != android::NO_ERROR ||
      data.writeUint64(key.generation) != android::NO_ERROR ||
      data.writeUint64(key.lease) != android::NO_ERROR ||
      !Transact(client->endpoint, code, &data, &reply))
    return -EPIPE;
  return reply.readInt32();
}

int ClientQueue(void* opaque, void* native_buffer, int fence,
                int32_t dataspace) {
  auto* client = static_cast<RemoteClient*>(opaque);
  if (client == nullptr) {
    CloseFence(fence);
    return -EINVAL;
  }
  std::unique_ptr<RemoteLease> lease;
  {
    std::lock_guard<std::mutex> lock(client->mutex);
    auto found = client->leases.find(native_buffer);
    if (found == client->leases.end()) {
      CloseFence(fence);
      return -EINVAL;
    }
    lease = std::move(found->second);
    client->leases.erase(found);
  }
  if (fence < 0 || !ConsumeFence(fence)) {
    (void)ClientRetireLease(client, lease->key, kQuarantine);
    if (lease->buffer != nullptr) AHardwareBuffer_release(lease->buffer);
    return -EIO;
  }
  android::Parcel data;
  android::Parcel reply;
  const bool sent = WriteHeader(&data, client->endpoint) &&
                    data.writeInt32(lease->key.slot) == android::NO_ERROR &&
                    data.writeUint64(lease->key.generation) == android::NO_ERROR &&
                    data.writeUint64(lease->key.lease) == android::NO_ERROR &&
                    data.writeInt32(dataspace) == android::NO_ERROR &&
                    Transact(client->endpoint, kQueue, &data, &reply);
  const int status = sent ? reply.readInt32() : -EPIPE;
  if (!sent) (void)ClientRetireLease(client, lease->key, kQuarantine);
  if (lease->buffer != nullptr) AHardwareBuffer_release(lease->buffer);
  return status;
}

int ClientCancel(void* opaque, void* native_buffer, int fence) {
  auto* client = static_cast<RemoteClient*>(opaque);
  if (client == nullptr) {
    CloseFence(fence);
    return -EINVAL;
  }
  std::unique_ptr<RemoteLease> lease;
  {
    std::lock_guard<std::mutex> lock(client->mutex);
    auto found = client->leases.find(native_buffer);
    if (found == client->leases.end()) {
      CloseFence(fence);
      return -EINVAL;
    }
    lease = std::move(found->second);
    client->leases.erase(found);
  }
  // The native-window -1 convention means the caller has no pending fence.
  // Vulkan uses it when returning its pre-render swapchain imports. A supplied
  // fence must complete before this owner lease can become reusable.
  const bool fence_ok = fence == -1 || (fence >= 0 && ConsumeFence(fence));
  const int status = ClientRetireLease(
      client, lease->key, fence_ok ? kCancel : kQuarantine);
  if (lease->buffer != nullptr) AHardwareBuffer_release(lease->buffer);
  return fence_ok ? status : -EIO;
}

int ClientPrepare(void* opaque, int32_t width, int32_t height) {
  auto* client = static_cast<RemoteClient*>(opaque);
  if (client == nullptr) return -EINVAL;
  android::Parcel data;
  android::Parcel reply;
  if (!WriteHeader(&data, client->endpoint) ||
      data.writeInt32(width) != android::NO_ERROR ||
      data.writeInt32(height) != android::NO_ERROR ||
      !Transact(client->endpoint, kPrepare, &data, &reply)) return -EPIPE;
  return reply.readInt32();
}

int ClientPresentMode(void* opaque, int32_t mode) {
  auto* client = static_cast<RemoteClient*>(opaque);
  if (client == nullptr) return -EINVAL;
  android::Parcel data;
  android::Parcel reply;
  if (!WriteHeader(&data, client->endpoint) ||
      data.writeInt32(mode) != android::NO_ERROR ||
      !Transact(client->endpoint, kSetPresentMode, &data, &reply)) return -EPIPE;
  return reply.readInt32();
}

uint64_t ClientNextFrame(void* opaque) {
  auto* client = static_cast<RemoteClient*>(opaque);
  if (client == nullptr) return 0;
  android::Parcel data;
  android::Parcel reply;
  if (!WriteHeader(&data, client->endpoint) ||
      !Transact(client->endpoint, kNextFrame, &data, &reply) ||
      reply.readInt32() != android::NO_ERROR)
    return 0;
  return reply.readUint64();
}

int ClientPrepareThunk(void* opaque, int32_t width, int32_t height) {
  return ClientPrepare(opaque, width, height);
}

void ClientRelease(void* opaque) {
  auto* client = static_cast<RemoteClient*>(opaque);
  if (client == nullptr) return;
  android::Parcel data;
  android::Parcel reply;
  if (WriteHeader(&data, client->endpoint))
    (void)Transact(client->endpoint, kRelease, &data, &reply);
  for (;;) {
    std::unique_ptr<RemoteLease> lease;
    {
      std::lock_guard<std::mutex> lock(client->mutex);
      if (client->leases.empty()) break;
      auto found = client->leases.begin();
      lease = std::move(found->second);
      client->leases.erase(found);
    }
    if (lease->buffer != nullptr) AHardwareBuffer_release(lease->buffer);
  }
  delete client;
}

}  // namespace

android::sp<android::IBinder> CreateRemoteSurfaceProducerEndpoint(
    void* owner_native_window) {
  if (owner_native_window == nullptr) return nullptr;
  return android::sp<RemoteSurfaceProducerEndpoint>::make(owner_native_window);
}

bool CreateRemoteSurfaceProducerClient(
    const android::sp<android::IBinder>& endpoint,
    DarwinArtRemoteNativeWindowHooks* out_hooks) {
  if (endpoint == nullptr || out_hooks == nullptr) return false;
  auto* client = new (std::nothrow) RemoteClient;
  if (client == nullptr) return false;
  client->endpoint = endpoint;
  client->token = android::sp<android::BBinder>::make();
  if (client->token == nullptr || !RegisterClient(client)) {
    delete client;
    return false;
  }
  *out_hooks = DarwinArtRemoteNativeWindowHooks{
      .context = client,
      .release = &ClientRelease,
      .dequeue = &ClientDequeue,
      .queue = &ClientQueue,
      .cancel = &ClientCancel,
      .prepare = &ClientPrepareThunk,
      .set_present_mode = &ClientPresentMode,
      .next_frame = &ClientNextFrame,
  };
  return true;
}

}  // namespace darwin_art::window
