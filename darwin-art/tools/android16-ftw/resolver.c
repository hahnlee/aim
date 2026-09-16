// libc traversal export selection; policy and implementation stay in AOSP.
#include "darwin_art_bionic_fs.h"
#include <string.h>
struct AndroidFtw;
extern int darwin_art_aosp_ftw(
    const char*, int (*)(const char*, const DarwinArtAndroidStat*, int), int);
extern int darwin_art_aosp_nftw(
    const char*, int (*)(const char*, const DarwinArtAndroidStat*, int,
                        struct AndroidFtw*), int, int);
DarwinArtBionicFsFunction darwin_art_bionic_ftw_resolve(const char* name) {
  if (name == NULL) return NULL;
  if (strcmp(name, "ftw") == 0)
    return (DarwinArtBionicFsFunction)darwin_art_aosp_ftw;
  if (strcmp(name, "nftw") == 0)
    return (DarwinArtBionicFsFunction)darwin_art_aosp_nftw;
  return NULL;
}
