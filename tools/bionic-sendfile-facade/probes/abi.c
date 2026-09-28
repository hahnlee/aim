#include <stddef.h>
#include <stdint.h>
#include <sys/sendfile.h>

_Static_assert(sizeof(off_t) == 8, "Android arm64 off_t");
_Static_assert(sizeof(ssize_t) == 8, "Android arm64 ssize_t");
ssize_t (*sendfile_signature)(int, int, off_t*, size_t) = sendfile;
