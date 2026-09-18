#include "compat/graphics/metal_shared_event_provider.h"
#include "compat/surfaceflinger/metal_composer.h"
#include "darwin_art_bionic_socket_broker.h"

#import <CoreFoundation/CoreFoundation.h>
#import <IOSurface/IOSurface.h>
#import <Metal/Metal.h>

#include <cassert>
#include <chrono>
#include <cerrno>
#include <condition_variable>
#include <cstdint>
#include <cstdio>
#include <map>
#include <mutex>
#include <poll.h>
#include <unistd.h>

namespace {
std::mutex close_mutex;
std::condition_variable close_changed;
std::map<int, int> close_counts;

void RecordClose(int fd) {
  std::lock_guard<std::mutex> lock(close_mutex);
  ++close_counts[fd];
  close_changed.notify_all();
}

int CloseCountTotal() {
  std::lock_guard<std::mutex> lock(close_mutex);
  int total = 0;
  for (const auto& [fd, count] : close_counts) total += count;
  return total;
}

bool WaitForCloseDelta(int before, int delta) {
  std::unique_lock<std::mutex> lock(close_mutex);
  return close_changed.wait_for(lock, std::chrono::seconds(2), [&] {
    int total = 0;
    for (const auto& [fd, count] : close_counts) total += count;
    return total >= before + delta;
  });
}
}  // namespace

extern "C" int darwin_art_bionic_socket_broker_pipe(int32_t descriptors[2]) {
  return ::pipe(descriptors);
}

extern "C" int darwin_art_bionic_socket_broker_dup(int fd) {
  return ::dup(fd);
}

extern "C" intptr_t darwin_art_bionic_socket_broker_write(
    int fd, const void* bytes, size_t count) {
  return ::write(fd, bytes, count);
}

extern "C" int darwin_art_bionic_socket_broker_close(int fd) {
  if (fd < 0) return 0;
  const int result = ::close(fd);
  if (result == 0) RecordClose(fd);
  return result;
}

extern "C" int sync_wait(int fd, int timeout_ms) {
  pollfd descriptor{.fd = fd, .events = POLLIN, .revents = 0};
  for (;;) {
    const int result = ::poll(&descriptor, 1, timeout_ms);
    if (result < 0 && errno == EINTR) continue;
    return result > 0 && (descriptor.revents & (POLLIN | POLLHUP)) != 0 ? 0
                                                                        : -1;
  }
}

void SignalThroughImportedFence(void* event,
                                std::uint64_t value) {
  int source[2]{-1, -1};
  assert(::pipe(source) == 0);
  assert(darwin_art_android_metal_shared_event_import_fence(event, value,
                                                             source[0]) == 0);
  const unsigned char marker = 1;
  assert(::write(source[1], &marker, sizeof(marker)) == 1);
  assert(::close(source[1]) == 0);
}

void TestEarlyConsumerClose(id<MTLDevice> device) {
  std::uint64_t value = 0;
  void* event = darwin_art_android_metal_shared_event_create(
      (__bridge void*)device, &value);
  assert(event != nullptr && value != 0);
  assert((__bridge id<MTLSharedEvent>)event != nil);
  const int consumer = darwin_art_android_metal_shared_event_fence_fd(event,
                                                                        value);
  assert(consumer >= 0);
  const int before = CloseCountTotal();
  assert(darwin_art_bionic_socket_broker_close(consumer) == 0);
  SignalThroughImportedFence(event, value);
  assert(WaitForCloseDelta(before, 4));
  darwin_art_android_metal_shared_event_release(event);
}

void TestUnregisteredNativeEventIsNotBorrowed(id<MTLDevice> device) {
  id<MTLSharedEvent> native_event = [device newSharedEvent];
  assert(native_event != nil);
  void* raw_event = (__bridge void*)native_event;
  assert(darwin_art_android_metal_shared_event_fence_fd(raw_event, 1) == -1);
  darwin_art_android_metal_shared_event_release(raw_event);
  [native_event release];
}

void TestDelayedNotificationAfterRelease(id<MTLDevice> device) {
  std::uint64_t value = 0;
  void* event = darwin_art_android_metal_shared_event_create(
      (__bridge void*)device, &value);
  assert(event != nullptr && value != 0);
  assert((__bridge id<MTLSharedEvent>)event != nil);
  const int consumer = darwin_art_android_metal_shared_event_fence_fd(event,
                                                                        value);
  assert(consumer >= 0);

  int source[2]{-1, -1};
  assert(::pipe(source) == 0);
  assert(darwin_art_android_metal_shared_event_import_fence(event, value,
                                                             source[0]) == 0);
  darwin_art_android_metal_shared_event_release(event);
  const unsigned char marker = 1;
  assert(::write(source[1], &marker, sizeof(marker)) == 1);
  assert(::close(source[1]) == 0);

  std::uint64_t fence_marker = 0;
  pollfd descriptor{.fd = consumer, .events = POLLIN, .revents = 0};
  assert(::poll(&descriptor, 1, 2000) == 1);
  assert(::read(consumer, &fence_marker, sizeof(fence_marker)) ==
         static_cast<ssize_t>(sizeof(fence_marker)));
  assert(fence_marker == UINT64_C(0x44415257494e4653));
  assert(darwin_art_bionic_socket_broker_close(consumer) == 0);
}

