#include "egl_native_fence_owner.h"

#include <atomic>
#include <iostream>
#include <memory>
#include <mutex>
#include <new>
#include <unordered_map>
#include <unistd.h>

namespace darwin_art::graphics {
namespace {

#if defined(DARWIN_ART_EGL_NATIVE_FENCE_OWNER_TESTING)
std::atomic<int> g_record_allocation_failures{0};
std::atomic<int> g_entry_allocation_failures{0};
std::atomic<int> g_index_allocation_failures{0};

bool ConsumeFailure(std::atomic<int>& failures) noexcept {
  int remaining = failures.load(std::memory_order_acquire);
  while (remaining > 0 &&
         !failures.compare_exchange_weak(remaining, remaining - 1,
                                         std::memory_order_acq_rel,
                                         std::memory_order_acquire)) {
  }
  return remaining > 0;
}
#endif

constexpr EglNativeFenceInt kEglNone = 0x3038;
constexpr EglNativeFenceEnum kEglSyncNativeFenceAndroid = 0x3144;
constexpr EglNativeFenceInt kEglSyncNativeFenceFdAndroid = 0x3145;
constexpr EglNativeFenceInt kEglNoNativeFenceFdAndroid = -1;
constexpr EglNativeFenceEnum kEglFenceSync = 0x30F9;
constexpr EglNativeFenceEnum kEglSyncMetalSharedEventAngle = 0x34D8;
constexpr EglNativeFenceInt kEglSyncMetalSharedEventObjectAngle = 0x34D9;
constexpr EglNativeFenceInt kEglSyncMetalSharedEventSignalValueLoAngle = 0x34DA;
constexpr EglNativeFenceInt kEglSyncMetalSharedEventSignalValueHiAngle = 0x34DB;
constexpr EglNativeFenceInt kEglDeviceExt = 0x322C;
constexpr EglNativeFenceInt kEglMetalDeviceAngle = 0x34A6;

struct Entry {
  EglNativeFenceDisplay display = nullptr;
  EglNativeFenceSync angle_sync = nullptr;
  void* metal_shared_event = nullptr;
  std::uint64_t signal_value = 0;
  EglNativeFenceBackend cleanup_backend;

  // Registry removal marks an entry retiring. Cleanup waits for callers that
  // already acquired it through the admission count, keeping ANGLE syncs and
  // Metal events alive for every in-flight wait/query/dup operation without
  // exposing the map.
  std::mutex mutex;
  std::size_t active_operations = 0;
  bool retiring = false;
  bool cleanup_started = false;
};

void CleanupEntry(const std::shared_ptr<Entry>& entry) noexcept;

void CleanupUnpublishedEntry(const std::shared_ptr<Entry>& entry) noexcept {
  if (entry == nullptr) return;
  const auto backend = entry->cleanup_backend;
  try {
    if (backend.destroy_sync != nullptr && entry->angle_sync != nullptr)
      (void)backend.destroy_sync(entry->display, entry->angle_sync);
  } catch (...) {
  }
  try {
    if (backend.metal_shared_event_release != nullptr &&
        entry->metal_shared_event != nullptr)
      backend.metal_shared_event_release(entry->metal_shared_event);
  } catch (...) {
  }
}

class ImportedFenceFd {
 public:
  ImportedFenceFd(const EglNativeFenceBackend& backend,
                  EglNativeFenceInt fd)
      : backend_(backend), fd_(fd) {}
  ImportedFenceFd(const ImportedFenceFd&) = delete;
  ImportedFenceFd& operator=(const ImportedFenceFd&) = delete;
  ~ImportedFenceFd() noexcept { Close(); }

  void Close() noexcept {
    const EglNativeFenceInt fd = fd_;
    fd_ = kEglNoNativeFenceFdAndroid;
    if (fd < 0 || backend_.close_fence_fd == nullptr) return;
    try {
      (void)backend_.close_fence_fd(fd);
    } catch (...) {
    }
  }

