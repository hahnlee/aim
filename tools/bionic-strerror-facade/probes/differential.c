#include "aim_bionic_strerror.h"

#include <errno.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>

typedef struct AndroidErrnoMessage {
  int32_t number;
  const char* message;
} AndroidErrnoMessage;

static const AndroidErrnoMessage kAospMessages[] = {
#include "android_errno_messages.inc"
};

static const char* ReferenceLookup(int number) {
  for (size_t index = 0;
       index < sizeof(kAospMessages) / sizeof(kAospMessages[0]); ++index) {
    if (kAospMessages[index].number == number) return kAospMessages[index].message;
  }
  return NULL;
}

static int Reference(int number, char* buffer, size_t size) {
  const char* known = ReferenceLookup(number);
  char unknown[32];
  if (known == NULL) {
    int written = snprintf(unknown, sizeof(unknown), "Unknown error %d", number);
    if (written < 0 || (size_t)written >= sizeof(unknown)) return -1;
    known = unknown;
  }
  const size_t length = strlen(known);
  if (size != 0) {
    const size_t copied = length < size - 1 ? length : size - 1;
    memcpy(buffer, known, copied);
    buffer[copied] = '\0';
  }
  return length >= size ? 34 : 0;
}

int main(void) {
  if (aim_bionic_strerror_resolve("__gnu_strerror_r") == NULL ||
      aim_bionic_strerror_resolve("strerror_r") == NULL ||
      aim_bionic_strerror_resolve("strerror") == NULL ||
      aim_bionic_strerror_resolve("strsignal") != NULL)
    return 1;
  for (int number = -256; number <= 512; ++number) {
    for (size_t size = 0; size <= 64; ++size) {
      char expected[80];
      char actual[80];
      memset(expected, 0x5a, sizeof(expected));
      memset(actual, 0x5a, sizeof(actual));
      const int expected_result = Reference(number, expected, size);
      errno = 1234;
      const int actual_result =
          aim_bionic_strerror_r(number, actual, size);
      if (errno != 1234 || actual_result != expected_result ||
          memcmp(expected, actual, sizeof(actual)) != 0)
        return 2;

      memset(actual, 0x5a, sizeof(actual));
      errno = 2345;
      char* returned =
          aim_bionic___gnu_strerror_r(number, actual, size);
      if (errno != 2345 || returned != actual ||
          memcmp(expected, actual, sizeof(actual)) != 0)
        return 3;
    }
  }
  return 0;
}
