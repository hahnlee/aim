// libc traversal export selection; policy and implementation stay in AOSP.
#include "aim_bionic_fs.h"
#include <string.h>
struct AndroidFtw;
extern int aim_aosp_ftw(
    const char*, int (*)(const char*, const AimAndroidStat*, int), int);
extern int aim_aosp_nftw(
    const char*, int (*)(const char*, const AimAndroidStat*, int,
                        struct AndroidFtw*), int, int);
AimBionicFsFunction aim_bionic_ftw_resolve(const char* name) {
  if (name == NULL) return NULL;
  if (strcmp(name, "ftw") == 0)
    return (AimBionicFsFunction)aim_aosp_ftw;
  if (strcmp(name, "nftw") == 0)
    return (AimBionicFsFunction)aim_aosp_nftw;
  return NULL;
}
