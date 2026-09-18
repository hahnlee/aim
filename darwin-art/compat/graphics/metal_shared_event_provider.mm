#include "metal_shared_event_provider.h"

#include "../../tools/bionic-socket-broker-adapter/include/darwin_art_bionic_socket_broker.h"

#import <Foundation/Foundation.h>
#import <Metal/Metal.h>

#include <atomic>
#include <cerrno>
#include <cstdint>
#include <limits>
#include <memory>
#include <mutex>
#include <new>
#include <thread>
#include <unordered_map>
#include <utility>

extern "C" int sync_wait(int fd, int timeout_ms);

namespace {

constexpr std::uint64_t kSignaledMetalFence =
    UINT64_C(0x44415257494e4653);  // "DARWINFS"

void CloseDescriptor(int* descriptor) noexcept {
  if (descriptor != nullptr && *descriptor >= 0) {
    (void)darwin_art_bionic_socket_broker_close(*descriptor);
    *descriptor = -1;
  }
}

class OwnedDescriptor final {
 public:
  explicit OwnedDescriptor(int descriptor = -1) noexcept
      : descriptor_(descriptor) {}
  OwnedDescriptor(const OwnedDescriptor&) = delete;
  OwnedDescriptor& operator=(const OwnedDescriptor&) = delete;
  OwnedDescriptor(OwnedDescriptor&& other) noexcept
      : descriptor_(std::exchange(other.descriptor_, -1)) {}
  OwnedDescriptor& operator=(OwnedDescriptor&& other) noexcept {
    if (this != &other) {
      reset();
      descriptor_ = std::exchange(other.descriptor_, -1);
    }
    return *this;
  }
  ~OwnedDescriptor() { reset(); }

  int get() const noexcept { return descriptor_; }
  int release() noexcept { return std::exchange(descriptor_, -1); }
  void reset() noexcept { CloseDescriptor(&descriptor_); }

 private:
  int descriptor_;
};

class DarwinArtMetalSharedEventImpl;

class FenceNotification final {
 public:
  FenceNotification(id<MTLSharedEvent> event, int write_descriptor,
                    int completion_read_guard) noexcept
      : event_(event),
        write_descriptor_(write_descriptor),
        completion_read_guard_(completion_read_guard) {
    [event_ retain];
  }
  FenceNotification(const FenceNotification&) = delete;
  FenceNotification& operator=(const FenceNotification&) = delete;
  ~FenceNotification() {
    CloseDescriptor(&write_descriptor_);
    CloseDescriptor(&completion_read_guard_);
    [event_ release];
  }

  void Complete() noexcept {
    bool expected = false;
    if (!completed_.compare_exchange_strong(expected, true,
                                            std::memory_order_acq_rel)) {
      return;
    }
    if (write_descriptor_ >= 0) {
      (void)darwin_art_bionic_socket_broker_write(
          write_descriptor_, &kSignaledMetalFence,
          sizeof(kSignaledMetalFence));
    }
    CloseDescriptor(&write_descriptor_);
    CloseDescriptor(&completion_read_guard_);
  }

 private:
  id<MTLSharedEvent> event_;
  int write_descriptor_ = -1;
  int completion_read_guard_ = -1;
  std::atomic<bool> completed_{false};
};

class FenceImportWork final {
 public:
  FenceImportWork(id<MTLSharedEvent> event, std::uint64_t signal_value,
                  OwnedDescriptor fence) noexcept
      : event_(event), signal_value_(signal_value), fence_(std::move(fence)) {
    [event_ retain];
  }
  FenceImportWork(const FenceImportWork&) = delete;
  FenceImportWork& operator=(const FenceImportWork&) = delete;
  ~FenceImportWork() {
    fence_.reset();
    [event_ release];
  }

  void Run() noexcept {
    const int wait_result = sync_wait(fence_.get(), -1);
    fence_.reset();
    if (wait_result == 0 && event_.signaledValue < signal_value_)
      event_.signaledValue = signal_value_;
  }

