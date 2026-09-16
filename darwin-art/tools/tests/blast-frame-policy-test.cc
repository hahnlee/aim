#include "../../compat/graphics/blast_frame_policy.h"

#include <cassert>
#include <cstdio>

using darwin_art::graphics::IsBlastTransactionDue;

int main() {
  // BLAST starts at frame 0. Android applies an unnumbered transaction at
  // once instead of waiting for a producer buffer that may never exist (for
  // example, a SurfaceView hosting a cross-process child SurfaceControl).
  assert(IsBlastTransactionDue(0, 0));
  assert(!IsBlastTransactionDue(0, 1));
  assert(IsBlastTransactionDue(1, 1));
  assert(IsBlastTransactionDue(7, 3));
  assert(!IsBlastTransactionDue(3, 7));

  std::puts("blast-frame-policy: PASS");
}
