#ifndef AIM_BIONIC_STAT_H_
#define AIM_BIONIC_STAT_H_
#include <stdint.h>
typedef struct AimAndroidTimespec {
  int64_t tv_sec;
  int64_t tv_nsec;
} AimAndroidTimespec;
typedef struct AimAndroidStat {
  uint64_t st_dev;
  uint64_t st_ino;
  uint32_t st_mode;
  uint32_t st_nlink;
  uint32_t st_uid;
  uint32_t st_gid;
  uint64_t st_rdev;
  uint64_t __pad1;
  int64_t st_size;
  int32_t st_blksize;
  int32_t __pad2;
  int64_t st_blocks;
  AimAndroidTimespec st_atim;
  AimAndroidTimespec st_mtim;
  AimAndroidTimespec st_ctim;
  uint32_t __unused4;
  uint32_t __unused5;
} AimAndroidStat;
#endif
