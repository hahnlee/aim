#ifndef DARWIN_ART_ANDROID_SCM_EXPORTS_H_
#define DARWIN_ART_ANDROID_SCM_EXPORTS_H_

#include <cstddef>
#include <cstdint>
#include <span>
#include <vector>

namespace darwin_art::bionic::scm {

struct AndroidControlHeader {
  uint64_t length;
  int32_t level;
  int32_t type;
};
static_assert(sizeof(AndroidControlHeader) == 16);

enum class ExportStatus { Ok, InvalidControl, MissingControl, Unsupported,
                          DescriptorFailure, OutOfMemory };

// The port returns a newly owned native descriptor or -1 with the guest
// descriptor provider's error already installed. Android FD policy remains
// with that provider, not the control-layout codec.
using ExportDescriptor = int (*)(void* context, int guest_descriptor);
using ExportDescriptorWithOwnership = int (*)(void* context,
                                              int guest_descriptor,
                                              bool* borrowed);

class ExportedRights final {
 public:
  ExportedRights() = default;
  ~ExportedRights();
  ExportedRights(const ExportedRights&) = delete;
  ExportedRights& operator=(const ExportedRights&) = delete;

  ExportStatus Decode(const void* control, std::size_t length,
                      ExportDescriptor export_descriptor, void* context);
  ExportStatus DecodeRetained(const void* control, std::size_t length,
                              ExportDescriptorWithOwnership export_descriptor,
                              void* context);
  std::span<const int> descriptors() const { return descriptors_; }
  void Clear() noexcept;

 private:
  std::vector<int> descriptors_;
  // A borrowed entry is owned by a separate scoped provider object and must
  // not be closed by this collection. The vector is reserved before callbacks
  // so ownership bookkeeping cannot fail after a descriptor is returned.
  std::vector<unsigned char> borrowed_;
};

}  // namespace darwin_art::bionic::scm
#endif
