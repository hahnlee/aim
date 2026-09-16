// Original property client and the compiled AOSP formatter both use native
// Darwin PCS. This boundary forwards va_list unchanged, not as Android varargs.
#include <stdarg.h>
#include "darwin_art_bionic_abort.h"

extern int darwin_art_property_log_format_log_va_list(int, const char*,
                                                      const char*, va_list);
extern void darwin_art_property_log_fatal_va_list(const char*, const char*, va_list);

int darwin_art_property_import_async_safe_format_log(int priority,
                                                     const char* tag,
                                                     const char* format, ...) {
  va_list args;
  va_start(args, format);
  int result = darwin_art_property_log_format_log_va_list(priority, tag, format, args);
  va_end(args);
  return result;
}
void darwin_art_property_import_async_safe_fatal_no_abort(const char* format, ...) {
  va_list args;
  va_start(args, format);
  darwin_art_property_log_fatal_va_list(0, format, args);
  va_end(args);
}
__attribute__((noreturn)) void darwin_art_property_import_abort(void) {
  darwin_art_bionic_abort();
}
