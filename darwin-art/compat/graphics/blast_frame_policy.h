#pragma once

#include <cstdint>

namespace darwin_art::graphics {

// Matches BLASTBufferQueue::mergeWithNextTransaction: a transaction whose
// requested frame has already been acquired is applied immediately. Frame 0
// is a real sentinel in that contract and therefore becomes immediately due
// while the queue's initial last-acquired frame is also 0.
inline bool IsBlastTransactionDue(uint64_t last_acquired_frame,
                                  uint64_t requested_frame) {
  return last_acquired_frame >= requested_frame;
}

}  // namespace darwin_art::graphics
