// Component integration test against an installed guest root containing fixture.
#include "darwin_art_bionic_fs.h"
#include <cstdio>
#include <cstring>
#include <initializer_list>

struct AndroidFtw { int base; int level; };
extern "C" int darwin_art_aosp_nftw(
    const char*, int (*)(const char*, const DarwinArtAndroidStat*, int, AndroidFtw*),
    int, int);
namespace {
int directories;
int files;
int Visit(const char* path, const DarwinArtAndroidStat* status, int kind,
          AndroidFtw* position) {
  if (std::strcmp(path, "/") == 0 && kind == 1 && position->level == 0) {
    ++directories;
    return 0;
  }
  if (std::strcmp(path, "/fixture") == 0 && kind == 0 &&
      position->level == 1 && status->st_size == 1) {
    ++files;
    return 0;
  }
  std::fprintf(stderr, "ftw: unexpected path=%s kind=%d level=%d\n",
               path, kind, position->level);
  return 99;
}
}
extern "C" int darwin_art_ftw_traversal_smoke(void) {
  for (int flags : {1, 9}) {
    directories = files = 0;
    const int result = darwin_art_aosp_nftw("/", Visit, 8, flags);
    char cwd[64];
    if (result != 0 || directories != 1 || files != 1 ||
        darwin_art_bionic_getcwd(cwd, sizeof(cwd)) == nullptr ||
        std::strcmp(cwd, "/") != 0) {
      std::fprintf(stderr, "ftw: failure flags=%d result=%d dirs=%d files=%d\n",
                   flags, result, directories, files);
      return 1;
    }
  }
  std::fprintf(stderr, "ftw: original traversal guest-root + CHDIR PASS\n");
  return 0;
}
