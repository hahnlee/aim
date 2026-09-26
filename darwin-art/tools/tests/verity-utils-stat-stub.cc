// Test-only: the filesystem bridge's guest stat, never reached by the digest test.
#include <cstdlib>
#include <sys/stat.h>
extern "C" int darwin_art_test_libcore_stat(const char*, struct stat*) { std::abort(); }
extern "C" int darwin_art_bionic_fs_resolve_private_host_path(const char*, char*, size_t) {
  std::abort();
}
extern "C" long darwin_art_bionic_fs_fd_private_host_path(int, char*, size_t) { std::abort(); }
