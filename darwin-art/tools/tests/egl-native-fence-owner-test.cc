#include "compat/graphics/egl_native_fence_owner.h"

#include <atomic>
#include <array>
#include <cassert>
#include <condition_variable>
#include <cstdint>
#include <mutex>
#include <thread>

using darwin_art::graphics::EglNativeFenceAttrib;
using darwin_art::graphics::EglNativeFenceBackend;
using darwin_art::graphics::EglNativeFenceBoolean;
using darwin_art::graphics::EglNativeFenceDisplay;
using darwin_art::graphics::EglNativeFenceEnum;
using darwin_art::graphics::EglNativeFenceExport;
using darwin_art::graphics::EglNativeFenceInt;
using darwin_art::graphics::EglNativeFenceSync;

namespace {

int g_angle_sync;
int g_other_sync;
int g_event;
int g_device;
int g_display;
std::mutex g_mutex;
std::condition_variable g_changed;
bool g_block_wait = false;
bool g_wait_entered = false;
int g_wait_calls = 0;
int g_destroy_calls = 0;
std::atomic<int> g_event_release_calls{0};
int g_close_calls = 0;
int g_sync_wait_calls = 0;
int g_sync_wait_fd = -1;
int g_dup_calls = 0;
bool g_fail_event_sync = false;
bool g_throw_event_sync = false;
EglNativeFenceSync g_reenter_token = nullptr;
const EglNativeFenceBackend* g_backend = nullptr;

void Reset() {
  std::lock_guard<std::mutex> lock(g_mutex);
  g_block_wait = false;
  g_wait_entered = false;
  g_wait_calls = 0;
  g_destroy_calls = 0;
  g_event_release_calls = 0;
  g_close_calls = 0;
  g_sync_wait_calls = 0;
  g_sync_wait_fd = -1;
  g_dup_calls = 0;
  g_fail_event_sync = false;
  g_throw_event_sync = false;
  g_reenter_token = nullptr;
}

EglNativeFenceSync CreateKhr(EglNativeFenceDisplay, EglNativeFenceEnum type,
                             const EglNativeFenceInt*) {
  return type == 0x30F9 ? &g_other_sync : &g_angle_sync;
}

EglNativeFenceBoolean Destroy(EglNativeFenceDisplay, EglNativeFenceSync) {
  ++g_destroy_calls;
  return 1;
}

EglNativeFenceInt Wait(EglNativeFenceDisplay, EglNativeFenceSync,
                       EglNativeFenceInt, std::uint64_t) {
  std::unique_lock<std::mutex> lock(g_mutex);
  ++g_wait_calls;
  g_wait_entered = true;
  g_changed.notify_all();
  g_changed.wait(lock, [] { return !g_block_wait; });
  if (g_reenter_token != nullptr && g_backend != nullptr) {
    const auto token = g_reenter_token;
    g_reenter_token = nullptr;
    // The operation lease is held while this callback retires the same token;
    // retirement must defer cleanup until this callback and lease complete.
    assert(darwin_art::graphics::DestroyNativeFenceSync(
               &g_display, token, *g_backend) == 1);
  }
  return 7;
}

EglNativeFenceBoolean WaitSync(EglNativeFenceDisplay, EglNativeFenceSync,
                               EglNativeFenceInt) {
  return 1;
}

EglNativeFenceBoolean GetAttrib(EglNativeFenceDisplay, EglNativeFenceSync,
                                EglNativeFenceInt,
                                EglNativeFenceAttrib* value) {
  if (value != nullptr) *value = 11;
  return 1;
}

EglNativeFenceBoolean GetAttribKhr(EglNativeFenceDisplay, EglNativeFenceSync,
                                   EglNativeFenceInt, EglNativeFenceInt* value) {
  if (value != nullptr) *value = 11;
  return 1;
}

EglNativeFenceSync CreateEventSync(EglNativeFenceDisplay, EglNativeFenceEnum,
                                   const EglNativeFenceAttrib*) {
  if (g_fail_event_sync) return nullptr;
  if (g_throw_event_sync) throw std::bad_alloc();
  return &g_angle_sync;
}

EglNativeFenceBoolean QueryDisplay(EglNativeFenceDisplay, EglNativeFenceInt,
                                   EglNativeFenceAttrib* value) {
  if (value != nullptr) *value = reinterpret_cast<EglNativeFenceAttrib>(&g_device);
  return 1;
}

EglNativeFenceBoolean QueryDevice(void*, EglNativeFenceInt,
                                  EglNativeFenceAttrib* value) {
  if (value != nullptr) *value = reinterpret_cast<EglNativeFenceAttrib>(&g_device);
  return 1;
}

void* CreateEvent(void*, std::uint64_t* value) {
  if (value != nullptr) *value = 19;
  return &g_event;
}

EglNativeFenceInt EventFence(void*, std::uint64_t value) {
  assert(value == 19);
  return 41;
}

void ReleaseEvent(void*) { ++g_event_release_calls; }

int SyncWait(int fd, int timeout) {
  assert(timeout == -1);
  ++g_sync_wait_calls;
  g_sync_wait_fd = fd;
  return 0;
}

int CloseFd(int fd) {
  assert(fd == g_sync_wait_fd);
  ++g_close_calls;
  return 0;
}

EglNativeFenceInt DupUnknown(EglNativeFenceDisplay, EglNativeFenceSync sync) {
  assert(sync == &g_other_sync);
  ++g_dup_calls;
  return 43;
}

EglNativeFenceBackend Backend() {
  EglNativeFenceBackend backend;
  backend.create_sync_khr = CreateKhr;
  backend.destroy_sync = Destroy;
  backend.destroy_sync_khr = Destroy;
  backend.client_wait_sync = Wait;
  backend.client_wait_sync_khr = Wait;
  backend.wait_sync = WaitSync;
  backend.wait_sync_khr = WaitSync;
  backend.get_sync_attrib = GetAttrib;
  backend.get_sync_attrib_khr = GetAttribKhr;
  backend.dup_native_fence_fd = DupUnknown;
  backend.dup_native_fence_fd_khr = DupUnknown;
  backend.create_sync = CreateEventSync;
  backend.query_display_attrib = QueryDisplay;
  backend.query_device_attrib = QueryDevice;
  backend.metal_shared_event_create = CreateEvent;
  backend.metal_shared_event_fence_fd = EventFence;
  backend.metal_shared_event_release = ReleaseEvent;
  backend.sync_wait = SyncWait;
  backend.close_fence_fd = CloseFd;
  return backend;
}

}  // namespace

