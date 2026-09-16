#include "image_decoder_input.h"
#include "darwin_art_bionic_fs.h"
#include <sys/stat.h>
#include <unistd.h>

namespace darwin_art::graphics {
FILE* OpenImageDecoderInput(int guest_fd) {
  if (guest_fd < 0) return nullptr;
  int host_fd = -1;
  if (darwin_art_bionic_fs_dup_host_fd_core(guest_fd, &host_fd) != 1) {
    return nullptr;
  }
  // The Rust descriptor owner retained the source while duplicating it. All
  // validation now uses that owned duplicate, never a racy borrowed host FD.
  struct stat metadata;
  if (fstat(host_fd, &metadata) != 0 || lseek(host_fd, 0, SEEK_CUR) == -1) {
    close(host_fd);
    return nullptr;
  }
  FILE* stream = fdopen(host_fd, "r");
  if (!stream) close(host_fd);
  return stream;
}
}
