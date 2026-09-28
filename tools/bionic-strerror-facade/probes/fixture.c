#include <errno.h>
#include <stddef.h>
#include <string.h>

static int Equal(const char* left, const char* right) {
  while (*left == *right && *left != '\0') {
    ++left;
    ++right;
  }
  return *left == *right;
}

__attribute__((visibility("default"))) int bionic_strerror_fixture_run(void) {
  char buffer[64];
  errno = 777;
  if (strerror_r(EINVAL, buffer, sizeof(buffer)) != 0 ||
      !Equal(buffer, "Invalid argument") || errno != 777)
    return 1;
  if (strerror_r(EADDRINUSE, buffer, sizeof(buffer)) != 0 ||
      !Equal(buffer, "Address already in use") || errno != 777)
    return 2;
  if (strerror_r(ENONET, buffer, sizeof(buffer)) != 0 ||
      !Equal(buffer, "Machine is not on the network") || errno != 777)
    return 3;
  if (strerror_r(666, buffer, sizeof(buffer)) != 0 ||
      !Equal(buffer, "Unknown error 666") || errno != 777)
    return 4;
  if (strerror_r(-7, buffer, sizeof(buffer)) != 0 ||
      !Equal(buffer, "Unknown error -7") || errno != 777)
    return 5;

  buffer[0] = 'x';
  if (strerror_r(EINVAL, buffer, 0) != ERANGE || buffer[0] != 'x' ||
      errno != 777)
    return 6;
  buffer[0] = 'x';
  if (strerror_r(EINVAL, buffer, 1) != ERANGE || buffer[0] != '\0' ||
      errno != 777)
    return 7;
  if (strerror_r(EINVAL, buffer, sizeof("Invalid argument")) != 0 ||
      !Equal(buffer, "Invalid argument") || errno != 777)
    return 8;
  if (strerror_r(EINVAL, buffer, sizeof("Invalid argument") - 1) != ERANGE ||
      !Equal(buffer, "Invalid argumen") || errno != 777)
    return 9;
  return 42;
}
