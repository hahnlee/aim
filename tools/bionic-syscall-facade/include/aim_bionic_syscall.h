#ifndef AIM_BIONIC_SYSCALL_H_
#define AIM_BIONIC_SYSCALL_H_

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef void (*AimBionicSyscallFunction)(void);

long aim_bionic_syscall(long number, ...);
int aim_bionic_gettid(void);
/* Fixed host ABI used by libc wrappers; unlike raw syscalls get returns zero
 * and clears the caller's mask tail. Never call the Android variadic ABI from C. */
int aim_bionic_affinity_get(int tid, size_t capacity, void* mask);
int aim_bionic_affinity_set(int tid, size_t capacity, const void* mask);
AimBionicSyscallFunction aim_bionic_syscall_resolve(
    const char* soname, const char* symbol, const char* version);
const char* aim_bionic_syscall_capability(const char* capability);

/* Diagnostic state used by deterministic contention gates. */
size_t aim_bionic_syscall_waiter_count(const int32_t* address);
void aim_bionic_syscall_spurious_wake(const int32_t* address);

#ifdef __cplusplus
}  // extern "C"
#endif

#endif  // AIM_BIONIC_SYSCALL_H_
