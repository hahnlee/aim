#ifndef AIM_BIONIC_RETAINED_SCM_EXPORT_H_
#define AIM_BIONIC_RETAINED_SCM_EXPORT_H_

#include "aim_bionic_fd_broker.h"

#include <cstdint>
#include <memory>

namespace aim::bionic::scm {

// Owns one broker-exported host descriptor and the exact Description pin from
// which it was exported. The snapshot is copied from that same retain
// operation; no guest descriptor lookup or capability metadata is performed.
class RetainedScmExport final {
public:
  static AimFdBrokerStatus
  Create(AimFdBroker *broker, int guest_descriptor,
         std::unique_ptr<RetainedScmExport> *export_result,
         AimFdIoResult *result) noexcept;

  ~RetainedScmExport() noexcept;
  RetainedScmExport(const RetainedScmExport &) = delete;
  RetainedScmExport &operator=(const RetainedScmExport &) = delete;

  int host_fd() const noexcept { return host_fd_; }
  // Transfer only descriptor ownership to the private transport. This object
  // still owns the exact Description pin until the real deposit ACK or error.
  // The pin is local provider state and must never enter serialized metadata.
  int take_host_fd() noexcept;
  const AimFdDescriptionSnapshotV1 &snapshot() const noexcept {
    return snapshot_;
  }

private:
  RetainedScmExport(AimFdBroker *broker, AimFdDescriptionPin *pin,
                    int host_fd,
                    const AimFdDescriptionSnapshotV1 &snapshot) noexcept;

  AimFdBroker *broker_ = nullptr;
  AimFdDescriptionPin *pin_ = nullptr;
  int host_fd_ = -1;
  AimFdDescriptionSnapshotV1 snapshot_{};
};

// Owns the carrier descriptor for one sendmsg/sendmmsg operation. Managed
// descriptors retain their Description for the owner lifetime; unmanaged
// descriptors use the adapter's existing export callback and own that result.
// The namespace decision is supplied by the adapter, never inferred from the
// carrier's numeric value.
class ScopedCarrierExport final {
public:
  using UnmanagedExport = int (*)(int guest_descriptor);

  static AimFdBrokerStatus
  Create(AimFdBroker *broker, int guest_descriptor,
         bool central_namespace, UnmanagedExport unmanaged_export,
         std::unique_ptr<ScopedCarrierExport> *export_result,
         AimFdIoResult *result) noexcept;

  ~ScopedCarrierExport() noexcept;
  ScopedCarrierExport(const ScopedCarrierExport &) = delete;
  ScopedCarrierExport &operator=(const ScopedCarrierExport &) = delete;
  ScopedCarrierExport(ScopedCarrierExport &&other) noexcept;
  ScopedCarrierExport &operator=(ScopedCarrierExport &&) = delete;

  int host_fd() const noexcept;
  const AimFdDescriptionSnapshotV1 *snapshot() const noexcept;

private:
  explicit ScopedCarrierExport(std::unique_ptr<RetainedScmExport> retained)
      noexcept;
  explicit ScopedCarrierExport(int host_fd) noexcept;

  std::unique_ptr<RetainedScmExport> retained_;
  int host_fd_ = -1;
};

} // namespace aim::bionic::scm

#endif // AIM_BIONIC_RETAINED_SCM_EXPORT_H_