 private:
  id<MTLSharedEvent> event_;
  std::uint64_t signal_value_;
  OwnedDescriptor fence_;
};

class DarwinArtMetalSharedEventImpl {
 public:
  explicit DarwinArtMetalSharedEventImpl(id<MTLSharedEvent> event) noexcept
      : event_(event) {}
  DarwinArtMetalSharedEventImpl(const DarwinArtMetalSharedEventImpl&) = delete;
  DarwinArtMetalSharedEventImpl& operator=(
      const DarwinArtMetalSharedEventImpl&) = delete;
  ~DarwinArtMetalSharedEventImpl() { [event_ release]; }

  int FenceFd(std::uint64_t signal_value) noexcept {
    if (signal_value == 0) return -1;
    std::int32_t descriptors[2]{-1, -1};
    if (darwin_art_bionic_socket_broker_pipe(descriptors) != 0) {
      CloseDescriptor(&descriptors[0]);
      CloseDescriptor(&descriptors[1]);
      return -1;
    }
    OwnedDescriptor read_descriptor(descriptors[0]);
    OwnedDescriptor write_descriptor(descriptors[1]);
    if (event_.signaledValue >= signal_value) {
      const intptr_t written = darwin_art_bionic_socket_broker_write(
          write_descriptor.get(), &kSignaledMetalFence,
          sizeof(kSignaledMetalFence));
      if (written != static_cast<intptr_t>(sizeof(kSignaledMetalFence)))
        return -1;
      write_descriptor.reset();
      return read_descriptor.release();
    }

    const int completion_read_guard =
        darwin_art_bionic_socket_broker_dup(read_descriptor.get());
    if (completion_read_guard < 0) return -1;
    OwnedDescriptor guard(completion_read_guard);
    std::shared_ptr<FenceNotification> notification;
    try {
      // Keep both descriptor owners intact until the callback state is
      // successfully allocated. This makes allocation failure close every
      // descriptor through the ordinary RAII path.
      notification = std::make_shared<FenceNotification>(
          event_, write_descriptor.get(), guard.get());
    } catch (...) {
      return -1;
    }
    write_descriptor.release();
    guard.release();
    MTLSharedEventListener* listener =
        [[MTLSharedEventListener alloc] init];
    if (listener == nil) return -1;
    [event_ notifyListener:listener
                   atValue:signal_value
                     block:^(id<MTLSharedEvent>, std::uint64_t) {
                       notification->Complete();
                     }];
    [listener release];
    return read_descriptor.release();
  }

  std::uint64_t NextValue() noexcept {
    const std::uint64_t observed = event_.signaledValue;
    return observed == UINT64_MAX ? 0 : observed + 1;
  }

  int ImportFence(std::uint64_t signal_value, int fence_fd) noexcept {
    OwnedDescriptor owned_fence(fence_fd);
    if (signal_value == 0 || fence_fd < -1) return -1;
    if (fence_fd == -1) {
      std::lock_guard<std::mutex> lock(mutex_);
      if (event_.signaledValue < signal_value)
        event_.signaledValue = signal_value;
      return 0;
    }

    std::shared_ptr<FenceImportWork> work;
    try {
      work = std::make_shared<FenceImportWork>(
          event_, signal_value, std::move(owned_fence));
    } catch (...) {
      return -1;
    }
#if defined(DARWIN_ART_METAL_SHARED_EVENT_TESTING)
    if (reject_workers_.load(std::memory_order_acquire)) return -1;
#endif
    try {
      std::thread([work = std::move(work)]() noexcept { work->Run(); }).detach();
    } catch (...) {
      // Destruction of the moved closure and shared state closes fence_fd.
      return -1;
    }
    return 0;
  }

#if defined(DARWIN_ART_METAL_SHARED_EVENT_TESTING)
  static void RejectWorkers(bool reject) noexcept {
    reject_workers_.store(reject, std::memory_order_release);
  }
#endif

 private:
  id<MTLSharedEvent> event_;
  std::mutex mutex_;
#if defined(DARWIN_ART_METAL_SHARED_EVENT_TESTING)
  inline static std::atomic<bool> reject_workers_{false};
#endif
};

}  // namespace

