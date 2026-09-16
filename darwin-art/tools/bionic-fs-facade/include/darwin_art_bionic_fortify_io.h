#ifndef DARWIN_ART_BIONIC_FORTIFY_IO_H_
#define DARWIN_ART_BIONIC_FORTIFY_IO_H_
#include <stddef.h>
#include <stdint.h>
#include <stdlib.h>

// Android arm64 count/buffer checks from bionic fortify.cpp and
// private/bionic_fortify.h, revision 09a271af557444c9a6b3f3146d6d474156fd6cdb.
// Invalid claims are fatal before entering the filesystem provider. Darwin
// abort supplies process termination; Android fatal-message reporting is not
// implemented by this boundary.
static inline void darwin_art_bionic_check_io_buffer(size_t count,
                                                    size_t buffer_size) {
  if (count > INT64_MAX || count > buffer_size) abort();
}
#endif
