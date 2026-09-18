#pragma once

#include <cstdint>

namespace darwin_art::embedding {

// Runs shutdown through VM destruction. The completion flag tells an embedding
// client that provider/process finalization is still required. Clients may
// release their own VM-dependent resources between these two phases.
int32_t RunProcessShutdown(bool* needs_completion);

// Releases the remaining framework/embedding state and marks shutdown complete.
int32_t CompleteProcessShutdown();

// Performs the standalone process-exit DSO drain used when the host exits
// without the full DestroyJavaVM shutdown sequence.
int32_t PrepareProcessExit();

}  // namespace darwin_art::embedding
