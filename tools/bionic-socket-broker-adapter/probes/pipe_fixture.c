#include <errno.h>
#include <poll.h>
#include <stdint.h>
#include <unistd.h>

typedef struct PipeFixtureResult {
  int32_t empty_ready;
  int32_t readable_ready;
  int32_t read_revents;
  int32_t last_android_errno;
  intptr_t write_count;
  intptr_t read_count;
  uint8_t value;
} PipeFixtureResult;

__attribute__((visibility("default"))) int
PipeFixtureRoundTrip(PipeFixtureResult *result) {
  if (result == 0)
    return -1;
  int descriptors[2] = {-1, -1};
  if (pipe(descriptors) != 0)
    return -2;
  struct pollfd readable = {descriptors[0], POLLIN, 0};
  result->empty_ready = poll(&readable, 1, 0);
  const uint8_t sent = 0xa5;
  result->write_count = write(descriptors[1], &sent, sizeof(sent));
  readable.revents = 0;
  result->readable_ready = poll(&readable, 1, 1000);
  result->read_revents = readable.revents;
  result->read_count =
      read(descriptors[0], &result->value, sizeof(result->value));
  result->last_android_errno = errno;
  const int close_read = close(descriptors[0]);
  const int close_write = close(descriptors[1]);
  return result->empty_ready == 0 && result->write_count == 1 &&
                 result->readable_ready == 1 &&
                 (result->read_revents & POLLIN) != 0 &&
                 result->read_count == 1 && result->value == sent &&
                 close_read == 0 && close_write == 0
             ? 0
             : -3;
}
