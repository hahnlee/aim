#pragma once
#include "darwin_art_bionic_fd_broker.h"
#include "sync_fence_merge.h"
#include <memory>

namespace darwin_art::socket {
// Publisher consumes read_fd and merge on success AND failure. On failure it
// must cancel merge before dropping it. The published FD remains broker-owned.
using PublishSyncFence = int (*)(void* context, int read_fd,
    std::shared_ptr<SyncFenceMerge> merge, int* android_errno);
int MergeBrokerFences(DarwinArtFdBroker* broker, int first, int second,
    void* context, PublishSyncFence publish, int* android_errno) noexcept;
}
