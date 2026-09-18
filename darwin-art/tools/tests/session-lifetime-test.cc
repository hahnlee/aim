#include "runtime/embedding/session_lifetime.h"

#include <atomic>
#include <cassert>
#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <new>
#include <thread>
#include <type_traits>

using darwin_art_graphics::SessionAdmissionKind;
using darwin_art_graphics::SessionLease;

namespace {

std::atomic<int> allocations_before_failure{-1};
std::atomic<std::size_t> allocation_attempts{0};
std::atomic<std::size_t> successful_allocations{0};
std::atomic<std::size_t> deallocations{0};

}  // namespace

void* operator new(std::size_t size) {
  allocation_attempts.fetch_add(1, std::memory_order_relaxed);
  const int remaining = allocations_before_failure.load(std::memory_order_relaxed);
  if (remaining == 0) {
    allocations_before_failure.store(-1, std::memory_order_relaxed);
    throw std::bad_alloc();
  }
  if (remaining > 0)
    allocations_before_failure.fetch_sub(1, std::memory_order_relaxed);
  void* allocation = std::malloc(size == 0 ? 1 : size);
  if (allocation == nullptr) throw std::bad_alloc();
  successful_allocations.fetch_add(1, std::memory_order_relaxed);
  return allocation;
}

void operator delete(void* pointer) noexcept {
  if (pointer != nullptr) deallocations.fetch_add(1, std::memory_order_relaxed);
  std::free(pointer);
}

void operator delete(void* pointer, std::size_t) noexcept {
  if (pointer != nullptr) deallocations.fetch_add(1, std::memory_order_relaxed);
  std::free(pointer);
}

void* operator new(std::size_t size, const std::nothrow_t&) noexcept {
  try {
    return ::operator new(size);
  } catch (...) {
    return nullptr;
  }
}

void operator delete(void* pointer, const std::nothrow_t&) noexcept {
  ::operator delete(pointer);
}

namespace {

constexpr uintptr_t kArtThreadIdentity = 0x1234;
constexpr uintptr_t kLooperIdentity = 0x5678;

void* ArtThreadIdentity() {
  return reinterpret_cast<void*>(kArtThreadIdentity);
}

void* LooperIdentity() {
  return reinterpret_cast<void*>(kLooperIdentity);
}

}  // namespace

