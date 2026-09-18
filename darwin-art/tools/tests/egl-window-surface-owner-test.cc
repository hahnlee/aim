#include "compat/graphics/egl_window_surface_owner.h"

#include <atomic>
#include <cassert>
#include <cstdio>
#include <memory>
#include <thread>
#include <cstdlib>
#include <new>

static bool fail_next_allocation = false;
static int fail_after_allocations = -1;
void* operator new(std::size_t size) {
  if (fail_next_allocation || fail_after_allocations == 0) {
    fail_next_allocation = false;
    fail_after_allocations = -1;
    throw std::bad_alloc();
  }
  if (fail_after_allocations > 0) --fail_after_allocations;
  if (void* result = std::malloc(size == 0 ? 1 : size)) return result;
  throw std::bad_alloc();
}
void* operator new[](std::size_t size) { return ::operator new(size); }
void operator delete(void* pointer) noexcept { std::free(pointer); }
void operator delete[](void* pointer) noexcept { std::free(pointer); }
void operator delete(void* pointer, std::size_t) noexcept { std::free(pointer); }
void operator delete[](void* pointer, std::size_t) noexcept { std::free(pointer); }

using Owner = darwin_art::graphics::EglWindowSurfaceOwner;
using Info = darwin_art::graphics::EglWindowSurfaceCreateInfo;
using Cleanup = darwin_art::graphics::EglWindowSurfaceCleanup;
using Admission = darwin_art::graphics::EglWindowSurfaceAdmission;

struct Counters {
  std::atomic<int> cleanup{0};
  std::atomic<int> terminate{0};
  Owner* owner = nullptr;
  void* display = nullptr;
  void* surface = nullptr;
};

void CleanupCallback(const Cleanup& cleanup, void* opaque) {
  auto* counters = static_cast<Counters*>(opaque);
  assert(cleanup.display != nullptr && cleanup.surface != nullptr);
  counters->cleanup.fetch_add(1, std::memory_order_relaxed);
  // Callback reentry must not deadlock the owner or run cleanup twice.
  if (counters->owner != nullptr) {
    Admission admission;
    (void)counters->owner->Destroy(counters->display, counters->surface,
                                   &admission);
  }
}

bool TerminateCallback(void* display, void* opaque) {
  auto* counters = static_cast<Counters*>(opaque);
  (void)display;
  counters->terminate.fetch_add(1, std::memory_order_relaxed);
  return true;
}

struct BlockingState {
  std::atomic<bool> started{false};
  std::atomic<bool> release{false};
  std::atomic<int> cleanup{0};
  std::atomic<int> terminate{0};
  void* display = nullptr;
};

void BlockingCleanup(const Cleanup& cleanup, void* opaque) {
  auto* state = static_cast<BlockingState*>(opaque);
  assert(cleanup.display == state->display);
  state->started.store(true, std::memory_order_release);
  while (!state->release.load(std::memory_order_acquire))
    std::this_thread::yield();
  state->cleanup.fetch_add(1, std::memory_order_relaxed);
}

bool BlockingTerminate(void* display, void* opaque) {
  auto* state = static_cast<BlockingState*>(opaque);
  assert(display == state->display);
  state->terminate.fetch_add(1, std::memory_order_relaxed);
  return true;
}

bool FailingTerminate(void*, void*) { return false; }

