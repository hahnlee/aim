#include "probes/graphics_fixture_state.h"

#include <cassert>

using darwin_art_graphics_fixture::FixtureInputDispatchResult;

int main() {
  // A successful handoff with a deferred finish is delivered, but its
  // consumption is unknown and must never enter the unhandled fallback.
  const FixtureInputDispatchResult deferred{true, false, false};
  assert(deferred.delivered && !deferred.completed && !deferred.handled);
  assert(!deferred.ShouldRetryUnhandled());

  const FixtureInputDispatchResult completed_handled{true, true, true};
  assert(!completed_handled.ShouldRetryUnhandled());

  const FixtureInputDispatchResult completed_unhandled{true, true, false};
  assert(completed_unhandled.ShouldRetryUnhandled());

  const FixtureInputDispatchResult failed{false, false, false};
  assert(!failed.ShouldRetryUnhandled());
  const FixtureInputDispatchResult progress_failed{true, true, false, true};
  assert(progress_failed.delivered && !progress_failed.ShouldRetryUnhandled());
}