int main() {
  static_assert(!std::is_copy_constructible_v<SessionLease>);
  static_assert(!std::is_copy_assignable_v<SessionLease>);
  static_assert(std::is_move_constructible_v<SessionLease>);
  static_assert(std::is_move_assignable_v<SessionLease>);

  // The session allocation must be reclaimed when the first unordered_set
  // node/bucket publication fails. This injects one failure after the
  // session's own allocation, rather than making all allocations fail.
  const auto attempts_before = allocation_attempts.load();
  const auto successes_before = successful_allocations.load();
  const auto frees_before = deallocations.load();
  allocations_before_failure.store(1, std::memory_order_release);
  darwin_art_graphics_session_t* failed_session = nullptr;
  bool threw = false;
  try {
    failed_session = darwin_art_graphics::create_session();
  } catch (...) {
    threw = true;
  }
  assert(!threw && failed_session == nullptr);
  const auto attempts_delta = allocation_attempts.load() - attempts_before;
  const auto successes_delta = successful_allocations.load() - successes_before;
  const auto frees_delta = deallocations.load() - frees_before;
  assert(attempts_delta == successes_delta + 1);
  assert(successes_delta == 1 && frees_delta == 1);
  std::puts("session publication OOM: post-allocation failure, no exception, unpublished allocation reclaimed PASS");

  auto* recovered_session = darwin_art_graphics::create_session();
  assert(recovered_session != nullptr);
  assert(darwin_art_graphics::close_session(recovered_session, nullptr, false) == 0);
  assert(darwin_art_graphics::destroy_session(recovered_session) == 0);

  // An admitted owner callback may re-enter close/destroy. Both calls return
  // retryable status immediately; neither waits on its own lease.
  auto* session = darwin_art_graphics::create_session();
  assert(session != nullptr);
  assert(darwin_art_graphics::bind_session_for_process_lifetime(session) == 0);
  assert(darwin_art_graphics::bind_session_art_thread_identity(
             ArtThreadIdentity()) == 0);
  assert(darwin_art_graphics::preflight_session(
             session, SessionAdmissionKind::kOwner) == 0);
  auto* state = darwin_art_graphics::state_for_context(session);
  assert(state != nullptr);

  SessionLease admitted;
  assert(darwin_art_graphics::admit_session(
             session, SessionAdmissionKind::kOwner, ArtThreadIdentity(), true,
             &admitted) == 0);
  bool callback_ran = false;
  auto reentrant_callback = [&] {
    callback_ran = true;
    assert(darwin_art_graphics::close_session(
               session, ArtThreadIdentity(), true) ==
           DARWIN_ART_STATUS_GRAPHICS_SESSION_NOT_READY);
    assert(darwin_art_graphics::destroy_session(session) ==
           DARWIN_ART_STATUS_GRAPHICS_SESSION_NOT_READY);
  };
  reentrant_callback();
  assert(callback_ran);
  admitted = SessionLease{};

  // Closed-but-bound destruction is rejected until canonical finalization.
  assert(darwin_art_graphics::close_session(session, ArtThreadIdentity(),
                                            true) == 0);
  assert(darwin_art_graphics::destroy_session(session) ==
         DARWIN_ART_STATUS_GRAPHICS_SESSION_NOT_READY);
  assert(darwin_art_graphics::bound_session_quiescent(state));
  assert(darwin_art_graphics::finalize_bound_session(state,
                                                     ArtThreadIdentity()) == 0);
  // Finalization is the hand-off point after the last state use; the process
  // owner may now reset GraphicsState and later destroy the opaque allocation.
  assert(!darwin_art_graphics::bound_session_quiescent(state));

  // Finalized destruction is safe from a non-owner thread and performs no ART
  // lookup. The stale handle is rejected before state/ART access afterwards.
  std::thread finalized_destroy([&] {
    assert(darwin_art_graphics::destroy_session(session) == 0);
  });
  finalized_destroy.join();
  assert(darwin_art_graphics::preflight_session(
             session, SessionAdmissionKind::kOwner) ==
         DARWIN_ART_STATUS_GRAPHICS_SESSION_INVALID);
  assert(darwin_art_graphics::preflight_session(
             session, SessionAdmissionKind::kWake) ==
         DARWIN_ART_STATUS_GRAPHICS_SESSION_INVALID);
  assert(darwin_art_graphics::state_for_context(session) == nullptr);

  // Wrong-thread owner admission is rejected exactly, while wake admission
  // uses only the retained Looper token and never asks ART for current thread.
  auto* wake_session = darwin_art_graphics::create_session();
  assert(wake_session != nullptr);
  assert(darwin_art_graphics::bind_session_for_process_lifetime(wake_session) ==
         0);
  assert(darwin_art_graphics::bind_session_art_thread_identity(
             ArtThreadIdentity()) == 0);
  darwin_art_graphics::set_session_looper(wake_session, LooperIdentity());

  std::atomic<bool> wake_admitted{false};
  std::atomic<bool> release_wake{false};
  std::thread helper([&] {
    // Preflight must reject affinity before the wrapper would ask ART for the
    // current thread. validate_session has the same stale/raw-handle rule for
    // close callers.
    assert(darwin_art_graphics::preflight_session(
               wake_session, SessionAdmissionKind::kOwner) ==
           DARWIN_ART_STATUS_GRAPHICS_SESSION_WRONG_THREAD);
    assert(darwin_art_graphics::validate_session(wake_session) ==
           DARWIN_ART_STATUS_GRAPHICS_SESSION_WRONG_THREAD);
    SessionLease wrong_owner;
    assert(darwin_art_graphics::admit_session(
               wake_session, SessionAdmissionKind::kOwner,
               ArtThreadIdentity(), true, &wrong_owner) ==
           DARWIN_ART_STATUS_GRAPHICS_SESSION_WRONG_THREAD);

    SessionLease wake;
    assert(darwin_art_graphics::admit_session(
               wake_session, SessionAdmissionKind::kWake, nullptr, false,
               &wake) == 0);
    wake_admitted.store(true, std::memory_order_release);
    while (!release_wake.load(std::memory_order_acquire)) std::this_thread::yield();
    // The wake lease is still active, but this is not the owner thread.
    assert(darwin_art_graphics::close_session(wake_session, nullptr, false) ==
           DARWIN_ART_STATUS_GRAPHICS_SESSION_WRONG_THREAD);
  });

  while (!wake_admitted.load(std::memory_order_acquire)) std::this_thread::yield();
  assert(darwin_art_graphics::close_session(wake_session, ArtThreadIdentity(),
                                            true) ==
         DARWIN_ART_STATUS_GRAPHICS_SESSION_NOT_READY);
  release_wake.store(true, std::memory_order_release);
  helper.join();

  assert(darwin_art_graphics::close_session(wake_session, ArtThreadIdentity(),
                                            true) == 0);
  auto* wake_state = darwin_art_graphics::state_for_context(wake_session);
  assert(wake_state != nullptr);
  assert(darwin_art_graphics::finalize_bound_session(
             wake_state, ArtThreadIdentity()) == 0);
  assert(darwin_art_graphics::destroy_session(wake_session) == 0);

  std::puts("session lifetime: reentrant close/destroy, quiescent finalization, "
            "stale admission, exact wrong-thread rejection, and wake-vs-close "
            "PASS");
  return 0;
}
