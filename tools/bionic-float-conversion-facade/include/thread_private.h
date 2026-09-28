#ifndef AIM_GDTOA_THREAD_PRIVATE_H_
#define AIM_GDTOA_THREAD_PRIVATE_H_

#ifdef __cplusplus
extern "C" {
#endif

#ifndef AIM_GDTOA_LOCAL_LOCKS
extern void* __dtoa_locks[];
#endif
void aim_gdtoa_lock(void* identity);
void aim_gdtoa_unlock(void* identity);

#ifdef __cplusplus
}
#endif

#define _MUTEX_LOCK(lock) aim_gdtoa_lock((void*)(lock))
#define _MUTEX_UNLOCK(lock) aim_gdtoa_unlock((void*)(lock))

#endif  // AIM_GDTOA_THREAD_PRIVATE_H_
