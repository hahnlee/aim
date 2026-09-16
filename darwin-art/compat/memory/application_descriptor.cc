#include "application_descriptor.h"
#include "application_memory.h"
#include <cerrno>
#include <cstdint>
#include <fcntl.h>
#include <sys/mman.h>
#include <unistd.h>

extern "C" int darwin_art_bionic_fs_adopt_host_fd_core(int);
extern "C" int darwin_art_bionic_fs_dup_host_fd_core(int, int*);
extern "C" int darwin_art_bionic_fs_close_core(int);
extern "C" int darwin_art_bionic_errno_load();
extern "C" int darwin_art_bionic_errno_from_darwin(int, int32_t*);

namespace darwin_art::memory {
namespace {
void SetHostError() {
  const int guest = darwin_art_bionic_errno_load();
  for (int host = 1; host <= ELAST; ++host) {
    int32_t mapped = 0;
    if (darwin_art_bionic_errno_from_darwin(host, &mapped) && mapped == guest) {
      errno = host;
      return;
    }
  }
  errno = EIO;
}
int Publish(int native) {
  if (native < 0) return -1;
  // adopt consumes native even on failure.
  int guest = darwin_art_bionic_fs_adopt_host_fd_core(native);
  if (guest < 0) SetHostError();
  return guest;
}
int Borrow(int guest) {
  int native = -1;
  int result = darwin_art_bionic_fs_dup_host_fd_core(guest, &native);
  if (result == 1) return native;
  if (result < 0) SetHostError();
  else errno = EBADF;
  return -1;
}
}
int CreateApplicationDescriptor(size_t size) noexcept {
  int native = CreateApplicationMemory(size);
  if (native < 0) return -1;
  // Retain a live capability for cleanup if publication consumes its duplicate
  // and fails. Never use an already-closed, potentially recycled FD as a key.
  int guest = Publish(fcntl(native, F_DUPFD_CLOEXEC, 0));
  int error = errno;
  if (guest < 0) AbandonApplicationMemory(native);
  close(native);
  errno = error;
  return guest;
}
void* MapApplicationDescriptor(int guest, size_t size, bool writable) noexcept {
  int native = Borrow(guest);
  if (native < 0) return MAP_FAILED;
  void* result = MapApplicationMemory(native, size, writable);
  int error = errno;
  if (result == MAP_FAILED) AbandonApplicationMemory(native);
  close(native);
  errno = error;
  return result;
}
int DuplicateApplicationDescriptorReader(int guest) noexcept {
  int native = Borrow(guest);
  if (native < 0) return -1;
  int reader = DuplicateApplicationMemoryReader(native);
  int error = errno;
  close(native);
  errno = error;
  return Publish(reader);
}
int CloseApplicationDescriptor(int guest) noexcept {
  int native = Borrow(guest);
  if (native >= 0) {
    AbandonApplicationMemory(native);
    close(native);
  }
  int result = darwin_art_bionic_fs_close_core(guest);
  if (result < 0) SetHostError();
  return result;
}
}
