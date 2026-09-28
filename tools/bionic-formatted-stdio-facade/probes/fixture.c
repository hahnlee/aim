#include <errno.h>
#include <stdarg.h>
#include <stdio.h>

static int via_vfprintf(FILE* file, const char* format, ...) {
  va_list args;
  va_start(args, format);
  int result = vfprintf(file, format, args);
  va_end(args);
  return result;
}

__attribute__((visibility("default"))) int
bionic_formatted_stdio_fixture_fprintf(FILE* file) {
  return fprintf(file,
                 "F:%d,%d,%d,%d,%d,%d,%d,%d|"
                 "%.1f,%.1f,%.1f,%.1f,%.1f,%.1f,%.1f,%.1f,%.1f,%.1f\n",
                 1, 2, 3, 4, 5, 6, 7, 8, 0.5, 1.5, 2.5, 3.5, 4.5,
                 5.5, 6.5, 7.5, 8.5, 9.5);
}

__attribute__((visibility("default"))) int
bionic_formatted_stdio_fixture_vfprintf(FILE* file) {
  return via_vfprintf(file,
                      "V:%d,%d,%d,%d,%d,%d,%d,%d|"
                      "%.1f,%.1f,%.1f,%.1f,%.1f,%.1f,%.1f,%.1f,%.1f,%.1f\n",
                      -1, -2, -3, -4, -5, -6, -7, -8, 10.5, 11.5,
                      12.5, 13.5, 14.5, 15.5, 16.5, 17.5, 18.5, 19.5);
}

__attribute__((visibility("default"))) int
bionic_formatted_stdio_fixture_rejected(FILE* file) {
  int untouched = 91;
  const char rejected[] = {'b', 'a', 'd', '%', 'n', 0};
  errno = 0;
  if (fprintf(file, rejected, &untouched) != -1 || untouched != 91 ||
      errno != 95) {
    return 10;
  }
  errno = 0;
  if (fprintf(file, "%1048577d", 7) != -1 || errno != 27) return 11;
  static char unterminated[4096];
  for (size_t i = 0; i < sizeof(unterminated); ++i) unterminated[i] = 'x';
  errno = 0;
  if (fprintf(file, unterminated, 0) != -1 || errno != 7) return 12;
  return 42;
}

__attribute__((visibility("default"))) int
bionic_formatted_stdio_fixture_capacity(FILE* file) {
  errno = 0;
  return fprintf(file, "%1048576d", 7) == -1 && errno == 27 ? 42 : 13;
}

__attribute__((visibility("default"))) int
bionic_formatted_stdio_fixture_foreign_file(FILE* file) {
  errno = 0;
  return fprintf(file, "must-not-write") == -1 && errno == 9 ? 42 : 14;
}