 private:
  const EglNativeFenceBackend& backend_;
  EglNativeFenceInt fd_;
};

class Lease {
 public:
  Lease() = default;
  explicit Lease(std::shared_ptr<Entry> entry) : entry_(std::move(entry)) {}
  Lease(const Lease&) = delete;
  Lease& operator=(const Lease&) = delete;
  Lease(Lease&& other) noexcept : entry_(std::move(other.entry_)) {}
  Lease& operator=(Lease&& other) noexcept {
    if (this != &other) {
      Reset();
      entry_ = std::move(other.entry_);
    }
    return *this;
  }
  ~Lease() { Reset(); }

  Entry* operator->() const { return entry_.get(); }
  explicit operator bool() const { return entry_ != nullptr; }

 private:
  void Reset() {
    if (entry_ == nullptr) return;
    auto entry = std::move(entry_);
    bool cleanup = false;
    {
      std::lock_guard<std::mutex> lock(entry->mutex);
      if (--entry->active_operations == 0) {
        cleanup = entry->retiring && !entry->cleanup_started;
      }
    }
    if (cleanup) CleanupEntry(entry);
  }

  std::shared_ptr<Entry> entry_;
};

class Registry {
 public:
  struct Record {
    std::shared_ptr<Entry> entry;
    bool retired = false;
  };

  Record* AllocateRecord() noexcept {
#if defined(DARWIN_ART_EGL_NATIVE_FENCE_OWNER_TESTING)
    if (ConsumeFailure(g_record_allocation_failures)) return nullptr;
#endif
    return new (std::nothrow) Record();
  }

  void DiscardUnpublishedRecord(Record* record) noexcept { delete record; }

  bool Publish(Record* record, std::shared_ptr<Entry>& entry) {
    std::lock_guard<std::mutex> registry_lock(mutex_);
    // The index is permanent: no erase-on-retire means a stale translated
    // token can never become an unknown ANGLE token, while lookup remains
    // bounded by the hash table rather than all historical fences.
    try {
#if defined(DARWIN_ART_EGL_NATIVE_FENCE_OWNER_TESTING)
      if (ConsumeFailure(g_index_allocation_failures)) return false;
#endif
      if (!index_.emplace(record, record).second) return false;
    } catch (...) {
      return false;
    }
    record->entry = std::move(entry);
    return true;
  }

  Lease Acquire(EglNativeFenceSync sync) {
    std::lock_guard<std::mutex> registry_lock(mutex_);
    auto* record = FindLocked(sync);
    if (record == nullptr || record->retired || record->entry == nullptr)
      return {};
    auto entry = record->entry;
    std::lock_guard<std::mutex> entry_lock(entry->mutex);
    if (entry->retiring) return {};
    ++entry->active_operations;
    return Lease(std::move(entry));
  }

  std::shared_ptr<Entry> Remove(EglNativeFenceSync sync) {
    std::lock_guard<std::mutex> registry_lock(mutex_);
    auto* record = FindLocked(sync);
    if (record == nullptr || record->retired || record->entry == nullptr)
      return {};
    auto entry = std::move(record->entry);
    record->retired = true;
    std::lock_guard<std::mutex> entry_lock(entry->mutex);
    entry->retiring = true;
    return entry;
  }

  bool IsRetired(EglNativeFenceSync sync) {
    std::lock_guard<std::mutex> registry_lock(mutex_);
    auto* record = FindLocked(sync);
    return record != nullptr && record->retired;
  }

 private:
  Record* FindLocked(EglNativeFenceSync sync) const noexcept {
    const auto found = index_.find(sync);
    return found == index_.end() ? nullptr : found->second;
  }