void TestFailedWorkerAdmission(id<MTLDevice> device) {
  std::uint64_t value = 0;
  void* event = darwin_art_android_metal_shared_event_create(
      (__bridge void*)device, &value);
  assert(event != nullptr && value != 0);
  assert((__bridge id<MTLSharedEvent>)event != nil);
  auto native_event = (__bridge id<MTLSharedEvent>)event;
  native_event.signaledValue = 17;
  assert(darwin_art_android_metal_shared_event_next_value(event) == 18);
  int source[2]{-1, -1};
  assert(::pipe(source) == 0);
  const int before = CloseCountTotal();
  darwin_art_android_metal_shared_event_reject_workers_for_test(true);
  assert(darwin_art_android_metal_shared_event_import_fence(event, value,
                                                             source[0]) == -1);
  darwin_art_android_metal_shared_event_reject_workers_for_test(false);
  assert(WaitForCloseDelta(before, 1));
  assert(::close(source[1]) == 0);
  darwin_art_android_metal_shared_event_release(event);
}

IOSurfaceRef MakeComposerTarget(uint32_t width, uint32_t height) {
  const int32_t width_value = static_cast<int32_t>(width);
  const int32_t height_value = static_cast<int32_t>(height);
  const int32_t bytes_per_element = 4;
  const int32_t bytes_per_row = width_value * bytes_per_element;
  const int32_t allocation_size = height_value * bytes_per_row;
  CFMutableDictionaryRef properties = CFDictionaryCreateMutable(
      kCFAllocatorDefault, 5, &kCFTypeDictionaryKeyCallBacks,
      &kCFTypeDictionaryValueCallBacks);
  assert(properties != nullptr);
  CFNumberRef width_number =
      CFNumberCreate(kCFAllocatorDefault, kCFNumberSInt32Type, &width_value);
  CFNumberRef height_number =
      CFNumberCreate(kCFAllocatorDefault, kCFNumberSInt32Type, &height_value);
  CFNumberRef bytes_number = CFNumberCreate(
      kCFAllocatorDefault, kCFNumberSInt32Type, &bytes_per_element);
  CFNumberRef row_number =
      CFNumberCreate(kCFAllocatorDefault, kCFNumberSInt32Type, &bytes_per_row);
  CFNumberRef allocation_number = CFNumberCreate(
      kCFAllocatorDefault, kCFNumberSInt32Type, &allocation_size);
  assert(width_number != nullptr && height_number != nullptr &&
         bytes_number != nullptr && row_number != nullptr &&
         allocation_number != nullptr);
  CFDictionarySetValue(properties, kIOSurfaceWidth, width_number);
  CFDictionarySetValue(properties, kIOSurfaceHeight, height_number);
  CFDictionarySetValue(properties, kIOSurfaceBytesPerElement, bytes_number);
  CFDictionarySetValue(properties, kIOSurfaceBytesPerRow, row_number);
  CFDictionarySetValue(properties, kIOSurfaceAllocSize, allocation_number);
  IOSurfaceRef surface = IOSurfaceCreate(properties);
  CFRelease(allocation_number);
  CFRelease(row_number);
  CFRelease(bytes_number);
  CFRelease(height_number);
  CFRelease(width_number);
  CFRelease(properties);
  assert(surface != nullptr);
  assert(IOSurfaceGetWidth(surface) == width);
  assert(IOSurfaceGetHeight(surface) == height);
  return surface;
}

void WaitForComposerCompletion(void* event, uint64_t value) {
  const int fence = darwin_art_android_metal_shared_event_fence_fd(event, value);
  assert(fence >= 0);
  uint64_t marker = 0;
  pollfd descriptor{.fd = fence, .events = POLLIN, .revents = 0};
  assert(::poll(&descriptor, 1, 5000) == 1);
  assert(::read(fence, &marker, sizeof(marker)) ==
         static_cast<ssize_t>(sizeof(marker)));
  assert(marker == UINT64_C(0x44415257494e4653));
  assert(darwin_art_bionic_socket_broker_close(fence) == 0);
}

void TestMetalComposerRetiresCompletedEvents(id<MTLDevice> device) {
  for (int iteration = 0; iteration < 32; ++iteration) {
    IOSurfaceRef target = MakeComposerTarget(64, 48);
    void* completion_event = nullptr;
    uint64_t completion_value = 0;
    assert(darwin_art_metal_composer_compose(
        (__bridge void*)device, (__bridge void*)target, 64, 48, nullptr, 0,
        nullptr, 0, &completion_event, &completion_value));
    assert(completion_event != nullptr && completion_value != 0);

    // This is a real provider fence for the real Metal command buffer. It
    // proves the command has signaled before the opaque provider handle is
    // retired, without dereferencing the Objective-C event after release.
    WaitForComposerCompletion(completion_event, completion_value);
    darwin_art_android_metal_shared_event_release(completion_event);
    assert(darwin_art_android_metal_shared_event_next_value(completion_event) ==
           0);
    assert(darwin_art_android_metal_shared_event_fence_fd(
               completion_event, completion_value) == -1);
    CFRelease(target);
  }
}

int main() {
  @autoreleasepool {
    id<MTLDevice> device = MTLCreateSystemDefaultDevice();
    assert(device != nil && "real Metal device required for provider TU test");
    TestUnregisteredNativeEventIsNotBorrowed(device);
    TestEarlyConsumerClose(device);
    TestDelayedNotificationAfterRelease(device);
    TestFailedWorkerAdmission(device);
    TestMetalComposerRetiresCompletedEvents(device);
    std::puts("metal-shared-event-provider: notification/import/composer ownership PASS");
    [device release];
  }
}