int main() {
  Owner& owner = Owner::Instance();
  void* display = reinterpret_cast<void*>(0x1000);
  void* surface = reinterpret_cast<void*>(0x1234);
  Counters counters{.owner = &owner, .display = display, .surface = surface};
  owner.BeginDisplay(display);

  Info info;
  info.native_window = reinterpret_cast<void*>(0x5678);
  info.iosurface = reinterpret_cast<void*>(0x9abc);
  info.width = 1440;
  info.height = 900;
  assert(owner.Create(display, surface, std::move(info), CleanupCallback,
                      &counters));

  Admission admission;
  auto operation = owner.Acquire(display, surface, &admission);
  assert(operation && admission == Admission::kAdmitted);
  auto snapshot = operation.Snapshot();
  assert(snapshot.native_window == reinterpret_cast<void*>(0x5678));
  assert(owner.Acquire(display, surface, &admission).operator bool() == false &&
         admission == Admission::kBusy);
  assert(owner.Destroy(display, surface, &admission) &&
         admission == Admission::kBusy);
  operation.SetDimensions(2880, 1800, 2880, 1800);
  operation = {};
  assert(counters.cleanup.load() == 1);
  assert(!owner.Acquire(display, surface, &admission) &&
         admission == Admission::kUnknown);

  // Unknown pbuffers remain caller/ANGLE-owned, and wrong-display access is
  // never classified as an unknown pass-through.
  void* other_surface = reinterpret_cast<void*>(0x2234);
  Info other;
  assert(owner.Create(display, other_surface, std::move(other),
                      CleanupCallback, &counters));
  assert(!owner.Acquire(reinterpret_cast<void*>(0x2000), other_surface,
                        &admission) && admission == Admission::kWrongDisplay);
  assert(owner.Destroy(display, other_surface, &admission));

  // Admission is exclusive even when two producers race from different
  // threads; the winner keeps its lease until the test releases it.
  void* concurrent_surface = reinterpret_cast<void*>(0x2f34);
  Info concurrent;
  assert(owner.Create(display, concurrent_surface, std::move(concurrent),
                      CleanupCallback, &counters));
  std::atomic<int> admitted{0};
  std::atomic<int> busy{0};
  std::atomic<int> entered{0};
  std::atomic<bool> release{false};
  auto race = [&] {
    entered.fetch_add(1, std::memory_order_release);
    while (entered.load(std::memory_order_acquire) != 2)
      std::this_thread::yield();
    Admission result;
    auto lease = owner.Acquire(display, concurrent_surface, &result);
    if (lease) {
      admitted.fetch_add(1, std::memory_order_relaxed);
      while (!release.load(std::memory_order_acquire)) std::this_thread::yield();
    } else if (result == Admission::kBusy) {
      busy.fetch_add(1, std::memory_order_relaxed);
    }
  };
  std::thread first(race);
  std::thread second(race);
  while (entered.load(std::memory_order_acquire) != 2) std::this_thread::yield();
  while (busy.load(std::memory_order_acquire) != 1)
    std::this_thread::yield();
  release.store(true, std::memory_order_release);
  first.join();
  second.join();
  assert(admitted.load() == 1 && busy.load() == 1);
  // The winning thread has already joined, so its lease has been released.
  assert(owner.Destroy(display, concurrent_surface, &admission));

  // Retire enrollment also covers a cleanup callback already in flight.  The
  // display terminate callback must remain pending until that callback exits
  // and until an admitted swap lease is released.
  void* deferred_surface = reinterpret_cast<void*>(0x3234);
  Info deferred;
  assert(owner.Create(display, deferred_surface, std::move(deferred),
                      CleanupCallback, &counters));
  Admission deferred_admission;
  auto held = owner.Acquire(display, deferred_surface, &deferred_admission);
  assert(held);

  BlockingState blocking{.display = display};
  void* blocking_surface = reinterpret_cast<void*>(0x3f34);
  Info blocking_info;
  assert(owner.Create(display, blocking_surface, std::move(blocking_info),
                      BlockingCleanup, &blocking));
  std::thread destroyer([&] {
    Admission result;
    assert(owner.Destroy(display, blocking_surface, &result));
  });
  while (!blocking.started.load(std::memory_order_acquire))
    std::this_thread::yield();
  assert(owner.RetireDisplay(display, BlockingTerminate, &blocking));
  assert(blocking.terminate.load(std::memory_order_acquire) == 0);
  assert(!owner.BeginDisplay(display));
  Info rejected_during_drain;
  assert(!owner.Create(display, reinterpret_cast<void*>(0x3f35),
                       std::move(rejected_during_drain), CleanupCallback,
                       &counters));
  blocking.release.store(true, std::memory_order_release);
  destroyer.join();
  assert(blocking.cleanup.load() == 1 && blocking.terminate.load() == 0);
  assert(counters.cleanup.load() == 3);
  held = {};
  assert(counters.cleanup.load() == 4);
  assert(blocking.terminate.load() == 1);

  // A second termination is rejected while the display is draining, and a
  // later initialize can explicitly reopen the identity.
  assert(!owner.RetireDisplay(display, TerminateCallback, &counters));
  owner.BeginDisplay(display);
  assert(owner.RetireDisplay(display, FailingTerminate, nullptr));
  assert(owner.TerminationStatus(display) ==
         darwin_art::graphics::EglWindowSurfaceTermination::kFailed);

  // A retained lease may outlive an explicit owner instance. The control
  // block detaches finalization without leaving a raw-owner UAF.
  struct LifetimeState {
    std::atomic<int> cleaned{0};
  } lifetime;
  auto LifetimeCleanup = [](const Cleanup&, void* opaque) {
    static_cast<LifetimeState*>(opaque)->cleaned.fetch_add(1);
  };
  auto local_owner = std::make_unique<Owner>();
  void* local_display = reinterpret_cast<void*>(0x5000);
  void* local_surface = reinterpret_cast<void*>(0x5001);
  local_owner->BeginDisplay(local_display);
  Info local_info;
  assert(local_owner->Create(local_display, local_surface,
                             std::move(local_info), LifetimeCleanup,
                             &lifetime));
  Admission local_admission;
  auto surviving_lease = local_owner->Acquire(local_display, local_surface,
                                              &local_admission);
  assert(surviving_lease);
  local_owner.reset();
  surviving_lease = {};
  assert(lifetime.cleaned.load() == 1);

  BlockingState destruction_block{.display = local_display};
  auto blocked_owner = std::make_unique<Owner>();
  blocked_owner->BeginDisplay(local_display);
  void* blocked_surface = reinterpret_cast<void*>(0x5002);
  Info blocked_info;
  assert(blocked_owner->Create(local_display, blocked_surface,
                               std::move(blocked_info), BlockingCleanup,
                               &destruction_block));
  std::thread blocked_destroyer([&] {
    Admission result;
    assert(blocked_owner->Destroy(local_display, blocked_surface, &result));
  });
  while (!destruction_block.started.load(std::memory_order_acquire))
    std::this_thread::yield();
  blocked_owner.reset();
  destruction_block.release.store(true, std::memory_order_release);
  blocked_destroyer.join();
  assert(destruction_block.cleanup.load() == 1);
  {
    Owner failure_owner;
    Counters failure{.display = reinterpret_cast<void*>(0x6000)};
    assert(failure_owner.BeginDisplay(failure.display));
    Info initial;
    assert(failure_owner.Create(failure.display, reinterpret_cast<void*>(0x6001),
                                std::move(initial), CleanupCallback, &failure));
    fail_next_allocation = true;
    assert(!failure_owner.RetireDisplay(failure.display, TerminateCallback,
                                        &failure));
    assert(!fail_next_allocation);
    assert(failure_owner.TerminationStatus(failure.display) ==
           darwin_art::graphics::EglWindowSurfaceTermination::kActive);
    assert(failure.cleanup.load() == 0 && failure.terminate.load() == 0);
    Info subsequent;
    assert(failure_owner.Create(failure.display, reinterpret_cast<void*>(0x6002),
                                std::move(subsequent), CleanupCallback, &failure));
    assert(failure_owner.RetireDisplay(failure.display, TerminateCallback,
                                       &failure));
    assert(failure.cleanup.load() == 2 && failure.terminate.load() == 1);
  }
  {
    LifetimeState noalloc;
    auto destructor_owner = std::make_unique<Owner>();
    assert(destructor_owner->BeginDisplay(local_display));
    Info resources;
    assert(destructor_owner->Create(local_display, local_surface,
                                    std::move(resources), LifetimeCleanup,
                                    &noalloc));
    fail_next_allocation = true;
    destructor_owner.reset();
    assert(fail_next_allocation && noalloc.cleaned.load() == 1);
    fail_next_allocation = false;
  }
  {
    LifetimeState init_cleanup;
    Owner initialization_owner;
    void* init_display = reinterpret_cast<void*>(0x7000);
    void* init_surface = reinterpret_cast<void*>(0x7001);
    Counters init_counters;
    init_counters.owner = &initialization_owner;
    init_counters.display = init_display;
    auto first = initialization_owner.ReserveDisplayInitialization(init_display);
    assert(first);
    assert(!initialization_owner.ReserveDisplayInitialization(init_display));
    assert(!initialization_owner.RetireDisplay(init_display, TerminateCallback,
                                               &init_counters));
    Info before_complete;
    assert(!initialization_owner.Create(init_display, init_surface,
                                        std::move(before_complete),
                                        LifetimeCleanup, &init_cleanup));
    assert(!first.Complete(false));
    assert(!initialization_owner.Create(init_display, init_surface,
                                        Info{}, LifetimeCleanup, &init_cleanup));
    auto initialized =
        initialization_owner.ReserveDisplayInitialization(init_display);
    assert(initialized && initialized.Complete(true));
    assert(initialization_owner.BeginDisplay(init_display));
    Info active_info;
    assert(initialization_owner.Create(init_display, init_surface,
                                       std::move(active_info), LifetimeCleanup,
                                       &init_cleanup));
    auto failed_active =
        initialization_owner.ReserveDisplayInitialization(init_display);
    assert(failed_active && !failed_active.Complete(false));
    Admission active_admission;
    assert(initialization_owner.Acquire(init_display, init_surface,
                                        &active_admission));
    assert(initialization_owner.Destroy(init_display, init_surface));
    assert(initialization_owner.RetireDisplay(init_display, TerminateCallback,
                                               &init_counters));
    assert(initialization_owner.TerminationStatus(init_display) ==
           darwin_art::graphics::EglWindowSurfaceTermination::kSucceeded);
    auto failed_terminal =
        initialization_owner.ReserveDisplayInitialization(init_display);
    assert(failed_terminal);
    assert(!initialization_owner.Create(init_display,
                                        reinterpret_cast<void*>(0x7002), Info{},
                                        LifetimeCleanup, &init_cleanup));
    assert(!failed_terminal.Complete(false));
    assert(initialization_owner.TerminationStatus(init_display) ==
           darwin_art::graphics::EglWindowSurfaceTermination::kSucceeded);
    assert(!initialization_owner.Create(init_display,
                                        reinterpret_cast<void*>(0x7003), Info{},
                                        LifetimeCleanup, &init_cleanup));
    auto reinitialized =
        initialization_owner.ReserveDisplayInitialization(init_display);
    assert(reinitialized && reinitialized.Complete(true));
    Info new_generation;
    assert(initialization_owner.Create(init_display,
                                       reinterpret_cast<void*>(0x7004),
                                       std::move(new_generation),
                                       LifetimeCleanup, &init_cleanup));
  }
  {
    Owner abandoned_owner;
    void* display = reinterpret_cast<void*>(0x8000);
    {
      auto abandoned = abandoned_owner.ReserveDisplayInitialization(display);
      assert(abandoned);
    }
    auto retry = abandoned_owner.ReserveDisplayInitialization(display);
    assert(retry);
    Owner::DisplayInitializationLease moved;
    moved = std::move(retry);
    assert(!retry && moved);
    assert(moved.Complete(true) && !moved.Complete(true));
    auto other = abandoned_owner.ReserveDisplayInitialization(
        reinterpret_cast<void*>(0x8001));
    auto replacement = abandoned_owner.ReserveDisplayInitialization(
        reinterpret_cast<void*>(0x8002));
    assert(other && replacement);
    other = std::move(replacement);  // releases the previous reservation
    assert(abandoned_owner.ReserveDisplayInitialization(
        reinterpret_cast<void*>(0x8001)));
    assert(other.Complete(true));
  }
  {
    Owner::DisplayInitializationLease surviving;
    {
      Owner destroyed_owner;
      surviving = destroyed_owner.ReserveDisplayInitialization(
          reinterpret_cast<void*>(0x8100));
      assert(surviving);
    }
    assert(!surviving.Complete(true));
  }
  {
    Owner oom_owner;
    void* display = reinterpret_cast<void*>(0x8200);
    fail_next_allocation = true;
    assert(!oom_owner.ReserveDisplayInitialization(display));
    assert(!fail_next_allocation);
    auto retry = oom_owner.ReserveDisplayInitialization(display);
    assert(retry && retry.Complete(true));
  }
  {
    Owner terminating_owner;
    void* display = reinterpret_cast<void*>(0x8300);
    auto initial = terminating_owner.ReserveDisplayInitialization(display);
    assert(initial && initial.Complete(true));
    struct BlockedTermination {
      std::atomic<bool> entered{false};
      std::atomic<bool> release{false};
      std::atomic<int> calls{0};
    } blocked;
    auto terminate = [](void*, void* opaque) -> bool {
      auto* state = static_cast<BlockedTermination*>(opaque);
      ++state->calls;
      state->entered.store(true);
      while (!state->release.load()) std::this_thread::yield();
      return true;
    };
    std::thread retirement([&] {
      assert(terminating_owner.RetireDisplay(display, terminate, &blocked));
    });
    while (!blocked.entered.load()) std::this_thread::yield();
    int backend_initializations = 0;
    auto attempted = terminating_owner.ReserveDisplayInitialization(display);
    if (attempted) {
      ++backend_initializations;
      (void)attempted.Complete(true);
    }
    assert(!attempted && backend_initializations == 0);
    blocked.release.store(true);
    retirement.join();
    assert(blocked.calls.load() == 1);
    auto reinitialized = terminating_owner.ReserveDisplayInitialization(display);
    assert(reinitialized && reinitialized.Complete(true));
  }
  {
    int rejected_allocation_sites = 0;
    bool reached_success = false;
    for (int allocation_site = 0; allocation_site < 8; ++allocation_site) {
      Owner allocation_owner;
      void* display = reinterpret_cast<void*>(0x8400);
      fail_after_allocations = allocation_site;
      auto reserved = allocation_owner.ReserveDisplayInitialization(display);
      fail_after_allocations = -1;
      if (!reserved) {
        ++rejected_allocation_sites;
        auto retry = allocation_owner.ReserveDisplayInitialization(display);
        assert(retry && retry.Complete(true));
      } else {
        // Completion is a publication of prepared state, never an allocation.
        fail_next_allocation = true;
        assert(reserved.Complete(true));
        assert(fail_next_allocation);
        fail_next_allocation = false;
        reached_success = true;
        break;
      }
    }
    // A fresh reservation allocates its record, map node and bucket storage.
    assert(rejected_allocation_sites >= 3 && reached_success);
  }
  std::puts("egl window surface owner: PASS ownership/admission/deferred drain");
}
