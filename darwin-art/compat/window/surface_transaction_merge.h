#pragma once

#include "surface_transaction_state.h"

namespace darwin_art::window {

// Transfers source's structural state into destination and places every
// displaced destination owner into the empty, operation-local disposal
// transaction. It performs no callback invocation or resource release.
// Returns false before mutation if the required vector reservations fail or
// disposal is not empty.
bool MergeSurfaceTransactions(SurfaceTransaction* destination,
                              SurfaceTransaction* source,
                              SurfaceTransaction* disposal) noexcept;

}  // namespace darwin_art::window
