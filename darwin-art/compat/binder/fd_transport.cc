#include "fd_transport.h"

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
