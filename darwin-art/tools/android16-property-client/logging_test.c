#include "darwin_art_bionic_errno.h"
#include <errno.h>
#include <string.h>
#include <stddef.h>

extern int darwin_art_property_log_format_buffer(char*, size_t, const char*, ...);
int main(void) {
  char output[128];
  errno = EDOM;
  darwin_art_bionic_errno_store(2);
  int length = darwin_art_property_log_format_buffer(
      output, sizeof(output), "property %s %d: %m", "request", 42);
  if (length <= 0 || strcmp(output, "property request 42: No such file or directory") != 0)
    return 1;
  if (errno != EDOM || darwin_art_bionic_errno_load() != 2) return 2;
  return 0;
}