  std::mutex mutex_;
  std::unordered_map<EglNativeFenceSync, Record*> index_;
};

Registry& NativeFenceRegistry() {
  static Registry registry;
  return registry;
}

void CleanupEntry(const std::shared_ptr<Entry>& entry) noexcept {
  EglNativeFenceBackend backend;
  {
    std::lock_guard<std::mutex> lock(entry->mutex);
    if (entry->cleanup_started || !entry->retiring ||
        entry->active_operations != 0)
      return;
    entry->cleanup_started = true;
    backend = entry->cleanup_backend;
  }
  // Never invoke provider code while either registry or entry locks are held.
  try {
    if (backend.destroy_sync != nullptr)
      (void)backend.destroy_sync(entry->display, entry->angle_sync);
  } catch (...) {
  }
  try {
    if (entry->metal_shared_event != nullptr &&
        backend.metal_shared_event_release != nullptr)
      backend.metal_shared_event_release(entry->metal_shared_event);
  } catch (...) {
  }
}

EglNativeFenceInt NativeFenceAttributeFd(const EglNativeFenceInt* attributes) {
  if (attributes == nullptr) return kEglNoNativeFenceFdAndroid;
  for (const EglNativeFenceInt* attribute = attributes;
       attribute[0] != kEglNone; attribute += 2) {
    if (attribute[0] == kEglSyncNativeFenceFdAndroid) return attribute[1];
  }
  return kEglNoNativeFenceFdAndroid;
}

void LogCreate(const EglNativeFenceBackend& backend,
               EglNativeFenceEnum type, bool translated, bool acquire,
               EglNativeFenceInt imported_fd, EglNativeFenceSync sync,
               const Entry* entry) noexcept {
  if (!backend.debug) return;
  try {
    std::cerr << "ART Android EGL: eglCreateSyncKHR pid=" << getpid()
              << " type=0x" << std::hex << type << std::dec
              << " translated=" << translated << " acquire=" << acquire
              << " imported_fd=" << imported_fd << " sync=" << sync
              << " angle_sync="
              << (entry == nullptr ? nullptr : entry->angle_sync)
              << " shared_event="
              << (entry == nullptr ? nullptr : entry->metal_shared_event)
              << "\n";
  } catch (...) {
  }
}

}  // namespace

#if defined(DARWIN_ART_EGL_NATIVE_FENCE_OWNER_TESTING)
void SetNativeFenceOwnerAllocationFailuresForTest(int record_failures,
                                                  int entry_failures,
                                                  int index_failures) noexcept {
  g_record_allocation_failures.store(record_failures,
                                     std::memory_order_release);
  g_entry_allocation_failures.store(entry_failures,
                                    std::memory_order_release);
  g_index_allocation_failures.store(index_failures,
                                    std::memory_order_release);
}
#endif

EglNativeFenceSync CreateNativeFenceSync(
    EglNativeFenceDisplay display, EglNativeFenceEnum type,
    const EglNativeFenceInt* attributes,
    const EglNativeFenceBackend& backend) {
  const bool native_fence = type == kEglSyncNativeFenceAndroid;
  if (!native_fence) {
    EglNativeFenceSync sync = nullptr;
    try {
      sync = backend.create_sync_khr == nullptr
                 ? nullptr
                 : backend.create_sync_khr(display, type, attributes);
    } catch (...) {
      return nullptr;
    }
    LogCreate(backend, type, false, false, kEglNoNativeFenceFdAndroid, sync,
              nullptr);
    return sync;
  }

  const EglNativeFenceInt imported_fd = NativeFenceAttributeFd(attributes);
  const bool acquire_fence = imported_fd >= 0;
  ImportedFenceFd imported_fd_owner(backend, imported_fd);
  if (acquire_fence) {
    // EGL takes ownership of EGL_SYNC_NATIVE_FENCE_FD_ANDROID.  Always close
    // the broker descriptor, including when the wait reports an error.
    try {
      if (backend.sync_wait != nullptr)
        (void)backend.sync_wait(imported_fd, -1);
      if (backend.synchronize_iosurface_to_ahb != nullptr)
        backend.synchronize_iosurface_to_ahb();
    } catch (...) {
      return nullptr;
    }
  } else if (backend.synchronize_ahb_to_iosurface != nullptr) {
    try {
      backend.synchronize_ahb_to_iosurface();
    } catch (...) {
      return nullptr;
    }
  }

  // Allocate the permanent token record before creating ANGLE/Metal objects;
  // allocation failure therefore cannot leak an imported fence or resource.
  Registry& registry = NativeFenceRegistry();
  Registry::Record* record = registry.AllocateRecord();
  if (record == nullptr) return nullptr;
  std::shared_ptr<Entry> entry;
  try {
#if defined(DARWIN_ART_EGL_NATIVE_FENCE_OWNER_TESTING)
    if (ConsumeFailure(g_entry_allocation_failures)) throw std::bad_alloc();
#endif
    entry = std::make_shared<Entry>();
    entry->display = display;
    entry->cleanup_backend = backend;
    if (acquire_fence) {
      // Preserve EGL query/wait behavior after the imported Android fence has
      // established the cross-process completion boundary.
      entry->angle_sync = backend.create_sync_khr == nullptr
                              ? nullptr
                              : backend.create_sync_khr(display, kEglFenceSync,
                                                         nullptr);
    } else if (backend.query_display_attrib != nullptr &&
               backend.query_device_attrib != nullptr &&
               backend.create_sync != nullptr) {
      EglNativeFenceAttrib egl_device = 0;
      EglNativeFenceAttrib metal_device = 0;
      if (backend.query_display_attrib(display, kEglDeviceExt, &egl_device) != 0 &&
          backend.query_device_attrib(reinterpret_cast<void*>(egl_device),
                                      kEglMetalDeviceAngle, &metal_device) != 0) {
        entry->metal_shared_event =
            backend.metal_shared_event_create == nullptr
                ? nullptr
                : backend.metal_shared_event_create(
                      reinterpret_cast<void*>(metal_device),
                      &entry->signal_value);
        if (entry->metal_shared_event != nullptr) {
          const EglNativeFenceAttrib event_value =
              reinterpret_cast<EglNativeFenceAttrib>(
                  entry->metal_shared_event);
          const EglNativeFenceAttrib event_attributes[] = {
              kEglSyncMetalSharedEventObjectAngle, event_value,
              kEglSyncMetalSharedEventSignalValueLoAngle,
              static_cast<EglNativeFenceAttrib>(entry->signal_value & UINT32_MAX),
              kEglSyncMetalSharedEventSignalValueHiAngle,
              static_cast<EglNativeFenceAttrib>(entry->signal_value >> 32),
              kEglNone};
          entry->angle_sync = backend.create_sync(
              display, kEglSyncMetalSharedEventAngle, event_attributes);
        }
      }
    }
  } catch (...) {
    CleanupUnpublishedEntry(entry);
    registry.DiscardUnpublishedRecord(record);
    return nullptr;
  }

  if (entry->angle_sync == nullptr) {
    CleanupUnpublishedEntry(entry);
    registry.DiscardUnpublishedRecord(record);
    LogCreate(backend, type, true, acquire_fence, imported_fd, nullptr,
              nullptr);
    return nullptr;
  }
  const EglNativeFenceSync token = record;
  LogCreate(backend, type, true, acquire_fence, imported_fd, token,
            entry.get());
  // Publication inserts the permanent index before moving the Entry. Index
  // allocation failure leaves the Entry unpublished for explicit cleanup;
  // once successful, retirement is a lock-only state update.
  try {
    if (!registry.Publish(record, entry)) {
      CleanupUnpublishedEntry(entry);
      registry.DiscardUnpublishedRecord(record);
      return nullptr;
    }
  } catch (...) {
    CleanupUnpublishedEntry(entry);
    registry.DiscardUnpublishedRecord(record);
    return nullptr;
  }
  imported_fd_owner.Close();
  return token;
}

EglNativeFenceBoolean DestroyNativeFenceSync(
    EglNativeFenceDisplay display, EglNativeFenceSync sync,
    const EglNativeFenceBackend& backend) {
  auto entry = NativeFenceRegistry().Remove(sync);
  if (entry == nullptr) {
    if (NativeFenceRegistry().IsRetired(sync)) {
      if (backend.debug)
        std::cerr << "ART Android EGL: eglDestroySyncKHR pid=" << getpid()
                  << " sync=" << sync << " translated=1 stale=1\n";
      return 0;
    }
    const auto result = backend.destroy_sync_khr == nullptr
                            ? 0
                            : backend.destroy_sync_khr(display, sync);
    if (backend.debug)
      std::cerr << "ART Android EGL: eglDestroySyncKHR pid=" << getpid()
                << " sync=" << sync << " translated=0 result=" << result
                << "\n";
    return result;
  }
  CleanupEntry(entry);
  if (backend.debug)
    std::cerr << "ART Android EGL: eglDestroySyncKHR pid=" << getpid()
              << " sync=" << sync << " translated=1\n";
  return 1;
}

EglNativeFenceInt ClientWaitNativeFenceSync(
    EglNativeFenceDisplay display, EglNativeFenceSync sync,
    EglNativeFenceInt flags, std::uint64_t timeout,
    const EglNativeFenceBackend& backend) {
  auto entry = NativeFenceRegistry().Acquire(sync);
  if (entry) {
    return backend.client_wait_sync == nullptr
               ? 0
               : backend.client_wait_sync(display, entry->angle_sync, flags,
                                          timeout);
  }
  if (NativeFenceRegistry().IsRetired(sync)) return 0;
  return backend.client_wait_sync_khr == nullptr
             ? 0
             : backend.client_wait_sync_khr(display, sync, flags, timeout);
}

EglNativeFenceBoolean WaitNativeFenceSync(
    EglNativeFenceDisplay display, EglNativeFenceSync sync,
    EglNativeFenceInt flags, const EglNativeFenceBackend& backend) {
  auto entry = NativeFenceRegistry().Acquire(sync);
  if (entry) {
    return backend.wait_sync == nullptr
               ? 0
               : backend.wait_sync(display, entry->angle_sync, flags);
  }
  if (NativeFenceRegistry().IsRetired(sync)) return 0;
  return backend.wait_sync_khr == nullptr
             ? 0
             : backend.wait_sync_khr(display, sync, flags);
}

EglNativeFenceBoolean GetNativeFenceSyncAttrib(
    EglNativeFenceDisplay display, EglNativeFenceSync sync,
    EglNativeFenceInt attribute, EglNativeFenceInt* value,
    const EglNativeFenceBackend& backend) {
  auto entry = NativeFenceRegistry().Acquire(sync);
  if (entry) {
    if (value == nullptr || backend.get_sync_attrib == nullptr) return 0;
    EglNativeFenceAttrib wide = 0;
    if (backend.get_sync_attrib(display, entry->angle_sync, attribute, &wide) ==
        0)
      return 0;
    *value = static_cast<EglNativeFenceInt>(wide);
    return 1;
  }
  if (NativeFenceRegistry().IsRetired(sync)) return 0;
  return backend.get_sync_attrib_khr == nullptr
             ? 0
             : [&] {
                 if (value == nullptr) return EglNativeFenceBoolean{0};
                 const auto result = backend.get_sync_attrib_khr(
                     display, sync, attribute, value);
                 return result;
               }();
}

EglNativeFenceInt DupNativeFenceFd(
    EglNativeFenceDisplay display, EglNativeFenceSync sync,
    const EglNativeFenceBackend& backend) {
  auto entry = NativeFenceRegistry().Acquire(sync);
  if (entry) {
    const EglNativeFenceInt result =
        entry->metal_shared_event == nullptr ||
                backend.metal_shared_event_fence_fd == nullptr
            ? kEglNoNativeFenceFdAndroid
            : backend.metal_shared_event_fence_fd(entry->metal_shared_event,
                                                  entry->signal_value);
    if (backend.debug)
      std::cerr << "ART Android EGL: eglDupNativeFenceFDANDROID pid="
                << getpid() << " sync=" << sync << " translated=1 result="
                << result << "\n";
    return result;
  }
  if (NativeFenceRegistry().IsRetired(sync)) return kEglNoNativeFenceFdAndroid;
  return backend.dup_native_fence_fd_khr == nullptr
             ? kEglNoNativeFenceFdAndroid
             : backend.dup_native_fence_fd_khr(display, sync);
}

EglNativeFenceExport ExportNativeFence(
    EglNativeFenceDisplay display, const EglNativeFenceBackend& backend) {
  const auto sync = CreateNativeFenceSync(display, kEglSyncNativeFenceAndroid,
                                          nullptr, backend);
  if (sync == nullptr) return {};
  auto entry = NativeFenceRegistry().Acquire(sync);
  if (!entry) {
    (void)DestroyNativeFenceSync(display, sync, backend);
    return {};
  }
  return {.token = sync,
          .shared_event = entry->metal_shared_event,
          .signal_value = entry->signal_value};
}

void ReleaseNativeFence(EglNativeFenceDisplay display,
                        EglNativeFenceSync token,
                        const EglNativeFenceBackend& backend) {
  if (token != nullptr) (void)DestroyNativeFenceSync(display, token, backend);
}

}  // namespace darwin_art::graphics
