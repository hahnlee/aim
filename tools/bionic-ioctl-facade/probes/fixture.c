#include <stdint.h>

extern int ioctl(int fd, int request, ...);

enum {
  RNDGETENTCNT = (int)0x80045200U,
  UNKNOWN_REQUEST = 0x12345678,
};

__attribute__((visibility("default"))) int IoctlFixtureEntropy(int fd,
                                                                int32_t* out) {
  return ioctl(fd, RNDGETENTCNT, out);
}

__attribute__((visibility("default"))) int IoctlFixtureUnknown(int fd,
                                                                int32_t* out) {
  return ioctl(fd, UNKNOWN_REQUEST, out);
}

__attribute__((visibility("default"))) int IoctlFixtureNull(int fd) {
  return ioctl(fd, RNDGETENTCNT, (void*)0);
}
