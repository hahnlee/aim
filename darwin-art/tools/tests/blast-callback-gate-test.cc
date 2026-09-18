#include "../../compat/window/blast_callback_gate.h"

#include <atomic>
#include <cassert>
#include <cstdio>
#include <memory>
#include <thread>
#include <type_traits>
#include <utility>

using darwin_art::window::BlastCallbackGate;

int main() {
  static_assert(!std::is_copy_constructible_v<BlastCallbackGate::Lease>);
  static_assert(!std::is_copy_assignable_v<BlastCallbackGate::Lease>);
  static_assert(std::is_move_constructible_v<BlastCallbackGate::Lease>);
  static_assert(std::is_move_assignable_v<BlastCallbackGate::Lease>);

  // A callback admitted before close is allowed to finish. Close is
  // idempotent, rejects stale/new admissions, and does not report drained
  // until that callback's cleanup scope has ended.
  {
    BlastCallbackGate gate;
    auto admitted = gate.TryEnter();
    assert(admitted);
    gate.Close();
    gate.Close();
    assert(!gate.TryEnter());
    assert(!gate.Drained());
    admitted = BlastCallbackGate::Lease{};
    assert(gate.Drained());
    assert(!gate.TryEnter());
  }

  // The lease owns the control block independently of the gate object. This
  // models an observer retaining State while its callback is being cleaned up.
  {
    auto gate = std::make_unique<BlastCallbackGate>();
    auto admitted = gate->TryEnter();
    assert(admitted);
    delete gate.release();
    assert(admitted);
    admitted = BlastCallbackGate::Lease{};
  }

  // Moving a lease transfers exactly one count. Move-assignment first releases
  // the destination, so two moved leases still drain to zero exactly once.
  {
    BlastCallbackGate gate;
    auto first = gate.TryEnter();
    auto second = gate.TryEnter();
    assert(first && second);
    gate.Close();
    auto moved = std::move(first);
    assert(!first && moved);
    moved = std::move(second);
    assert(!second && moved);
    assert(!gate.Drained());
    moved = BlastCallbackGate::Lease{};
    assert(gate.Drained());
  }

  // Closing from mutually-racing owners has no lock-order dependency. Both
  // threads close both gates while synchronized at the start of the round.
  {
    BlastCallbackGate left;
    BlastCallbackGate right;
    auto left_lease = left.TryEnter();
    auto right_lease = right.TryEnter();
    assert(left_lease && right_lease);

    std::atomic<unsigned> ready{0};
    std::atomic<bool> go{false};
    auto close_both = [&](BlastCallbackGate& first,
                          BlastCallbackGate& second) {
      ready.fetch_add(1, std::memory_order_release);
      while (!go.load(std::memory_order_acquire)) std::this_thread::yield();
      first.Close();
      second.Close();
    };
    std::thread one(close_both, std::ref(left), std::ref(right));
    std::thread two(close_both, std::ref(right), std::ref(left));
    while (ready.load(std::memory_order_acquire) != 2)
      std::this_thread::yield();
    go.store(true, std::memory_order_release);
    one.join();
    two.join();
    assert(!left.TryEnter() && !right.TryEnter());
    assert(!left.Drained() && !right.Drained());
    left_lease = BlastCallbackGate::Lease{};
    right_lease = BlastCallbackGate::Lease{};
    assert(left.Drained() && right.Drained());
  }

  std::puts("BLAST callback gate: stale/closed admission, reentrant close, "
            "cross-close, move transfer, and deferred drain PASS");
  return 0;
}
