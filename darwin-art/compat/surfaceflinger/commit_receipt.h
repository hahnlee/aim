#pragma once
#include <stdint.h>

enum {
  DARWIN_ART_SF_COMMIT_UNKNOWN = 0,
  DARWIN_ART_SF_COMMIT_REJECTED = 1,
  DARWIN_ART_SF_COMMIT_COMMITTED = 2,
};

// Fixed-layout C transport receipt, not Android transaction policy.
// completion_fd is an owned guest descriptor, or -1. A committed receipt stays
// committed even if obtaining its completion descriptor fails.
typedef struct DarwinArtSurfaceFlingerReceipt {
  uint32_t disposition;
  int32_t error;
  int32_t completion_fd;
} DarwinArtSurfaceFlingerReceipt;
