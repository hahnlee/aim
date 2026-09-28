#include <android/dlext.h>
#include <dlfcn.h>
#include <stddef.h>

_Static_assert(RTLD_LOCAL == 0, "Android RTLD_LOCAL drift");
_Static_assert(RTLD_LAZY == 1, "Android RTLD_LAZY drift");
_Static_assert(RTLD_NOW == 2, "Android RTLD_NOW drift");
_Static_assert(RTLD_NOLOAD == 4, "Android RTLD_NOLOAD drift");
_Static_assert(RTLD_GLOBAL == 0x100, "Android RTLD_GLOBAL drift");
_Static_assert(RTLD_NODELETE == 0x1000, "Android RTLD_NODELETE drift");
_Static_assert(ANDROID_DLEXT_VALID_FLAG_BITS == 0x67f,
               "Android dlext valid mask drift");
_Static_assert(sizeof(android_dlextinfo) == 48, "android_dlextinfo size drift");
_Static_assert(_Alignof(android_dlextinfo) == 8,
               "android_dlextinfo alignment drift");
_Static_assert(offsetof(android_dlextinfo, flags) == 0, "flags offset drift");
_Static_assert(offsetof(android_dlextinfo, reserved_addr) == 8,
               "reserved_addr offset drift");
_Static_assert(offsetof(android_dlextinfo, reserved_size) == 16,
               "reserved_size offset drift");
_Static_assert(offsetof(android_dlextinfo, relro_fd) == 24,
               "relro_fd offset drift");
_Static_assert(offsetof(android_dlextinfo, library_fd) == 28,
               "library_fd offset drift");
_Static_assert(offsetof(android_dlextinfo, library_fd_offset) == 32,
               "library_fd_offset drift");
_Static_assert(offsetof(android_dlextinfo, library_namespace) == 40,
               "library_namespace offset drift");

int main(void) { return 0; }
