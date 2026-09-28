#pragma once

#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

#define TIMEOUT_NEVER (-1)

struct sync_fence_info {
  char obj_name[32];
  char driver_name[32];
  int32_t status;
  uint64_t timestamp_ns;
};

struct sync_file_info {
  char name[32];
  int32_t status;
  uint32_t flags;
  uint32_t num_fences;
  uint32_t pad;
  uint64_t sync_fence_info;
};

int sync_wait(int fd, int timeout_ms);
int sync_merge(const char* name, int fd1, int fd2);
struct sync_file_info* sync_file_info(int fd);
struct sync_fence_info* sync_get_fence_info(struct sync_file_info* info);
void sync_file_info_free(struct sync_file_info* info);

#ifdef __cplusplus
}
#endif
