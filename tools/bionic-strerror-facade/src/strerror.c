#include "aim_bionic_strerror.h"

#include <errno.h>
#include <stdint.h>

enum { ANDROID_ERANGE = 34 };

typedef struct AndroidErrnoMessage {
  int32_t number;
  const char* message;
} AndroidErrnoMessage;

static const AndroidErrnoMessage kMessages[] = {
#include "android_errno_messages.inc"
};

_Static_assert(sizeof(int) == 4, "Android arm64 errno argument width drift");
_Static_assert(sizeof(size_t) == 8, "Android arm64 size_t width drift");

static size_t StringLength(const char* text) {
  size_t length = 0;
  while (text[length] != '\0') ++length;
  return length;
}

static const char* Lookup(int32_t number) {
  size_t low = 0;
  size_t high = sizeof(kMessages) / sizeof(kMessages[0]);
  while (low < high) {
    const size_t middle = low + (high - low) / 2;
    if (number == kMessages[middle].number) return kMessages[middle].message;
    if (number < kMessages[middle].number)
      high = middle;
    else
      low = middle + 1;
  }
  return NULL;
}

static size_t FormatUnknown(int32_t number, char output[32]) {
  static const char prefix[] = "Unknown error ";
  size_t length = sizeof(prefix) - 1;
  for (size_t index = 0; index < sizeof(prefix) - 1; ++index) {
    output[index] = prefix[index];
  }
  uint32_t magnitude;
  if (number < 0) {
    output[length++] = '-';
    magnitude = (uint32_t)(-(int64_t)number);
  } else {
    magnitude = (uint32_t)number;
  }
  char reverse[10];
  size_t digits = 0;
  do {
    reverse[digits++] = (char)('0' + magnitude % 10);
    magnitude /= 10;
  } while (magnitude != 0);
  while (digits != 0) output[length++] = reverse[--digits];
  output[length] = '\0';
  return length;
}

static int CopyResult(const char* message, size_t length, char* buffer,
                      size_t size) {
  if (size != 0 && buffer != NULL) {
    const size_t copied = length < size - 1 ? length : size - 1;
    for (size_t index = 0; index < copied; ++index) buffer[index] = message[index];
    buffer[copied] = '\0';
  }
  return length >= size ? ANDROID_ERANGE : 0;
}

int aim_bionic_strerror_r_core(int android_errno, char* buffer,
                                      size_t size) {
  const char* message = Lookup(android_errno);
  if (message != NULL) {
    return CopyResult(message, StringLength(message), buffer, size);
  }
  char unknown[32];
  const size_t length = FormatUnknown(android_errno, unknown);
  return CopyResult(unknown, length, buffer, size);
}

int aim_bionic_strerror_r(int android_errno, char* buffer, size_t size) {
  const int saved_host_errno = errno;
  const int result =
      aim_bionic_strerror_r_core(android_errno, buffer, size);
  errno = saved_host_errno;
  return result;
}

char* aim_bionic___gnu_strerror_r(int android_errno, char* buffer,
                                        size_t size) {
  const int saved_host_errno = errno;
  (void)aim_bionic_strerror_r_core(android_errno, buffer, size);
  errno = saved_host_errno;
  return buffer;
}

char* aim_bionic_strerror(int android_errno) {
  static _Thread_local char message[64];
  const int saved_host_errno = errno;
  (void)aim_bionic_strerror_r_core(android_errno, message,
                                          sizeof(message));
  errno = saved_host_errno;
  return message;
}

static int NameEquals(const char* left, const char* right) {
  if (left == NULL || right == NULL) return 0;
  while (*left == *right && *left != '\0') {
    ++left;
    ++right;
  }
  return *left == *right;
}

AimBionicStrerrorFunction aim_bionic_strerror_resolve(
    const char* import_name) {
  if (NameEquals(import_name, "__gnu_strerror_r"))
    return (AimBionicStrerrorFunction)aim_bionic___gnu_strerror_r;
  if (NameEquals(import_name, "strerror"))
    return (AimBionicStrerrorFunction)aim_bionic_strerror;
  if (NameEquals(import_name, "strerror_r"))
    return (AimBionicStrerrorFunction)aim_bionic_strerror_r;
  return NULL;
}
