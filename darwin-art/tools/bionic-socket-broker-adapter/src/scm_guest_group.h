#ifndef DARWIN_ART_SCM_GUEST_GROUP_H_
#define DARWIN_ART_SCM_GUEST_GROUP_H_

#include "darwin_art_bionic_fd_broker.h"
#include "darwin_art_bionic_fs.h"

#include <array>
#include <cstddef>

namespace darwin_art::bionic::scm {
// Factory selects the guest owner and hands over an unpublished object or
// owned file backing. All provider RPC and object allocation precede Add.
struct PreparedGuestDescriptor {
  DarwinArtFdPublishBatchEntryV1 central{};
  int filesystem_fd = -1;
  int descriptor_flags = 0;
};

class GuestDescriptorGroup final {
public:
  using CloseCentral = void (*)(void *, uint64_t) noexcept;
  GuestDescriptorGroup(DarwinArtFdBroker *broker, void *context,
                       CloseCentral close) noexcept
      : broker_(broker), context_(context), close_(close) {}
  ~GuestDescriptorGroup() noexcept;
  GuestDescriptorGroup(const GuestDescriptorGroup &) = delete;
  GuestDescriptorGroup &operator=(const GuestDescriptorGroup &) = delete;

  // Success transfers cleanup ownership. Failure leaves the caller's entry
  // untouched. Description/provider lifetimes must outlive this group.
  bool Add(const PreparedGuestDescriptor &entry) noexcept;
  // Atomic FS+central publication, 0 or positive Android errno. On failure no
  // guest output is installed; destruction closes all unpublished resources.
  int Publish() noexcept;
  std::size_t count() const noexcept { return count_; }
  int guest_fd(std::size_t ordinal) const noexcept {
    return committed_ && ordinal < count_ ? guest_[ordinal] : -1;
  }

private:
  static int CommitCentral(void *, const int *, std::size_t) noexcept;
  DarwinArtFdBroker *broker_ = nullptr;
  void *context_ = nullptr;
  CloseCentral close_ = nullptr;
  std::array<PreparedGuestDescriptor, 16> entries_{};
  std::array<DarwinArtFdPublishBatchEntryV1, 16> central_{};
  std::array<DarwinArtFsOwnedDescriptor, 16> files_{};
  std::array<int, 16> guest_{};
  std::array<int, 16> central_guest_{};
  std::array<int, 16> file_guest_{};
  std::size_t count_ = 0;
  std::size_t central_count_ = 0;
  std::size_t file_count_ = 0;
  bool attempted_ = false;
  bool committed_ = false;
};
}
#endif
