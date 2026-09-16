#include <stdint.h>

/* Isolated ioctl tests have no central FD broker; report unhandled. */
extern "C" int darwin_art_bionic_fd_broker_ioctl_dispatch(
    int fd, uint32_t request, void* argument, int* handled, int* result,
    int* android_errno) {
  (void)fd;
  (void)request;
  (void)argument;
  if (handled != nullptr) *handled = 0;
  if (result != nullptr) *result = -1;
  if (android_errno != nullptr) *android_errno = 0;
  return 0;
}
