#include "fd_transport.h"

#include "retained_export_lease.h"
#include "socket_endpoint_exports.h"

#include <cerrno>
#include <unistd.h>

extern "C" int darwin_art_bionic_fd_export_for_scm(int guest_fd);
extern "C" int darwin_art_bionic_fd_import_from_scm(int host_fd);
extern "C" int darwin_art_bionic_socket_broker_close(int guest_fd);
extern "C" int darwin_art_bionic_socket_broker_dup(int guest_fd);

extern "C" int darwin_art_binder_export_file_descriptor(int guest_fd) {
  return darwin_art_bionic_fd_export_for_scm(guest_fd);
}

extern "C" int darwin_art_binder_import_file_descriptor(int host_fd) {
  return darwin_art_bionic_fd_import_from_scm(host_fd);
}

extern "C" int darwin_art_binder_close_file_descriptor(int guest_fd) {
  return darwin_art_bionic_socket_broker_close(guest_fd);
}

extern "C" int darwin_art_binder_duplicate_file_descriptor(int guest_fd) {
  return darwin_art_bionic_socket_broker_dup(guest_fd);
}

extern "C" int darwin_art_binder_export_retained_file_descriptor(
    int guest_fd, const DarwinArtBinderTransferBinding *binding,
    DarwinArtBinderRetainedExportedDescriptor *result) {
  return darwin_art::binder::ExportRetainedFileDescriptor(guest_fd, binding,
                                                         result);
}

extern "C" void darwin_art_binder_release_export_lease(void *lease) {
  darwin_art::binder::ReleaseRetainedFileDescriptorLease(lease);
}

extern "C" int darwin_art_binder_export_bound_file_descriptor(
    int guest_fd, const DarwinArtBinderTransferBinding *binding,
    DarwinArtBinderExportedDescriptor *result) {
  if (result == nullptr || binding == nullptr) { errno = EINVAL; return -1; }
  *result = {};
  result->host_fd = -1;
  // Managed descriptions require the retained port and genuine deposit ACK.
  // The bare export port refuses them rather than emitting untagged aliases.
  const int descriptor = darwin_art_bionic_fd_export_for_scm(guest_fd);
  if (descriptor < 0) return -1;
  result->host_fd = descriptor;
  return 0;
}

extern "C" int darwin_art_binder_import_bound_file_descriptor(
    int host_fd, const DarwinArtBinderTransferBinding *binding,
    const uint8_t *attributes, size_t attributes_length) {
  if (binding == nullptr || (attributes_length != 0 && attributes == nullptr)) {
    if (host_fd >= 0) (void)::close(host_fd);
    errno = EINVAL;
    return -1;
  }
  if (attributes_length == 0)
    return darwin_art_bionic_fd_import_from_scm(host_fd);
  const DarwinArtScmBinderBindingV2 bound{binding->source_connection,
      binding->transfer, binding->ordinal, binding->object_offset};
  return darwin_art::bionic::scm::ImportBinderEndpoint(
      host_fd, bound, attributes, attributes_length);
}