namespace {

std::mutex g_events_mutex;
std::unordered_map<void*, std::shared_ptr<DarwinArtMetalSharedEventImpl>>
    g_events;

void* CreateImpl(
    void* metal_device, std::uint64_t* first_signal_value) noexcept {
  if (metal_device == nullptr || first_signal_value == nullptr) return nullptr;
  id<MTLDevice> device = (__bridge id<MTLDevice>)metal_device;
  id<MTLSharedEvent> event = [device newSharedEvent];
  if (event == nil) return nullptr;
  const std::uint64_t first = event.signaledValue == UINT64_MAX
                                  ? 0
                                  : event.signaledValue + 1;
  if (first == 0) {
    [event release];
    return nullptr;
  }
  std::shared_ptr<DarwinArtMetalSharedEventImpl> result;
  try {
    // The +1 returned by newSharedEvent is transferred into the state owner.
    result = std::make_shared<DarwinArtMetalSharedEventImpl>(event);
  } catch (...) {
    [event release];
    return nullptr;
  }
  void* raw_event = (__bridge void*)event;
  try {
    std::lock_guard<std::mutex> lock(g_events_mutex);
    if (!g_events.emplace(raw_event, result).second) {
      return nullptr;
    }
  } catch (...) {
    // `result` still owns the +1 event and releases it on this path.
    return nullptr;
  }
  *first_signal_value = first;
  return raw_event;
}

std::shared_ptr<DarwinArtMetalSharedEventImpl> Lookup(void* raw_event) {
  if (raw_event == nullptr) return nullptr;
  std::lock_guard<std::mutex> lock(g_events_mutex);
  auto found = g_events.find(raw_event);
  return found == g_events.end() ? nullptr : found->second;
}

}  // namespace

namespace darwin_art::graphics {

void* CreateMetalSharedEvent(
    void* metal_device, std::uint64_t* first_signal_value) noexcept {
  return CreateImpl(metal_device, first_signal_value);
}

int MetalSharedEventFenceFd(void* event,
                            std::uint64_t signal_value) noexcept {
  auto state = Lookup(event);
  return state == nullptr ? -1 : state->FenceFd(signal_value);
}

std::uint64_t MetalSharedEventNextValue(void* event) noexcept {
  auto state = Lookup(event);
  return state == nullptr ? 0 : state->NextValue();
}

int MetalSharedEventImportFence(void* event,
                                std::uint64_t signal_value,
                                int fence_fd) noexcept {
  auto state = Lookup(event);
  if (state == nullptr) {
    CloseDescriptor(&fence_fd);
    return -1;
  }
  return state->ImportFence(signal_value, fence_fd);
}

void ReleaseMetalSharedEvent(void* event) noexcept {
  if (event == nullptr) return;
  std::shared_ptr<DarwinArtMetalSharedEventImpl> state;
  {
    std::lock_guard<std::mutex> lock(g_events_mutex);
    auto found = g_events.find(event);
    if (found == g_events.end()) return;
    state = std::move(found->second);
    g_events.erase(found);
  }
}

}  // namespace darwin_art::graphics

extern "C" void* darwin_art_android_metal_shared_event_create(
    void* metal_device, std::uint64_t* signal_value) {
  return darwin_art::graphics::CreateMetalSharedEvent(metal_device,
                                                       signal_value);
}

extern "C" int darwin_art_android_metal_shared_event_fence_fd(
    void* shared_event, std::uint64_t signal_value) {
  return darwin_art::graphics::MetalSharedEventFenceFd(shared_event,
                                                       signal_value);
}

extern "C" std::uint64_t darwin_art_android_metal_shared_event_next_value(
    void* shared_event) {
  return darwin_art::graphics::MetalSharedEventNextValue(
      shared_event);
}

extern "C" int darwin_art_android_metal_shared_event_import_fence(
    void* shared_event, std::uint64_t signal_value, int fence_fd) {
  return darwin_art::graphics::MetalSharedEventImportFence(
      shared_event, signal_value, fence_fd);
}

extern "C" void darwin_art_android_metal_shared_event_release(
    void* shared_event) {
  darwin_art::graphics::ReleaseMetalSharedEvent(
      shared_event);
}

#if defined(DARWIN_ART_METAL_SHARED_EVENT_TESTING)
extern "C" void darwin_art_android_metal_shared_event_reject_workers_for_test(
    bool reject) {
  DarwinArtMetalSharedEventImpl::RejectWorkers(reject);
}
#endif
