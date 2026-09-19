#include "scm_guest_group.h"

#include <cerrno>
#include <unistd.h>

namespace darwin_art::bionic::scm {

GuestDescriptorGroup::~GuestDescriptorGroup() noexcept {
  if (committed_) return;
  const int saved = errno;
  for (std::size_t i = 0; i < count_; ++i) {
    if (entries_[i].central.object != 0 && close_ != nullptr)
      close_(context_, entries_[i].central.object);
    if (entries_[i].filesystem_fd >= 0)
      (void)::close(entries_[i].filesystem_fd);
  }
  errno = saved;
}

bool GuestDescriptorGroup::Add(const PreparedGuestDescriptor &entry) noexcept {
  const bool has_object = entry.central.object != 0;
  const bool has_owner = entry.central.owner != 0;
  // A partially populated central entry must never be silently reclassified
  // as a filesystem descriptor.  The caller retains every input on rejection.
  const bool central = has_object && has_owner;
  const bool file = entry.filesystem_fd >= 0;
  if (attempted_ || count_ == entries_.size() || has_object != has_owner ||
      central == file ||
      (central && (broker_ == nullptr || close_ == nullptr)) ||
      (entry.descriptor_flags & ~DARWIN_ART_FD_CLOEXEC) != 0)
    return false;
  entries_[count_++] = entry;
  return true;
}

int GuestDescriptorGroup::CommitCentral(void *opaque, const int *files,
                                       std::size_t file_count) noexcept {
  auto &self = *static_cast<GuestDescriptorGroup *>(opaque);
  if (file_count != self.file_count_) return 22;
  if (self.central_count_ != 0) {
    const auto status = darwin_art_fd_broker_publish_batch_with_flags(
        self.broker_, self.central_.data(), self.central_count_,
        self.central_guest_.data());
    if (status != DARWIN_ART_FD_BROKER_OK)
      return status == DARWIN_ART_FD_BROKER_EXHAUSTED ? 24 : 9;
  }
  // No allocation, RPC, destruction or FS reentry follows publication.
  for (std::size_t i = 0; i < file_count; ++i) self.file_guest_[i] = files[i];
  return 0;
}

int GuestDescriptorGroup::Publish() noexcept {
  if (attempted_) return 22;
  attempted_ = true;
  for (std::size_t i = 0; i < count_; ++i) {
    auto &entry = entries_[i];
    if (entry.central.object != 0) {
      central_[central_count_++] = entry.central;
    } else {
      files_[file_count_++] = {entry.filesystem_fd,
                              static_cast<uint32_t>(entry.descriptor_flags)};
      // FS consumes each input even if staging or the callback fails.
      entry.filesystem_fd = -1;
    }
  }
  int error = 0;
  if (file_count_ == 0) {
    error = CommitCentral(this, nullptr, 0);
  } else {
    error = darwin_art_bionic_fs_adopt_group(
        files_.data(), file_count_, &CommitCentral, this, file_guest_.data());
  }
  if (error != 0) return error;
  std::size_t central_index = 0, file_index = 0;
  for (std::size_t i = 0; i < count_; ++i) {
    guest_[i] = entries_[i].central.object != 0
        ? central_guest_[central_index++] : file_guest_[file_index++];
  }
  committed_ = true;
  return 0;
}
}
