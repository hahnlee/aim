#pragma once

#ifdef __cplusplus
extern "C" {
#endif
int sync_wait(int fd, int timeout_ms);
#ifdef __cplusplus
}
#endif
