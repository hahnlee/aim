#pragma once
#include <stddef.h>
#ifdef __cplusplus
extern "C" {
#endif
// Private host loader API, not a guest libdl or open implementation.
// On success (0), canonical contains the NUL-terminated guest path and out_fd
// receives an owned read-only host fd. Close it with host close(), not guest
// close(). On failure (-1), out_fd is -1 and Android errno is set. Buffers must
// not overlap. The caller must check namespace permissions on this path and
// load from the returned fd, not reopen the pathname. No ELF validation or
// namespace permission is implied by filesystem admission alone.
int darwin_art_fs_open_native_image(const char* path, char* canonical,
                                   size_t capacity, int* out_fd);
// Same host-fd ownership, but requires a directory. Relative paths resolve
// from guest cwd. Does not grant namespace permissions or parse APK entries.
int darwin_art_fs_open_native_directory(const char* path, char* canonical,
                                       size_t capacity, int* out_fd);
#ifdef __cplusplus
}
#endif
