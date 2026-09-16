#include "../compat/filesystem/archive_open.h"
#include <cassert>
#include <cerrno>
#include <cstring>
#include <cstdlib>
#include <fcntl.h>
#include <unistd.h>
#include <iostream>
static int calls = 0;
static int Guest(const char* path) {
  assert(std::strcmp(path, "/system/framework/test.apk") == 0);
  ++calls;
  errno = EACCES;
  return -1;
}
int main() {
  char path[] = "/tmp/archive-open-test.XXXXXX";
  int seed = mkstemp(path);
  assert(seed >= 0);
  close(seed);
  darwin_art_set_archive_opener(&Guest);
  int fd = darwin_art_archive_open(path, O_RDONLY | O_CLOEXEC, 0);
  assert(fd >= 0 && calls == 0);
  close(fd);
  assert(darwin_art_archive_open("/system/framework/test.apk", O_RDONLY, 0) == -1);
  assert(errno == EACCES && calls == 1);
  assert(darwin_art_archive_open(path, O_CREAT | O_WRONLY, 0600) == -1 && errno == EINVAL);
  darwin_art_set_archive_opener(nullptr);
  unlink(path);
  std::cout << "archive open boundary PASS\n";
}
