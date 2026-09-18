#include "retained_scm_export.h"

#include <cerrno>
#include <new>
#include <utility>
#include <unistd.h>

namespace darwin_art::bionic::scm {
namespace {

struct CaptureContext {
  int host_fd = -1;
  DarwinArtFdDescriptionSnapshotV1 snapshot{};
};

intptr_t CaptureExport(void *context,
                       const DarwinArtFdDescriptionSnapshotV1 *snapshot,
                       int host_fd, int *android_errno) {
  auto *capture = static_cast<CaptureContext *>(context);
  if (capture == nullptr || snapshot == nullptr || host_fd < 0 ||
      android_errno == nullptr) {
    if (android_errno != nullptr)
      *android_errno = 14;
    return -1;
  }
  capture->snapshot = *snapshot;
  capture->host_fd = host_fd;
  *android_errno = 0;
  return 0;
}

void ReleaseExport(void *context, int host_fd) {
  const int saved_errno = errno;
  auto *capture = static_cast<CaptureContext *>(context);
  if (capture != nullptr && capture->host_fd == host_fd)
    capture->host_fd = -1;
  if (host_fd >= 0)
    (void)::close(host_fd);
  errno = saved_errno;
}

void CloseAndRelease(DarwinArtFdBroker *broker, DarwinArtFdDescriptionPin *pin,
                     int host_fd) noexcept {
  const int saved_errno = errno;
  if (host_fd >= 0)
    (void)::close(host_fd);
  if (broker != nullptr && pin != nullptr)
    (void)darwin_art_fd_broker_release_description(broker, pin);
  errno = saved_errno;
}

} // namespace

RetainedScmExport::RetainedScmExport(
    DarwinArtFdBroker *broker, DarwinArtFdDescriptionPin *pin, int host_fd,
    const DarwinArtFdDescriptionSnapshotV1 &snapshot) noexcept
    : broker_(broker), pin_(pin), host_fd_(host_fd), snapshot_(snapshot) {}

RetainedScmExport::~RetainedScmExport() noexcept {
  CloseAndRelease(broker_, pin_, host_fd_);
  broker_ = nullptr;
  pin_ = nullptr;
  host_fd_ = -1;
}

int RetainedScmExport::take_host_fd() noexcept {
  return std::exchange(host_fd_, -1);
}

DarwinArtFdBrokerStatus
RetainedScmExport::Create(DarwinArtFdBroker *broker, int guest_descriptor,
                          std::unique_ptr<RetainedScmExport> *export_result,
                          DarwinArtFdIoResult *result) noexcept {
  if (export_result == nullptr || result == nullptr || broker == nullptr) {
    if (result != nullptr) {
      result->value = -1;
      result->android_errno = EINVAL;
    }
    return DARWIN_ART_FD_BROKER_INVALID_ARGUMENT;
  }
  export_result->reset();
  result->value = -1;
  result->android_errno = 0;

  CaptureContext capture;
  DarwinArtFdDescriptionPin *pin = nullptr;
  const DarwinArtFdBrokerStatus status =
      darwin_art_fd_broker_retain_exported_description(
          broker, guest_descriptor, &CaptureExport, &ReleaseExport, &capture,
          &pin, result);
  if (status != DARWIN_ART_FD_BROKER_OK || pin == nullptr ||
      capture.host_fd < 0) {
    CloseAndRelease(broker, pin, capture.host_fd);
    return status;
  }

  auto retained = std::unique_ptr<RetainedScmExport>(
      new (std::nothrow)
          RetainedScmExport(broker, pin, capture.host_fd, capture.snapshot));
  if (retained == nullptr) {
    CloseAndRelease(broker, pin, capture.host_fd);
    result->value = -1;
    result->android_errno = ENOMEM;
    return DARWIN_ART_FD_BROKER_EXHAUSTED;
  }
  result->value = capture.host_fd;
  result->android_errno = 0;
  *export_result = std::move(retained);
  return DARWIN_ART_FD_BROKER_OK;
}

ScopedCarrierExport::ScopedCarrierExport(
    std::unique_ptr<RetainedScmExport> retained) noexcept
    : retained_(std::move(retained)) {}

ScopedCarrierExport::ScopedCarrierExport(int host_fd) noexcept
    : host_fd_(host_fd) {}

ScopedCarrierExport::ScopedCarrierExport(ScopedCarrierExport &&other) noexcept
    : retained_(std::move(other.retained_)), host_fd_(other.host_fd_) {
  other.host_fd_ = -1;
}

ScopedCarrierExport::~ScopedCarrierExport() noexcept {
  const int saved_errno = errno;
  retained_.reset();
  if (host_fd_ >= 0) {
    (void)::close(host_fd_);
    host_fd_ = -1;
  }
  errno = saved_errno;
}

int ScopedCarrierExport::host_fd() const noexcept {
  return retained_ != nullptr ? retained_->host_fd() : host_fd_;
}

const DarwinArtFdDescriptionSnapshotV1 *
ScopedCarrierExport::snapshot() const noexcept {
  return retained_ != nullptr ? &retained_->snapshot() : nullptr;
}

DarwinArtFdBrokerStatus ScopedCarrierExport::Create(
    DarwinArtFdBroker *broker, int guest_descriptor, bool central_namespace,
    UnmanagedExport unmanaged_export,
    std::unique_ptr<ScopedCarrierExport> *export_result,
    DarwinArtFdIoResult *result) noexcept {
  if (export_result == nullptr || result == nullptr ||
      (central_namespace && broker == nullptr) ||
      (!central_namespace && unmanaged_export == nullptr)) {
    if (result != nullptr) {
      result->value = -1;
      result->android_errno = EINVAL;
    }
    return DARWIN_ART_FD_BROKER_INVALID_ARGUMENT;
  }
  export_result->reset();
  result->value = -1;
  result->android_errno = 0;

  if (central_namespace) {
    std::unique_ptr<RetainedScmExport> retained;
    const DarwinArtFdBrokerStatus status = RetainedScmExport::Create(
        broker, guest_descriptor, &retained, result);
    if (status != DARWIN_ART_FD_BROKER_OK || retained == nullptr)
      return status == DARWIN_ART_FD_BROKER_OK
                 ? DARWIN_ART_FD_BROKER_EXHAUSTED
                 : status;
    result->value = retained->host_fd();
    *export_result = std::unique_ptr<ScopedCarrierExport>(
        new (std::nothrow) ScopedCarrierExport(std::move(retained)));
    if (*export_result == nullptr) {
      result->value = -1;
      result->android_errno = ENOMEM;
      return DARWIN_ART_FD_BROKER_EXHAUSTED;
    }
    return DARWIN_ART_FD_BROKER_OK;
  }

  // The unmanaged callback owns the existing guest-errno behavior. A failed
  // callback therefore returns OK with no carrier so callers can return its
  // failure without replacing the callback's errno state.
  const int host_fd = unmanaged_export(guest_descriptor);
  if (host_fd < 0)
    return DARWIN_ART_FD_BROKER_OK;
  auto scoped = std::unique_ptr<ScopedCarrierExport>(
      new (std::nothrow) ScopedCarrierExport(host_fd));
  if (scoped == nullptr) {
    const int saved_errno = errno;
    (void)::close(host_fd);
    errno = saved_errno;
    result->android_errno = ENOMEM;
    return DARWIN_ART_FD_BROKER_EXHAUSTED;
  }
  result->value = host_fd;
  *export_result = std::move(scoped);
  return DARWIN_ART_FD_BROKER_OK;
}

} // namespace darwin_art::bionic::scm
