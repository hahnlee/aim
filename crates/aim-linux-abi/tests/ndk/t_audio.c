// Audio host-call v1, as the audio HAL opens an input ring. No start:
// opening must not ask for microphone access. The selected device rate
// can be 16/24 kHz on a Bluetooth headset (#439).
#include <stdint.h>
#include <sys/mman.h>
#include <sys/syscall.h>
#include <unistd.h>

#include "check.h"

struct device {
  uint32_t present, sample_rate, channels, buffer_frames, latency_frames, reserved;
  char name[64];
};
struct devices {
  struct device output, input;
};
struct audio_open {
  uint32_t direction, format, sample_rate, channels;
  int32_t fd;
  uint32_t reserved;
  uint64_t length, stream;
};

_Static_assert(sizeof(struct devices) == 176, "audio Devices ABI");
_Static_assert(sizeof(struct audio_open) == 40, "audio Open ABI");

static void input_open(void) {
  struct devices devices = {0};
  CHECK(syscall(0x48430000, 7, 1, &devices, sizeof devices) == 0);
  printf("input present %u, rate %u Hz, channels %u\n", devices.input.present,
         devices.input.sample_rate, devices.input.channels);
  int fd = memfd_create("audio-input", MFD_CLOEXEC);
  CHECK(fd >= 0);
  for (unsigned format = 1; format <= 2; format++) {
    for (unsigned channels = 1; channels <= 2; channels++) {
      uint32_t header[] = {1024, channels * (format == 1 ? 2 : 4)};
      uint64_t length = 256 + header[0] * header[1];
      CHECK(ftruncate(fd, length) == 0);
      CHECK(pwrite(fd, header, sizeof header, 0) == sizeof header);
      struct audio_open args = {
          .direction = 1, .format = format,
          .sample_rate = devices.input.present ? devices.input.sample_rate : 48000,
          .channels = channels, .fd = fd, .length = length};
      long result = syscall(0x48430000, 7, 2, &args, sizeof args);
      if (!devices.input.present) {
        CHECK(result == -1 && errno == ENODEV);
        continue;
      }
      CHECK(result == 0 && args.stream != 0);
      CHECK(syscall(0x48430000, 7, 5, &args.stream, sizeof args.stream) == 0);
      CHECK(syscall(0x48430000, 7, 5, &args.stream, sizeof args.stream) == -1 &&
            errno == EINVAL);
      args.sample_rate++;
      CHECK(syscall(0x48430000, 7, 2, &args, sizeof args) == -1 && errno == EINVAL);
    }
  }
  CHECK(close(fd) == 0);
}

int main(void) {
  RUN(input_open);
  DONE();
}
