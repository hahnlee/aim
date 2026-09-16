#include "graphics/image_decoder_input.h"
#include <cassert>
#include <cerrno>
#include <cstdio>
#include <fcntl.h>
#include <unistd.h>

// Unit seam only. Production obtains the duplicate from the Rust FD owner.
static int backing = -1;
static int duplicate = -1;
extern "C" int darwin_art_bionic_fs_dup_host_fd_core(int guest, int* host) {
  if (guest != 0 || backing < 0) return 0;
  duplicate = fcntl(backing, F_DUPFD_CLOEXEC, 0);
  *host = duplicate;
  return duplicate < 0 ? -1 : 1;
}
int main() {
  using darwin_art::graphics::OpenImageDecoderInput;
  FILE* seed = tmpfile();
  assert(seed);
  backing = fileno(seed);
  assert(write(backing, "abcd", 4) == 4);
  assert(lseek(backing, 1, SEEK_SET) == 1);
  assert(!OpenImageDecoderInput(-1));
  // A live host descriptor is not implicitly an Android descriptor.
  assert(backing != 0 && !OpenImageDecoderInput(backing));
  FILE* stream = OpenImageDecoderInput(0);
  assert(stream && duplicate != backing);
  assert((fcntl(duplicate, F_GETFD) & FD_CLOEXEC) != 0);
  assert(fgetc(stream) == 'b');
  fclose(stream);
  assert(fcntl(backing, F_GETFD) >= 0);
  assert(fcntl(duplicate, F_GETFD) == -1 && errno == EBADF);
  // Closing the caller's descriptor cannot invalidate the retained stream.
  stream = OpenImageDecoderInput(0);
  assert(stream);
  fclose(seed);
  backing = -1;
  assert(fseek(stream, 0, SEEK_SET) == 0 && fgetc(stream) == 'a');
  fclose(stream);
  int pipe_fds[2];
  assert(pipe(pipe_fds) == 0);
  backing = pipe_fds[0];
  assert(!OpenImageDecoderInput(0));
  assert(fcntl(duplicate, F_GETFD) == -1 && errno == EBADF);
  assert(fcntl(backing, F_GETFD) >= 0);
  close(pipe_fds[0]);
  close(pipe_fds[1]);
  std::puts("image decoder FD boundary: borrowed guest, owned stream, rejection cleanup PASS");
}