int main() {
  const auto backend = Backend();
  g_backend = &backend;
  auto display = static_cast<EglNativeFenceDisplay>(&g_display);
  constexpr EglNativeFenceInt kFdAttribute = 0x3145;

  // An exported fence keeps its Metal event alive until the token is retired.
  Reset();
  const EglNativeFenceExport exported =
      darwin_art::graphics::ExportNativeFence(display, backend);
  assert(exported.token != nullptr && exported.shared_event == &g_event);
  assert(darwin_art::graphics::DupNativeFenceFd(display, exported.token,
                                                backend) == 41);
  darwin_art::graphics::ReleaseNativeFence(display, exported.token, backend);
  assert(g_destroy_calls == 1 && g_event_release_calls == 1);
  // A stale/repeated destroy delegates to ANGLE but never releases the
  // retained Metal event twice.
  assert(darwin_art::graphics::DestroyNativeFenceSync(display, exported.token,
                                                       backend) == 0);
  assert(g_event_release_calls == 1);

  // Permanent-record allocation and Entry construction failures do not leak
  // imported descriptors or escape the EGL-facing owner call.
  Reset();
  const EglNativeFenceInt failure_attributes[] = {kFdAttribute, 88, 0x3038};
  darwin_art::graphics::SetNativeFenceOwnerAllocationFailuresForTest(1, 0);
  assert(darwin_art::graphics::CreateNativeFenceSync(
             display, 0x3144, failure_attributes, backend) == nullptr);
  assert(g_sync_wait_fd == 88 && g_close_calls == 1);
  darwin_art::graphics::SetNativeFenceOwnerAllocationFailuresForTest(0, 1);
  assert(darwin_art::graphics::ExportNativeFence(display, backend).token ==
         nullptr);
  darwin_art::graphics::SetNativeFenceOwnerAllocationFailuresForTest(0, 0, 1);
  assert(darwin_art::graphics::ExportNativeFence(display, backend).token ==
         nullptr);
  assert(g_event_release_calls == 1);
  darwin_art::graphics::SetNativeFenceOwnerAllocationFailuresForTest(0, 0);

  // A provider failure after an event has been retained runs the unpublished
  // Entry cleanup path exactly once.
  Reset();
  g_throw_event_sync = true;
  assert(darwin_art::graphics::ExportNativeFence(display, backend).token ==
         nullptr);
  assert(g_event_release_calls == 1);
  g_throw_event_sync = false;
  Reset();
  const auto replacement =
      darwin_art::graphics::ExportNativeFence(display, backend).token;
  assert(replacement != nullptr && replacement != exported.token);
  assert(darwin_art::graphics::ClientWaitNativeFenceSync(
             display, exported.token, 0, 1, backend) == 0);
  assert(darwin_art::graphics::ClientWaitNativeFenceSync(
             display, replacement, 0, 1, backend) == 7);
  darwin_art::graphics::ReleaseNativeFence(display, replacement, backend);

  // Historical retired records stay indexed: lookup of the old token remains
  // a stale translated-fence result even after many newer records exist.
  std::array<EglNativeFenceSync, 32> historical{};
  for (auto& token : historical) {
    token = darwin_art::graphics::ExportNativeFence(display, backend).token;
    assert(token != nullptr);
    darwin_art::graphics::ReleaseNativeFence(display, token, backend);
  }
  const int waits_before_stale = g_wait_calls;
  assert(darwin_art::graphics::ClientWaitNativeFenceSync(
             display, exported.token, 0, 1, backend) == 0);
  assert(g_wait_calls == waits_before_stale);

  // Provider creation failure releases a partially-created event and exposes
  // no translated handle to callers.
  Reset();
  g_fail_event_sync = true;
  assert(darwin_art::graphics::ExportNativeFence(display, backend).token ==
         nullptr);
  assert(g_event_release_calls == 1);

  // Destroy waits for an acquired operation and does not release the event
  // while the backend is still using it.
  Reset();
  const EglNativeFenceSync pending =
      darwin_art::graphics::ExportNativeFence(display, backend).token;
  assert(pending != nullptr);
  {
    std::lock_guard<std::mutex> lock(g_mutex);
    g_block_wait = true;
  }
  std::thread waiter([&] {
    assert(darwin_art::graphics::ClientWaitNativeFenceSync(
               display, pending, 0, 100, backend) == 7);
  });
  {
    std::unique_lock<std::mutex> lock(g_mutex);
    g_changed.wait(lock, [] { return g_wait_entered; });
  }
  std::atomic<bool> destroyed{false};
  std::thread destroyer([&] {
    assert(darwin_art::graphics::DestroyNativeFenceSync(display, pending,
                                                         backend) == 1);
    destroyed = true;
  });
  // Destroy retires synchronously but defers resource cleanup while the wait
  // lease is active. Joining here makes that ordering deterministic.
  destroyer.join();
  assert(destroyed.load() && g_event_release_calls == 0);
  {
    std::lock_guard<std::mutex> lock(g_mutex);
    g_block_wait = false;
  }
  g_changed.notify_all();
  waiter.join();
  assert(g_event_release_calls == 1);

  // Provider callbacks can synchronously retire the owner from an acquired
  // operation; cleanup is deferred until the operation lease releases.
  Reset();
  const auto reentrant =
      darwin_art::graphics::ExportNativeFence(display, backend).token;
  assert(reentrant != nullptr);
  g_reenter_token = reentrant;
  assert(darwin_art::graphics::ClientWaitNativeFenceSync(display, reentrant, 0,
                                                         1, backend) == 7);
  assert(g_event_release_calls == 1);

  // EGL takes ownership of an imported native-fence FD, including its close.
  Reset();
  const EglNativeFenceInt attributes[] = {kFdAttribute, 77, 0x3038};
  const auto imported = darwin_art::graphics::CreateNativeFenceSync(
      display, 0x3144, attributes, backend);
  assert(imported != nullptr && g_sync_wait_calls == 1 &&
         g_sync_wait_fd == 77 && g_close_calls == 1);
  assert(darwin_art::graphics::DestroyNativeFenceSync(display, imported,
                                                       backend) == 1);

  // Unknown handles retain ANGLE's provider path and are never treated as an
  // owned translated event.
  assert(darwin_art::graphics::DupNativeFenceFd(display, &g_other_sync,
                                                backend) == 43);
  assert(g_dup_calls == 1);
  return 0;
}
