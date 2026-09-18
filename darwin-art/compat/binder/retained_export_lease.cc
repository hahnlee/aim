#include "retained_export_lease.h"

#include "darwin_art_bionic_binder_fd.h"
#include "darwin_art_bionic_socket_broker.h"
#include "retained_scm_export.h"

#include <cerrno>
#include <cstdint>
#include <memory>
#include <new>
#include <unistd.h>
#include <utility>

namespace darwin_art::binder {
namespace {

constexpr uint32_t kCentralBrokerTokenMarker = UINT32_C(0x40000000);
constexpr uint32_t kCentralBrokerTokenTopMask = UINT32_C(0xc0000000);

using RetainedScmExport = darwin_art::bionic::scm::RetainedScmExport;

struct RetainedExportLease final {
  RetainedExportLease(void *process_cookie,
                      std::unique_ptr<RetainedScmExport> retained) noexcept
      : process_cookie(process_cookie), retained(std::move(retained)) {}

  ~RetainedExportLease() noexcept {
    const int saved_errno = errno;
    // The Description pin must be released before the Process reference.  The
    // broker is owned by Process and can otherwise be destroyed too early.
    retained.reset();
    if (process_cookie != nullptr) {
      darwin_art_bionic_binder_fd_release_process(process_cookie);
      process_cookie = nullptr;
    }
    errno = saved_errno;
  }

  RetainedExportLease(const RetainedExportLease &) = delete;
  RetainedExportLease &operator=(const RetainedExportLease &) = delete;

  void *process_cookie = nullptr;
  std::unique_ptr<RetainedScmExport> retained;
};

struct ProcessCookieGuard final {
  ~ProcessCookieGuard() noexcept {
    const int saved_errno = errno;
    if (cookie != nullptr)
      darwin_art_bionic_binder_fd_release_process(cookie);
    errno = saved_errno;
  }

  void *cookie = nullptr;
};

bool IsCentralDescriptor(int guest_fd) noexcept {
  const auto value = static_cast<uint32_t>(guest_fd);
  return (value & kCentralBrokerTokenTopMask) == kCentralBrokerTokenMarker;
}

void ClearResult(DarwinArtBinderRetainedExportedDescriptor *result) noexcept {
  if (result == nullptr)
    return;
  result->host_fd = -1;
  result->attributes_length = 0;
  for (uint8_t &attribute : result->attributes)
    attribute = 0;
  result->lease = nullptr;
}

int ExportRetainedFileDescriptorImpl(
    int guest_fd, const DarwinArtBinderTransferBinding *binding,
    DarwinArtBinderRetainedExportedDescriptor *result) noexcept {
  ClearResult(result);
  if (binding == nullptr || result == nullptr)
    return -1;

  ProcessCookieGuard process_guard;
  DarwinArtFdBroker *broker = nullptr;
  if (darwin_art_bionic_binder_fd_acquire_process(&process_guard.cookie,
                                                  &broker) !=
          0 ||
      process_guard.cookie == nullptr || broker == nullptr) {
    return -1;
  }

  if (IsCentralDescriptor(guest_fd)) {
    std::unique_ptr<RetainedScmExport> retained;
    DarwinArtFdIoResult export_result{};
    const auto status = RetainedScmExport::Create(
        broker, guest_fd, &retained, &export_result);
    if (status != DARWIN_ART_FD_BROKER_OK || retained == nullptr) {
      return -1;
    }

    // Allocate the lease before taking the FD so allocation failure leaves
    // RetainedScmExport responsible for both the FD and its Description pin.
    auto *lease = new (std::nothrow)
        RetainedExportLease(process_guard.cookie, std::move(retained));
    if (lease == nullptr) {
      return -1;
    }
    process_guard.cookie = nullptr;
    result->host_fd = lease->retained->take_host_fd();
    if (result->host_fd < 0) {
      delete lease;
      ClearResult(result);
      return -1;
    }
    result->lease = lease;
    return 0;
  }

  // Filesystem/shared-memory descriptors intentionally remain unmanaged.  The
  // existing adapter export owns the returned host FD for the caller; this
  // local lease only keeps the adapter Process alive until release.
  const int host_fd = darwin_art_bionic_fd_export_for_scm(guest_fd);
  if (host_fd < 0) {
    return -1;
  }
  auto *lease = new (std::nothrow)
      RetainedExportLease(process_guard.cookie, nullptr);
  if (lease == nullptr) {
    const int saved_errno = errno;
    (void)::close(host_fd);
    errno = saved_errno;
    return -1;
  }
  process_guard.cookie = nullptr;
  result->host_fd = host_fd;
  result->lease = lease;
  return 0;
}

} // namespace

int ExportRetainedFileDescriptor(
    int guest_fd, const DarwinArtBinderTransferBinding *binding,
    DarwinArtBinderRetainedExportedDescriptor *result) noexcept {
  // Every acquisition uses explicit failure results/nothrow allocation. Native
  // owner callbacks must obey the no-unwind C ABI; an outer catch cannot repair
  // a violation inside the noexcept acquisition path.
  return ExportRetainedFileDescriptorImpl(guest_fd, binding, result);
}

void ReleaseRetainedFileDescriptorLease(void *opaque) noexcept {
  delete static_cast<RetainedExportLease *>(opaque);
}

} // namespace darwin_art::binder
