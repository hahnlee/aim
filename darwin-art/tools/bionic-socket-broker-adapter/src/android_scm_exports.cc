#include "android_scm_exports.h"

#include <cerrno>
#include <cstring>
#include <limits>
#include <new>
#include <unistd.h>

namespace darwin_art::bionic::scm {

ExportedRights::~ExportedRights() { Clear(); }

void ExportedRights::Clear() noexcept {
  const int saved_errno = errno;
  for (std::size_t index = 0; index < descriptors_.size(); ++index) {
    if (index >= borrowed_.size() || borrowed_[index] == 0)
      (void)::close(descriptors_[index]);
  }
  descriptors_.clear();
  borrowed_.clear();
  errno = saved_errno;
}

ExportStatus ExportedRights::Decode(const void* control, std::size_t length,
                                    ExportDescriptor export_descriptor,
                                    void* context) {
  Clear();
  if (length != 0 && control == nullptr) return ExportStatus::MissingControl;
  if (export_descriptor == nullptr) return ExportStatus::InvalidControl;
  const auto* bytes = static_cast<const unsigned char*>(control);
  std::size_t offset = 0;
  try {
    while (length - offset >= sizeof(AndroidControlHeader)) {
      AndroidControlHeader header{};
      std::memcpy(&header, bytes + offset, sizeof(header));
      if (header.length < sizeof(header) || header.length > length - offset) {
        Clear();
        return ExportStatus::InvalidControl;
      }
      if (header.level != 1 || header.type != 1) {
        Clear();
        return ExportStatus::Unsupported;
      }
      const std::size_t payload = header.length - sizeof(header);
      if (payload % sizeof(int32_t) != 0) {
        Clear();
        return ExportStatus::InvalidControl;
      }
      const std::size_t count = payload / sizeof(int32_t);
      if (count > descriptors_.max_size() - descriptors_.size()) {
        Clear();
        return ExportStatus::OutOfMemory;
      }
      // Reserve before export so allocation failure cannot strand a newly
      // returned native descriptor outside local ownership.
      descriptors_.reserve(descriptors_.size() + count);
      borrowed_.reserve(borrowed_.size() + count);
      for (std::size_t index = 0; index < count; ++index) {
        int32_t guest_descriptor = -1;
        std::memcpy(&guest_descriptor,
                    bytes + offset + sizeof(header) + index * sizeof(int32_t),
                    sizeof(guest_descriptor));
        const int native_descriptor = export_descriptor(context, guest_descriptor);
        if (native_descriptor < 0) {
          Clear();
          return ExportStatus::DescriptorFailure;
        }
        descriptors_.push_back(native_descriptor);
        borrowed_.push_back(0);
      }
      constexpr std::size_t alignment = 8;
      if (header.length > std::numeric_limits<std::size_t>::max() - alignment + 1) {
        Clear();
        return ExportStatus::InvalidControl;
      }
      const std::size_t aligned = (header.length + alignment - 1) & ~(alignment - 1);
      // A final unpadded CMSG_LEN record is valid; do not read its padding.
      if (aligned > length - offset) break;
      offset += aligned;
    }
  } catch (const std::bad_alloc&) {
    Clear();
    return ExportStatus::OutOfMemory;
  }
  return ExportStatus::Ok;
}

ExportStatus ExportedRights::DecodeRetained(
    const void* control, std::size_t length,
    ExportDescriptorWithOwnership export_descriptor, void* context) {
  Clear();
  if (length != 0 && control == nullptr) return ExportStatus::MissingControl;
  if (export_descriptor == nullptr) return ExportStatus::InvalidControl;
  const auto* bytes = static_cast<const unsigned char*>(control);
  std::size_t offset = 0;
  try {
    while (length - offset >= sizeof(AndroidControlHeader)) {
      AndroidControlHeader header{};
      std::memcpy(&header, bytes + offset, sizeof(header));
      if (header.length < sizeof(header) || header.length > length - offset) {
        Clear();
        return ExportStatus::InvalidControl;
      }
      if (header.level != 1 || header.type != 1) {
        Clear();
        return ExportStatus::Unsupported;
      }
      const std::size_t payload = header.length - sizeof(header);
      if (payload % sizeof(int32_t) != 0) {
        Clear();
        return ExportStatus::InvalidControl;
      }
      const std::size_t count = payload / sizeof(int32_t);
      if (count > descriptors_.max_size() - descriptors_.size()) {
        Clear();
        return ExportStatus::OutOfMemory;
      }
      // Reserve both vectors before the first callback. A retained descriptor
      // must never be closed by this collection after ownership is published.
      descriptors_.reserve(descriptors_.size() + count);
      borrowed_.reserve(borrowed_.size() + count);
      for (std::size_t index = 0; index < count; ++index) {
        int32_t guest_descriptor = -1;
        std::memcpy(&guest_descriptor,
                    bytes + offset + sizeof(header) + index * sizeof(guest_descriptor),
                    sizeof(guest_descriptor));
        bool borrowed = false;
        const int native_descriptor =
            export_descriptor(context, guest_descriptor, &borrowed);
        if (native_descriptor < 0) {
          Clear();
          return ExportStatus::DescriptorFailure;
        }
        descriptors_.push_back(native_descriptor);
        borrowed_.push_back(borrowed ? 1 : 0);
      }
      constexpr std::size_t alignment = 8;
      if (header.length > std::numeric_limits<std::size_t>::max() - alignment + 1) {
        Clear();
        return ExportStatus::InvalidControl;
      }
      const std::size_t aligned = (header.length + alignment - 1) & ~(alignment - 1);
      if (aligned > length - offset) break;
      offset += aligned;
    }
  } catch (const std::bad_alloc&) {
    Clear();
    return ExportStatus::OutOfMemory;
  }
  return ExportStatus::Ok;
}

}  // namespace darwin_art::bionic::scm
