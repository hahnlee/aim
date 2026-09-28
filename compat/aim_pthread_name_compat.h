#pragma once

#include <pthread.h>
#include <stddef.h>

#ifdef __cplusplus
extern "C" {
#endif
int aim_pthread_getname_np(pthread_t thread, char* name, size_t size);
int aim_pthread_setname_np(pthread_t thread, const char* name);
#ifdef __cplusplus
}
#endif

// Mach-O modules converted from Android source retain Bionic's two-argument
// pthread_setname_np and logical cross-thread naming contract.
#define pthread_getname_np aim_pthread_getname_np
#define pthread_setname_np aim_pthread_setname_np
