#ifndef DARWIN_ART_SYNC_FENCE_MERGE_H_
#define DARWIN_ART_SYNC_FENCE_MERGE_H_

#include <memory>

namespace darwin_art::socket {

// Owns three already-exported native descriptors.  The two input descriptors
// must be independent read ends, and the output descriptor must be its write
// end.  Create consumes every descriptor on both success and failure.
//
// This class deliberately accepts native host descriptors only.  Broker guest
// descriptors must first be exported by their owner, and the resulting output
// must be imported/published by the adapter that owns the broker namespace.
class SyncFenceMerge {
 public:
  static std::shared_ptr<SyncFenceMerge> Create(
      int owned_first_host_fd, int owned_second_host_fd,
      int owned_output_write_fd,
      std::shared_ptr<void> retained_inputs = {}) noexcept;

  SyncFenceMerge(const SyncFenceMerge&) = delete;
  SyncFenceMerge& operator=(const SyncFenceMerge&) = delete;

  // Idempotent.  The owner must call this before closing the output read end.
  // Pending dispatch callbacks are allowed to drain, but cannot signal after
  // cancellation has won the state lock.
  void Cancel() noexcept;

 private:
  struct Impl;

  explicit SyncFenceMerge(std::shared_ptr<Impl> impl) noexcept;

  std::shared_ptr<Impl> impl_;
};

}  // namespace darwin_art::socket

#endif  // DARWIN_ART_SYNC_FENCE_MERGE_H_
